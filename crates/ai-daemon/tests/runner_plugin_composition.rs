use std::collections::HashSet;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ai_daemon::runners::{
    RunnerProcess, RunnerRegistry, RunnerRegistryError, RunnerState, SupervisorError,
};
use serde_json::json;

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
id = "org.example.fake-python"
project = "."
lock = "uv.lock"
python = ">=3.12,<3.13"
probe = ["{environment.python}", "-c", "print('probe')"]
[capacity]
max_instances = 1
max_concurrency_per_instance = 1
[timeouts]
boot_seconds = 5
load_seconds = 5
inference_seconds = 5
shutdown_seconds = 5
[security]
network_during_install = false
network_during_runtime = false
[[models]]
profile = "profiles/fake.toml"
adapter = "fake"
"#;

const PROFILE: &str = r#"
schema = "macai.model.v1"
id = "fake-tts"
name = "Fake TTS"
capabilities = ["tts.v1"]
runner = "org.example.fake"
adapter = "fake"
format = "directory"
[source]
type = "huggingface"
repo = "org/fake"
revision = "0123456789abcdef0123456789abcdef01234567"
[artifacts]
directory = "fake"
files = ["model.safetensors"]
[defaults]
keep_alive = "always"
voice = "zf_001"
format = "wav"
[resources]
memory_estimate_bytes = 400000000
[compatibility]
runner = ">=0.1,<0.2"
"#;

fn test_root(label: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "macai-runner-{label}-{}-{unique}",
        std::process::id()
    ))
}

fn write_package(package: &std::path::Path, command: &str, boot_seconds: u64) {
    std::fs::create_dir_all(package.join("profiles")).unwrap();
    std::fs::write(
        package.join("runner.toml"),
        MANIFEST
            .replace(
                "command = [\"fake-runner\"]",
                &format!("command = {command}"),
            )
            .replace(
                "boot_seconds = 5",
                &format!("boot_seconds = {boot_seconds}"),
            ),
    )
    .unwrap();
    std::fs::write(package.join("pyproject.toml"), "[project]\nname='fake'\n").unwrap();
    std::fs::write(package.join("uv.lock"), "version = 1\n").unwrap();
    std::fs::write(package.join("profiles/fake.toml"), PROFILE).unwrap();
}

fn trusted_runner<'a>(registry: &'a RunnerRegistry) -> &'a ai_daemon::runners::RunnerDescriptor {
    registry
        .trusted("org.example.fake", "=0.1.0")
        .expect("built-in Runner is trusted")
}

async fn startup_failure(mode: &str) -> (std::path::PathBuf, SupervisorError) {
    let root = test_root(mode);
    let package = root.join("fake");
    write_package(
        &package,
        &format!("[\"fake-runner-startup-failure\", \"{{runtime.temp_root}}\", \"{mode}\"]"),
        1,
    );
    std::fs::copy(
        env!("CARGO_BIN_EXE_fake-runner-startup-failure"),
        package.join("fake-runner-startup-failure"),
    )
    .unwrap();
    let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
    let error =
        match RunnerProcess::spawn(trusted_runner(&registry), &package.join("unused"), &root).await
        {
            Err(error) => error,
            Ok(_) => panic!("startup failure mode {mode} unexpectedly completed the handshake"),
        };
    (root, error)
}

fn assert_startup_child_reaped(root: &std::path::Path, mode: &str) {
    let pid = std::fs::read_to_string(root.join(format!("startup-failure-{mode}.pid"))).unwrap();
    assert!(
        !Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .output()
            .unwrap()
            .status
            .success(),
        "{mode} startup failure left Runner PID {pid} alive"
    );
}

#[tokio::test]
async fn trusted_fake_runner_completes_discover_handshake_load_infer_and_unload() {
    let root = test_root("composition");
    let package = root.join("fake");
    write_package(&package, "[\"fake-runner\"]", 5);
    std::fs::copy(
        env!("CARGO_BIN_EXE_fake-runner"),
        package.join("fake-runner"),
    )
    .unwrap();

    let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
    let runner = trusted_runner(&registry);
    assert_eq!(runner.state, RunnerState::Trusted);
    let resolved = registry.resolve_bundled_profile("fake-tts").unwrap();
    assert_eq!(resolved.profile.adapter, "fake");
    assert_eq!(resolved.profile.defaults.voice.as_deref(), Some("zf_001"));
    assert_eq!(
        resolved.profile.resources.memory_estimate_bytes,
        Some(400_000_000)
    );

    let manifest = runner.manifest.as_ref().unwrap();
    let mut process = RunnerProcess::spawn(runner, &package.join("fake-runner"), &root)
        .await
        .expect("fake Runner handshake");
    process.initialize(&manifest.id, &root).await.unwrap();
    process
        .request(
            "load",
            "load-1",
            json!({"model_id": "fake-tts"}),
            "loaded",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    process
        .request(
            "infer",
            "infer-1",
            json!({"capability": "tts.v1"}),
            "accepted",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let result = process.receive(Duration::from_secs(5)).await.unwrap();
    assert_eq!(result.message_type, "result");
    process
        .request(
            "unload",
            "unload-1",
            json!({}),
            "unloaded",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(process.shutdown().await.unwrap(), "");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn boot_deadline_kills_and_reaps_the_child() {
    let (root, error) = startup_failure("timeout").await;
    match error {
        SupervisorError::Deadline("boot") => {}
        error => panic!("expected boot deadline, received {error:?}"),
    }
    assert_startup_child_reaped(&root, "timeout");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn hello_identity_failure_kills_and_reaps_the_child() {
    let (root, error) = startup_failure("identity").await;
    assert!(matches!(error, SupervisorError::Identity(_)));
    assert_startup_child_reaped(&root, "identity");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn startup_protocol_failure_kills_and_reaps_the_child() {
    let (root, error) = startup_failure("protocol").await;
    assert!(matches!(error, SupervisorError::Protocol(_)));
    assert_startup_child_reaped(&root, "protocol");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn modified_package_cannot_execute_after_discovery() {
    let root = test_root("digest");
    let package = root.join("fake");
    write_package(&package, "[\"fake-runner\"]", 5);
    std::fs::copy(
        env!("CARGO_BIN_EXE_fake-runner"),
        package.join("fake-runner"),
    )
    .unwrap();
    let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
    std::fs::write(package.join("fake-runner"), "changed after discovery").unwrap();
    let error =
        match RunnerProcess::spawn(trusted_runner(&registry), &package.join("unused"), &root).await
        {
            Err(error) => error,
            Ok(_) => panic!("changed Runner package must not execute"),
        };
    assert!(matches!(
        error,
        SupervisorError::Trust(RunnerRegistryError::PackageDigestChanged { .. })
    ));
    let _ = std::fs::remove_dir_all(root);
}
