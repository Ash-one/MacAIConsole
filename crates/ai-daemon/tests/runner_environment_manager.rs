//! Phase 1B daemon-owned uv environment manager 的 isolated integration tests。
//!
//! 全部使用真实 `uv` 与无外部依赖的锁定 project fixture，覆盖决策记录
//! "Implementation status and next slice" 第 8 项要求的直接证据面：
//! cold sync、重复 exact sync、stale lock、probe failure（非零退出与超时）、
//! 取消、原子提升与 daemon 重启恢复。
//!
//! uv 不可用（未安装或不在 PATH）时相关断言失败——CI 明确安装固定版本 uv。

use std::path::PathBuf;
use std::time::Duration;

use ai_daemon::runners::{
    EnvironmentError, EnvironmentManager, EnvironmentManagerConfig, EnvironmentPhase,
    TESTED_UV_VERSION,
};

const DEFAULT_PROBE: &str = "[\"{environment.python}\", \"-c\", \"print('probe-ok')\"]";

const MANIFEST: &str = r#"
schema = "macai.runner.v1"
id = "org.example.fake"
version = "0.1.0"
protocols = ["macai.runner.v1"]
capabilities = ["tts.v1"]
[entrypoint]
command = ["fake-runner"]
working_directory = "package"
[runtime]
type = "python-uv"
id = "org.example.env"
project = "."
lock = "uv.lock"
python = ">=3.10"
PROBE_PLACEHOLDER
[capacity]
max_instances = 1
max_concurrency_per_instance = 1
[timeouts]
boot_seconds = 30
load_seconds = 30
inference_seconds = 30
shutdown_seconds = 5
[security]
network_during_install = false
network_during_runtime = false
"#;

fn temp_root(label: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("macai-env-{label}-{}-{unique}", std::process::id()))
}

/// 写一个最小 uv project + lock fixture（无第三方依赖，可离线同步）。
/// `probe` 是完整的 probe argv TOML 数组字面量。
fn write_fixture(package: &std::path::Path, probe: &str) {
    std::fs::create_dir_all(package).unwrap();
    let manifest = MANIFEST.replace("PROBE_PLACEHOLDER\n", &format!("probe = {probe}\n"));
    std::fs::write(package.join("runner.toml"), manifest).unwrap();
    std::fs::write(
        package.join("pyproject.toml"),
        "[project]\nname = \"macai-env-fixture\"\nversion = \"0.1.0\"\nrequires-python = \">=3.10\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("uv.lock"),
        "version = 1\nrevision = 3\nrequires-python = \">=3.10\"\n\n[[package]]\nname = \"macai-env-fixture\"\nversion = \"0.1.0\"\nsource = { virtual = \".\" }\n",
    )
    .unwrap();
}

/// 让 lock 与 pyproject 不一致（stale lock），用于验证 `--locked` 拒绝。
fn stale_lock(package: &std::path::Path) {
    std::fs::write(
        package.join("uv.lock"),
        "version = 1\nrevision = 3\nrequires-python = \">=3.10\"\n\n[[package]]\nname = \"macai-env-fixture\"\nversion = \"9.9.9\"\nsource = { virtual = \".\" }\n",
    )
    .unwrap();
}

fn manifest_of(package: &std::path::Path) -> ai_daemon::runners::RunnerManifest {
    ai_daemon::runners::RunnerManifest::load(&package.join("runner.toml")).unwrap()
}

fn manager(root: &std::path::Path) -> EnvironmentManager {
    EnvironmentManager::new(EnvironmentManagerConfig {
        runtime_root: root.join("Runtimes/python"),
        uv_path: None,
    })
}

/// 别名：避免与测试内的 `manager` 变量绑定遮蔽冲突。
fn new_manager(root: &std::path::Path) -> EnvironmentManager {
    manager(root)
}

fn require_uv() {
    if let Err(error) = ai_daemon::runners::RunnerManifest::parse(
        &MANIFEST.replace("PROBE_PLACEHOLDER\n", &format!("probe = {DEFAULT_PROBE}\n")),
    ) {
        panic!("fixture manifest must parse: {error}");
    }
    let output = std::process::Command::new("uv")
        .arg("--version")
        .output()
        .expect("uv must be installed for Phase 1B integration tests (CI installs it explicitly)");
    assert!(
        output.status.success(),
        "uv --version must succeed for Phase 1B integration tests"
    );
}

#[tokio::test]
async fn cold_sync_probes_and_promotes_a_ready_environment() {
    require_uv();
    let root = temp_root("cold");
    let package = root.join("package");
    write_fixture(&package, DEFAULT_PROBE);
    let manager = manager(&root);

    let status = manager
        .ensure_environment(&manifest_of(&package), &package, Duration::from_secs(120))
        .await
        .expect("cold install must succeed");
    assert_eq!(status.phase, EnvironmentPhase::Ready);
    assert!(status.uv_version.is_some());
    let python = status.python.expect("status carries python info");
    assert!(
        python.version.starts_with("3."),
        "python version must be reported: {:?}",
        python.version
    );
    let env_path = status.path.expect("ready environment has a path");
    let venv_python = env_path.join(".venv/bin/python");
    assert!(venv_python.exists(), "promoted venv python must exist");
    // 原子提升后 staging 目录消失，metadata 落盘。
    assert!(!env_path.join("installing").exists());
    assert!(env_path.join("ready.json").is_file());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn repeated_sync_reuses_the_ready_environment_without_resync() {
    require_uv();
    let root = temp_root("exact");
    let package = root.join("package");
    write_fixture(&package, DEFAULT_PROBE);
    let manager = manager(&root);

    let first = manager
        .ensure_environment(&manifest_of(&package), &package, Duration::from_secs(120))
        .await
        .unwrap();
    let first_installed_at = first.installed_at;
    // 记录 venv mtime 作为「未重新 sync」的观察点。
    let venv_marker = first.path.clone().unwrap().join(".venv/pyvenv.cfg");
    let mtime = std::fs::metadata(&venv_marker).unwrap().modified().unwrap();

    let second = manager
        .ensure_environment(&manifest_of(&package), &package, Duration::from_secs(120))
        .await
        .unwrap();
    assert_eq!(second.phase, EnvironmentPhase::Ready);
    assert_eq!(
        second.path, first.path,
        "same fingerprint must reuse the same environment directory"
    );
    assert_eq!(
        second.installed_at, first_installed_at,
        "reuse must not update the install time"
    );
    assert_eq!(
        std::fs::metadata(&venv_marker).unwrap().modified().unwrap(),
        mtime,
        "venv must not be resynced when the fingerprint directory is ready"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn stale_lock_fails_before_touching_any_ready_environment() {
    require_uv();
    let root = temp_root("stale");
    let package = root.join("package");
    write_fixture(&package, DEFAULT_PROBE);
    let manager = manager(&root);

    // 先用合法 lock 建立 ready 环境。
    let ready = manager
        .ensure_environment(&manifest_of(&package), &package, Duration::from_secs(120))
        .await
        .unwrap();
    assert_eq!(ready.phase, EnvironmentPhase::Ready);
    let ready_path = ready.path.clone().unwrap();
    let venv_marker = ready_path.join(".venv/pyvenv.cfg");
    let mtime = std::fs::metadata(&venv_marker).unwrap().modified().unwrap();

    // 篡改 lock，使 pyproject 与 lock 不一致（stale lock）。
    stale_lock(&package);
    // fingerprint 随 lock 变化 → 指向新 fingerprint 目录的安装失败；
    // 旧 ready 环境保持可用。
    let error = manager
        .ensure_environment(&manifest_of(&package), &package, Duration::from_secs(120))
        .await
        .unwrap_err();
    assert!(
        matches!(error, EnvironmentError::UvOperationFailed { .. }),
        "stale lock must fail the sync: {error:?}"
    );
    let failed = manager.status("org.example.env").unwrap();
    assert_eq!(failed.phase, EnvironmentPhase::Failed);
    assert!(failed.failure.is_some());
    // 旧 ready 环境未被破坏。
    assert_eq!(
        std::fs::metadata(&venv_marker).unwrap().modified().unwrap(),
        mtime
    );
    assert!(ready_path.join("ready.json").is_file());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn failing_probe_keeps_the_staging_out_of_ready() {
    require_uv();
    let root = temp_root("probfail");
    let package = root.join("package");
    write_fixture(
        &package,
        "[\"{environment.python}\", \"-c\", \"raise SystemExit(3)\"]",
    );
    let manager = manager(&root);

    let error = manager
        .ensure_environment(&manifest_of(&package), &package, Duration::from_secs(120))
        .await
        .unwrap_err();
    assert!(
        matches!(error, EnvironmentError::ProbeFailed { .. }),
        "probe failure must surface as ProbeFailed: {error:?}"
    );
    let status = manager.status("org.example.env").unwrap();
    assert_eq!(status.phase, EnvironmentPhase::Failed);
    // 任何 fingerprint 目录都不处于 ready。
    let env_root = root.join("Runtimes/python/org.example.env");
    if let Ok(entries) = std::fs::read_dir(&env_root) {
        for entry in entries.flatten() {
            assert!(
                !entry.path().join("ready.json").is_file(),
                "failed install must not promote a ready environment"
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn probe_timeout_fails_the_install() {
    require_uv();
    let root = temp_root("probetimeout");
    let package = root.join("package");
    write_fixture(
        &package,
        "[\"{environment.python}\", \"-c\", \"while True: pass\"]",
    );
    // 缩短 boot deadline 验证超时路径。
    let manifest = std::fs::read_to_string(package.join("runner.toml"))
        .unwrap()
        .replace("boot_seconds = 30", "boot_seconds = 1");
    std::fs::write(package.join("runner.toml"), manifest).unwrap();
    let manager = manager(&root);

    let error = manager
        .ensure_environment(&manifest_of(&package), &package, Duration::from_secs(120))
        .await
        .unwrap_err();
    assert!(
        matches!(error, EnvironmentError::ProbeFailed { .. }),
        "probe timeout must fail the install: {error:?}"
    );
    let status = manager.status("org.example.env").unwrap();
    assert_eq!(status.phase, EnvironmentPhase::Failed);
    assert!(
        status
            .failure
            .as_ref()
            .unwrap()
            .message
            .contains("boot deadline"),
        "timeout message must name the boot deadline: {:?}",
        status.failure
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn cancel_before_install_reports_cancelled_and_keeps_nothing_ready() {
    require_uv();
    let root = temp_root("cancel");
    let package = root.join("package");
    write_fixture(&package, DEFAULT_PROBE);
    let manager = manager(&root);
    manager.cancel_install("org.example.env");

    let status = manager
        .ensure_environment(&manifest_of(&package), &package, Duration::from_secs(120))
        .await
        .unwrap();
    assert_eq!(status.phase, EnvironmentPhase::Failed);
    assert_eq!(
        status.failure.as_ref().unwrap().kind,
        "cancelled",
        "cancel must surface as a retryable failed status, not ready"
    );
    assert!(
        status.failure.as_ref().unwrap().retryable,
        "cancelled install must be retryable"
    );
    // 无 ready 目录残留。
    let env_root = root.join("Runtimes/python/org.example.env");
    if let Ok(entries) = std::fs::read_dir(&env_root) {
        for entry in entries.flatten() {
            assert!(!entry.path().join("ready.json").is_file());
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn daemon_restart_restores_ready_and_does_not_fake_in_progress_phases() {
    require_uv();
    let root = temp_root("restore");
    let package = root.join("package");
    write_fixture(&package, DEFAULT_PROBE);
    let manager = manager(&root);
    let installed = manager
        .ensure_environment(&manifest_of(&package), &package, Duration::from_secs(120))
        .await
        .unwrap();
    assert_eq!(installed.phase, EnvironmentPhase::Ready);

    // 「重启」：新 manager 实例从同一受管目录恢复。
    let restarted = new_manager(&root);
    let status = restarted
        .status("org.example.env")
        .expect("ready environment must be restored from disk");
    assert_eq!(status.phase, EnvironmentPhase::Ready);
    assert_eq!(status.fingerprint, installed.fingerprint);
    assert!(status.path.is_some());
    assert!(status.uv_version.is_some());
    assert!(status.python.is_some());

    // 遗留 staging 目录不伪装成额外的 ready/进行中状态。
    let env_root = root.join("Runtimes/python/org.example.env");
    let fingerprint = &installed.fingerprint;
    std::fs::create_dir_all(env_root.join(fingerprint).join("installing")).unwrap();
    let restarted_with_staging = new_manager(&root);
    let staging_status = restarted_with_staging.status("org.example.env").unwrap();
    assert_eq!(
        staging_status.phase,
        EnvironmentPhase::Ready,
        "ready restore must survive a leftover staging dir"
    );
    assert_eq!(staging_status.phase.as_str(), "ready");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn probe_runs_under_the_synced_venv_interpreter() {
    require_uv();
    let root = temp_root("probevenv");
    let package = root.join("package");
    // probe 把 sys.path 写进 probe-runtime 目录（staging 内，原子提升 rename
    // 后随目录保留）。只有运行在依赖已 sync 的 venv 解释器上，sys.path 才含
    // `<env-id>/installing/.venv/`；基础解释器（uv `--python` 输入）不含该段。
    write_fixture(
        &package,
        "[\"{environment.python}\", \"-c\", \"print(repr(__import__('sys').path), file=open('probe-syspath','w'))\"]",
    );
    let manager = manager(&root);

    let status = manager
        .ensure_environment(&manifest_of(&package), &package, Duration::from_secs(120))
        .await
        .expect("install with a path-writing probe must succeed");
    assert_eq!(status.phase, EnvironmentPhase::Ready);
    let runtime_python = status
        .runtime_python()
        .expect("ready environment exposes the synced venv interpreter");
    let env_path = status.path.expect("ready environment has a path");
    assert_eq!(
        runtime_python,
        env_path.join(".venv/bin/python"),
        "runtime_python must be the synced venv interpreter"
    );
    let recorded = std::fs::read_to_string(env_path.join("probe-runtime/probe-syspath"))
        .expect("probe must write its sys.path into the runtime dir");
    assert!(
        recorded.contains("installing/.venv/"),
        "probe must run under the synced venv interpreter, not the base interpreter: {recorded}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn uv_resolution_reports_the_actual_version() {
    require_uv();
    let root = temp_root("uvversion");
    let manager = manager(&root);
    let uv = manager.resolve_uv().expect("uv must resolve on this host");
    // 实际版本必须是可解析的 x.y[.z]，主版本号与经过测试的记录一致。
    let parsed: Vec<u32> = uv
        .version
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect();
    assert!(
        parsed.len() >= 2,
        "uv version must look like x.y[.z]: {:?}",
        uv.version
    );
    let tested: Vec<u32> = TESTED_UV_VERSION
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect();
    assert_eq!(
        parsed[0], tested[0],
        "uv major version drifted from the tested record"
    );
    assert!(!uv.path.as_os_str().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn uv_resolution_fails_with_descriptive_error_when_explicit_path_missing() {
    let root = temp_root("baduv");
    let manager = EnvironmentManager::new(EnvironmentManagerConfig {
        runtime_root: root.join("Runtimes/python"),
        uv_path: Some(PathBuf::from("/non/existent/uv_binary")),
    });
    let err = manager
        .resolve_uv()
        .expect_err("must fail for nonexistent uv");
    assert!(err
        .to_string()
        .contains("cannot run /non/existent/uv_binary"));
    let _ = std::fs::remove_dir_all(root);
}
