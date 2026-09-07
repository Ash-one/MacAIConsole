//! daemon-owned `python-uv` environment manager 的单一 owner。
//!
//! 本模块拥有 uv executable 解析、受管目录布局、单飞安装锁、staging sync、
//! manifest probe、原子提升、状态描述与重启恢复。GUI / CLI / Provider 不执行
//! `uv` 或 Python；`status()` 只读内存状态，不等待安装锁或 Runner I/O。
//!
//! 行为契约见 `docs/decisions/2026-09-02-uv-python-environments.md`（staging、
//! 原子提升、状态语义）与 `docs/specs/runner-manifest-v1.md`（probe 契约）。

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use super::RunnerManifest;

const ENVIRONMENT_SCHEMA: &str = "macai.environment.v1";
const METADATA_FILE: &str = "ready.json";
const STAGING_DIR: &str = "installing";
const VENV_DIR: &str = ".venv";

/// 经过测试的 uv version 记录（本机与 CI 观测值，不构成未来版本承诺）。
pub const TESTED_UV_VERSION: &str = "0.9.21";

/// 可复现 Python environment 的最小身份输入。
///
/// fingerprint 是环境目录和升级判定的稳定输入。
#[derive(Debug, Clone)]
pub struct EnvironmentInput {
    pub id: String,
    pub project: PathBuf,
    pub lock: PathBuf,
    pub python_abi: String,
    pub platform: String,
    pub uv_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentFingerprint(pub String);

impl fmt::Display for EnvironmentFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl EnvironmentInput {
    pub fn fingerprint(&self) -> std::io::Result<EnvironmentFingerprint> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        let mut update_field = |name: &[u8], value: &[u8]| {
            hasher.update((name.len() as u64).to_be_bytes());
            hasher.update(name);
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value);
        };
        update_field(b"id", self.id.as_bytes());
        update_field(b"python_abi", self.python_abi.as_bytes());
        update_field(b"platform", self.platform.as_bytes());
        update_field(b"uv_source", self.uv_source.as_bytes());
        for (name, path) in [("pyproject", &self.project), ("uv_lock", &self.lock)] {
            let contents = std::fs::read(path)?;
            update_field(name.as_bytes(), &contents);
        }
        Ok(EnvironmentFingerprint(format!("{:x}", hasher.finalize())))
    }
}

#[derive(Debug)]
pub enum EnvironmentError {
    UvNotAvailable {
        reason: String,
    },
    PythonNotAvailable {
        constraint: String,
        reason: String,
    },
    UvOperationFailed {
        operation: &'static str,
        stderr: String,
    },
    ProbeFailed {
        reason: String,
    },
    UnsupportedRuntime {
        runtime_type: String,
    },
    Fingerprint(std::io::Error),
    Io {
        context: String,
        source: std::io::Error,
    },
}

impl fmt::Display for EnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UvNotAvailable { reason } => {
                write!(formatter, "uv executable is not available: {reason}")
            }
            Self::PythonNotAvailable { constraint, reason } => write!(
                formatter,
                "no Python for constraint '{constraint}': {reason}"
            ),
            Self::UvOperationFailed { operation, stderr } => {
                write!(formatter, "uv {operation} failed: {stderr}")
            }
            Self::ProbeFailed { reason } => write!(formatter, "environment probe failed: {reason}"),
            Self::UnsupportedRuntime { runtime_type } => write!(
                formatter,
                "environment manager does not support runtime type '{runtime_type}'"
            ),
            Self::Fingerprint(error) => {
                write!(formatter, "cannot fingerprint environment: {error}")
            }
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl std::error::Error for EnvironmentError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentPhase {
    Missing,
    Resolving,
    Syncing,
    Probing,
    Ready,
    Failed,
}

impl EnvironmentPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Resolving => "resolving",
            Self::Syncing => "syncing",
            Self::Probing => "probing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UvSource {
    pub path: PathBuf,
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PythonInfo {
    pub version: String,
    /// `managed`（uv 下载）或 `system`（满足约束的已有解释器）。
    pub source: &'static str,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentFailure {
    pub kind: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentStatus {
    pub environment_id: String,
    pub fingerprint: String,
    pub phase: EnvironmentPhase,
    pub uv_version: Option<String>,
    /// uv `--python` 输入的基础解释器（版本与 managed/system 来源）。
    /// 它不含任何 lock 依赖；probe 与 entrypoint 的实际运行解释器是
    /// `<environment path>/.venv/bin/python`，见 [`Self::runtime_python`]。
    pub python: Option<PythonInfo>,
    /// 受管环境目录：`<fingerprint>/`（含 `.venv` 与 ready.json）。
    pub path: Option<PathBuf>,
    pub lock_digest: String,
    pub installed_at: Option<u64>,
    pub failure: Option<EnvironmentFailure>,
}

impl EnvironmentStatus {
    /// 环境的实际运行解释器：`<environment path>/.venv/bin/python`。
    ///
    /// `uv sync` 经 `UV_PROJECT_ENVIRONMENT` 把 lock 依赖装进该 venv，
    /// 因此 manifest 的 `{environment.python}` 模板（probe 与 entrypoint）
    /// 只解析到这里；`python` 字段的基础解释器不能直接执行任何依赖。
    /// phase ready 前 path 未设置时返回 None。
    pub fn runtime_python(&self) -> Option<PathBuf> {
        let env_root = self.path.as_ref()?;
        Some(venv_python_path(env_root))
    }
}

#[derive(Debug, Clone)]
pub struct EnvironmentManagerConfig {
    /// 受管环境根目录，例如
    /// `~/Library/Application Support/MacAIConsole/Runtimes/python`。
    pub runtime_root: PathBuf,
}

impl Default for EnvironmentManagerConfig {
    fn default() -> Self {
        Self {
            runtime_root: std::env::temp_dir().join("macai-runtimes/python"),
        }
    }
}

impl EnvironmentManagerConfig {
    /// 生产布局：`~/Library/Application Support/MacAIConsole/Runtimes/python`。
    pub fn for_app_support() -> Self {
        let mut root = dirs_home();
        root.push("Library/Application Support/MacAIConsole/Runtimes/python");
        Self { runtime_root: root }
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// daemon-owned uv environment manager。
///
/// 安装以 environment ID 为 key 单飞（跨任务共享同一把锁）；状态查询只读
/// `Mutex<HashMap>`，绝不等待安装。cancel 仅对未进入终态的安装生效。
pub struct EnvironmentManager {
    config: EnvironmentManagerConfig,
    uv: Mutex<Option<UvSource>>,
    statuses: Mutex<HashMap<String, EnvironmentStatus>>,
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    cancelled: Mutex<HashMap<String, bool>>,
}

impl EnvironmentManager {
    /// 创建 manager 并从受管目录恢复 ready / failed 状态。
    /// daemon 重启时进行中的 phase 不会伪装成 ready。
    pub fn new(config: EnvironmentManagerConfig) -> Self {
        let manager = Self {
            config,
            uv: Mutex::new(None),
            statuses: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            cancelled: Mutex::new(HashMap::new()),
        };
        manager.restore_from_disk();
        manager
    }

    pub fn runtime_root(&self) -> &Path {
        &self.config.runtime_root
    }

    fn status_locked(
        &self,
        environment_id: &str,
        fingerprint: &str,
        lock_digest: &str,
    ) -> EnvironmentStatus {
        self.statuses
            .lock()
            .expect("status map lock")
            .entry(environment_id.to_string())
            .or_insert_with(|| EnvironmentStatus {
                environment_id: environment_id.to_string(),
                fingerprint: fingerprint.to_string(),
                phase: EnvironmentPhase::Missing,
                uv_version: None,
                python: None,
                path: None,
                lock_digest: lock_digest.to_string(),
                installed_at: None,
                failure: None,
            })
            .clone()
    }

    fn set_status(&self, status: EnvironmentStatus) {
        self.statuses
            .lock()
            .expect("status map lock")
            .insert(status.environment_id.clone(), status);
    }

    /// 快照某个环境的状态。调用方不需要持有任何安装锁。
    pub fn status(&self, environment_id: &str) -> Option<EnvironmentStatus> {
        self.statuses
            .lock()
            .expect("status map lock")
            .get(environment_id)
            .cloned()
    }

    pub fn statuses(&self) -> Vec<EnvironmentStatus> {
        self.statuses
            .lock()
            .expect("status map lock")
            .values()
            .cloned()
            .collect()
    }

    /// 定位并报告实际 uv executable。解析顺序：`MACAI_UV_PATH` 显式配置 →
    /// App Bundle 内置产物 → 系统/开发态 `PATH` → 常见系统与用户路径 fallback。
    /// 版本来自实际 `uv --version` 输出。
    pub fn resolve_uv(&self) -> Result<UvSource, EnvironmentError> {
        if let Some(cached) = self.uv.lock().expect("uv cache lock").clone() {
            return Ok(cached);
        }
        let source = self.locate_uv()?;
        self.uv
            .lock()
            .expect("uv cache lock")
            .replace(source.clone());
        Ok(source)
    }

    fn locate_uv(&self) -> Result<UvSource, EnvironmentError> {
        let mut candidates = Vec::new();
        if let Some(configured) = std::env::var_os("MACAI_UV_PATH") {
            candidates.push(PathBuf::from(configured));
        }
        if let Some(bundle_uv) = search_bundle_for("uv") {
            if !candidates.contains(&bundle_uv) {
                candidates.push(bundle_uv);
            }
        }
        if let Some(path_uv) = search_path_for("uv") {
            if !candidates.contains(&path_uv) {
                candidates.push(path_uv);
            }
        }
        for fallback in common_uv_fallback_paths() {
            if fallback.is_file() && !candidates.contains(&fallback) {
                candidates.push(fallback);
            }
        }
        for candidate in candidates {
            let output = std::process::Command::new(&candidate)
                .arg("--version")
                .output();
            let output = match output {
                Ok(output) if output.status.success() => output,
                Ok(output) => {
                    if is_path_fallback(&candidate) {
                        continue;
                    }
                    return Err(EnvironmentError::UvNotAvailable {
                        reason: format!("{} exited with {}", candidate.display(), output.status),
                    });
                }
                Err(error) => {
                    // MACAI_UV_PATH 是显式配置，失败即报错；fallback 失败
                    // 则继续尝试下一个候选。
                    if is_path_fallback(&candidate) {
                        continue;
                    }
                    return Err(EnvironmentError::UvNotAvailable {
                        reason: format!("cannot run {}: {error}", candidate.display()),
                    });
                }
            };
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let version = stdout
                .strip_prefix("uv ")
                .and_then(|rest| rest.split_whitespace().next())
                .ok_or_else(|| EnvironmentError::UvNotAvailable {
                    reason: format!("unrecognized `uv --version` output: '{stdout}'"),
                })?
                .to_string();
            return Ok(UvSource {
                path: candidate,
                version,
            });
        }
        Err(EnvironmentError::UvNotAvailable {
            reason: "no 'uv' executable found (checked MACAI_UV_PATH, App Bundle, PATH, and standard directories ~/.local/bin/uv, /opt/homebrew/bin/uv)".to_string(),
        })
    }

    /// 解析满足 manifest `python` 约束的解释器；缺失时由 uv 下载受管
    /// CPython（只在用户显式触发的安装中允许联网）。
    async fn resolve_python(
        &self,
        uv: &UvSource,
        constraint: &str,
    ) -> Result<PythonInfo, EnvironmentError> {
        self.run_uv(
            uv,
            "python-install",
            &["python", "install", constraint],
            false,
            Duration::from_secs(600),
        )
        .await?;
        let output = self
            .run_uv(
                uv,
                "python-find",
                &["python", "find", "--offline", constraint],
                true,
                Duration::from_secs(60),
            )
            .await?;
        let path = PathBuf::from(output.trim());
        let version_output = tokio::process::Command::new(&path)
            .args(["-c", "import sys;print('%d.%d.%d'%sys.version_info[:3])"])
            .env_clear()
            .output()
            .await
            .map_err(|error| EnvironmentError::Io {
                context: format!("cannot query {}", path.display()),
                source: error,
            })?;
        let version = String::from_utf8_lossy(&version_output.stdout)
            .trim()
            .to_string();
        if version.is_empty() {
            return Err(EnvironmentError::PythonNotAvailable {
                constraint: constraint.to_string(),
                reason: format!("interpreter at {} reported no version", path.display()),
            });
        }
        let source = if path.starts_with(python_install_dir(&self.config.runtime_root)) {
            "managed"
        } else {
            "system"
        };
        Ok(PythonInfo {
            version,
            source,
            path,
        })
    }

    /// 确保某个 manifest 声明的 `python-uv` 环境可用，返回最终状态。
    ///
    /// 流程：fingerprint → 单飞锁 → staging `uv sync --locked --no-dev` →
    /// manifest probe（离线、`boot_seconds` deadline）→ 原子提升。
    /// 失败或取消时保留已有 ready 环境。
    pub async fn ensure_environment(
        &self,
        manifest: &RunnerManifest,
        package_root: &Path,
        timeout: Duration,
    ) -> Result<EnvironmentStatus, EnvironmentError> {
        if manifest.runtime.runtime_type != "python-uv" {
            return Err(EnvironmentError::UnsupportedRuntime {
                runtime_type: manifest.runtime.runtime_type.clone(),
            });
        }
        let uv = self.resolve_uv()?;
        let environment_id = manifest.runtime.id.clone();
        let python = self
            .resolve_python(&uv, python_constraint(manifest))
            .await?;
        let input = EnvironmentInput {
            id: environment_id.clone(),
            project: package_root
                .join(&manifest.runtime.project)
                .join("pyproject.toml"),
            lock: package_root.join(&manifest.runtime.lock),
            python_abi: format!("cpython-{}", python.version),
            platform: platform_tag(),
            uv_source: format!("uv-{}", uv.version),
        };
        let fingerprint = input.fingerprint().map_err(EnvironmentError::Fingerprint)?;
        let lock_digest = file_digest(&input.lock).map_err(EnvironmentError::Fingerprint)?;

        let mut status = self.status_locked(&environment_id, &fingerprint.0, &lock_digest);
        status.fingerprint = fingerprint.0.clone();
        status.lock_digest = lock_digest.clone();
        status.uv_version = Some(uv.version.clone());
        status.python = Some(python.clone());
        self.set_status(status.clone());

        let env_root = self
            .config
            .runtime_root
            .join(&environment_id)
            .join(&fingerprint.0);
        if is_ready_dir(&env_root) {
            let mut status = status.clone();
            status.phase = EnvironmentPhase::Ready;
            status.path = Some(env_root);
            status.installed_at = self.installed_at(&environment_id, &fingerprint.0);
            self.set_status(status.clone());
            return Ok(status);
        }

        let lock = self
            .locks
            .lock()
            .expect("lock map lock")
            .entry(environment_id.clone())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        // 已有 ready 环境优先返回；取消只影响尚未完成的安装。
        if is_ready_dir(&env_root) {
            let mut status = status.clone();
            status.phase = EnvironmentPhase::Ready;
            status.path = Some(env_root.clone());
            status.installed_at = self.installed_at(&environment_id, &fingerprint.0);
            self.set_status(status.clone());
            return Ok(status);
        }
        if self.take_cancelled(&environment_id) {
            let mut cancelled = status.clone();
            cancelled.phase = EnvironmentPhase::Failed;
            cancelled.failure = Some(EnvironmentFailure {
                kind: "cancelled".to_string(),
                message: "install cancelled".to_string(),
                retryable: true,
            });
            self.set_status(cancelled.clone());
            return Ok(cancelled);
        }

        status.phase = EnvironmentPhase::Syncing;
        self.set_status(status.clone());
        // staging 与最终 fingerprint 目录同父级（Runtimes/python/<env-id>/），
        // rename 在同一目录内原子完成：installing → <fingerprint>。
        let env_parent = self.config.runtime_root.join(&environment_id);
        let staging = env_parent.join(STAGING_DIR);
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|error| EnvironmentError::Io {
            context: format!("cannot create staging {}", staging.display()),
            source: error,
        })?;

        let uv_for_sync = uv.clone();
        let staging_for_sync = staging.clone();
        let project = input.project.clone();
        let python_path = python.path.clone();
        let runtime_root = self.config.runtime_root.clone();
        let sync_result = tokio::time::timeout(
            timeout,
            tokio::task::spawn_blocking(move || {
                run_sync(
                    &uv_for_sync,
                    &project,
                    &staging_for_sync,
                    &python_path,
                    &runtime_root,
                )
            }),
        )
        .await;
        let sync_result = match sync_result {
            Ok(joined) => joined.unwrap_or_else(|error| {
                Err(EnvironmentError::Io {
                    context: "uv sync worker panicked".to_string(),
                    source: std::io::Error::new(std::io::ErrorKind::Other, error.to_string()),
                })
            }),
            Err(_) => Err(EnvironmentError::UvOperationFailed {
                operation: "sync",
                stderr: "sync exceeded the install deadline".to_string(),
            }),
        };
        if let Err(error) = sync_result {
            return self
                .fail_install(&environment_id, status, error, true)
                .await;
        }

        status.phase = EnvironmentPhase::Probing;
        self.set_status(status.clone());
        let probe = self.run_probe(manifest, package_root, &staging).await;
        if let Err(error) = probe {
            return self
                .fail_install(&environment_id, status, error, true)
                .await;
        }

        // probe 成功后原子提升：staging 目录 rename 为最终 fingerprint 目录。
        std::fs::rename(&staging, &env_root).map_err(|error| EnvironmentError::Io {
            context: format!(
                "cannot promote {} to {}",
                staging.display(),
                env_root.display()
            ),
            source: error,
        })?;
        self.persist_metadata(
            &env_root,
            &environment_id,
            &fingerprint.0,
            &uv.version,
            &python,
            &lock_digest,
        )?;
        status.phase = EnvironmentPhase::Ready;
        status.path = Some(env_root);
        status.installed_at = Some(unix_now());
        self.set_status(status.clone());
        Ok(status)
    }
    /// 请求取消一个尚未完成的安装。已进入终态的环境不受影响。
    pub fn cancel_install(&self, environment_id: &str) {
        self.cancelled
            .lock()
            .expect("cancel map lock")
            .insert(environment_id.to_string(), true);
    }

    fn take_cancelled(&self, environment_id: &str) -> bool {
        self.cancelled
            .lock()
            .expect("cancel map lock")
            .remove(environment_id)
            .unwrap_or(false)
    }

    async fn fail_install(
        &self,
        environment_id: &str,
        mut status: EnvironmentStatus,
        error: EnvironmentError,
        retryable: bool,
    ) -> Result<EnvironmentStatus, EnvironmentError> {
        let _ = std::fs::remove_dir_all(
            self.config
                .runtime_root
                .join(environment_id)
                .join(STAGING_DIR),
        );
        status.phase = EnvironmentPhase::Failed;
        status.failure = Some(EnvironmentFailure {
            kind: failure_kind(&error),
            message: error.to_string(),
            retryable,
        });
        self.set_status(status.clone());
        Err(error)
    }

    async fn run_probe(
        &self,
        manifest: &RunnerManifest,
        package_root: &Path,
        staging: &Path,
    ) -> Result<(), EnvironmentError> {
        // probe 必须运行在依赖已同步的 staging venv 解释器上。基础解释器只
        // 作为 uv `--python` 输入、不含任何 lock 依赖——真实 Kokoro probe 曾
        // 因解析到基础解释器而报 `ModuleNotFoundError: mlx_audio`。
        let environment_python = venv_python_path(staging);
        if !environment_python.is_file() {
            return Err(EnvironmentError::ProbeFailed {
                reason: format!(
                    "uv sync did not produce {}; the probe cannot run",
                    environment_python.display()
                ),
            });
        }
        let runtime_temp = staging.join("probe-runtime");
        std::fs::create_dir_all(&runtime_temp).map_err(|error| EnvironmentError::Io {
            context: format!("cannot create probe runtime dir {runtime_temp:?}"),
            source: error,
        })?;
        let command = manifest
            .resolve_probe(package_root, &environment_python, &runtime_temp)
            .map_err(|error| EnvironmentError::ProbeFailed {
                reason: format!("invalid probe command: {error}"),
            })?;
        let (program, arguments) = command.split_first().ok_or(EnvironmentError::ProbeFailed {
            reason: "probe command is empty".to_string(),
        })?;
        let deadline = Duration::from_secs(manifest.timeouts.boot_seconds);
        let spawned = tokio::process::Command::new(program)
            .args(arguments)
            .current_dir(runtime_temp)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .env_clear()
            .envs(inherited_environment(manifest))
            .output();
        let output = tokio::time::timeout(deadline, spawned).await;
        let output = match output {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return Err(EnvironmentError::ProbeFailed {
                    reason: format!("probe spawn failed: {error}"),
                })
            }
            Err(_) => {
                return Err(EnvironmentError::ProbeFailed {
                    reason: format!(
                        "probe exceeded the boot deadline of {}s",
                        manifest.timeouts.boot_seconds
                    ),
                })
            }
        };
        if !output.status.success() {
            return Err(EnvironmentError::ProbeFailed {
                reason: format!(
                    "probe exited with {}: {}",
                    output.status,
                    bounded_output(&output.stderr)
                ),
            });
        }
        Ok(())
    }

    fn persist_metadata(
        &self,
        env_dir: &Path,
        environment_id: &str,
        fingerprint: &str,
        uv_version: &str,
        python: &PythonInfo,
        lock_digest: &str,
    ) -> Result<(), EnvironmentError> {
        let metadata = serde_json::json!({
            "schema": ENVIRONMENT_SCHEMA,
            "environment_id": environment_id,
            "fingerprint": fingerprint,
            "uv_version": uv_version,
            "python": {
                "version": python.version,
                "source": python.source,
                "path": python.path.display().to_string(),
            },
            "lock_digest": lock_digest,
            "installed_at": unix_now(),
        });
        std::fs::write(
            env_dir.join(METADATA_FILE),
            serde_json::to_vec_pretty(&metadata).expect("metadata serializes"),
        )
        .map_err(|error| EnvironmentError::Io {
            context: format!("cannot persist {}", env_dir.join(METADATA_FILE).display()),
            source: error,
        })
    }

    fn installed_at(&self, environment_id: &str, fingerprint: &str) -> Option<u64> {
        self.statuses
            .lock()
            .expect("status map lock")
            .get(environment_id)
            .and_then(|status| {
                (status.fingerprint == fingerprint)
                    .then_some(status.installed_at)
                    .flatten()
            })
            .or_else(|| {
                let metadata = std::fs::read_to_string(
                    self.config
                        .runtime_root
                        .join(environment_id)
                        .join(fingerprint)
                        .join(METADATA_FILE),
                )
                .ok()?;
                serde_json::from_str::<Value>(&metadata)
                    .ok()?
                    .get("installed_at")?
                    .as_u64()
            })
    }

    /// daemon 重启恢复：受管目录中的 valid ready 环境恢复为 ready；
    /// 遗留 staging（崩溃或重启时进行中的安装）标记为 failed，不伪装 ready。
    fn restore_from_disk(&self) {
        let entries = match std::fs::read_dir(&self.config.runtime_root) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let environment_id = entry.file_name().to_string_lossy().into_owned();
            if environment_id.starts_with('.') {
                continue;
            }
            if !entry.path().is_dir() {
                continue;
            }
            let fingerprints = match std::fs::read_dir(entry.path()) {
                Ok(fingerprints) => fingerprints,
                Err(_) => continue,
            };
            for fingerprint in fingerprints.flatten() {
                let dir = fingerprint.path();
                if !dir.is_dir() {
                    continue;
                }
                if is_ready_dir(&dir) {
                    if let Some(status) = self.restore_ready(&environment_id, &dir) {
                        self.set_status(status);
                    }
                } else if fingerprint.file_name() == STAGING_DIR {
                    let mut status = self.status_locked(&environment_id, "", "");
                    status.phase = EnvironmentPhase::Failed;
                    status.failure = Some(EnvironmentFailure {
                        kind: "interrupted".to_string(),
                        message: "install was interrupted by a daemon restart".to_string(),
                        retryable: true,
                    });
                    self.set_status(status);
                }
            }
        }
    }

    fn restore_ready(&self, environment_id: &str, dir: &Path) -> Option<EnvironmentStatus> {
        let metadata: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(METADATA_FILE)).ok()?).ok()?;
        let fingerprint = metadata.get("fingerprint")?.as_str()?.to_string();
        if fingerprint != dir.file_name()?.to_string_lossy() {
            return None;
        }
        let python_value = metadata.get("python")?;
        let python = PythonInfo {
            version: python_value.get("version")?.as_str()?.to_string(),
            source: match python_value.get("source")?.as_str()? {
                "managed" => "managed",
                _ => "system",
            },
            path: PathBuf::from(python_value.get("path")?.as_str()?),
        };
        let mut status = self.status_locked(
            environment_id,
            &fingerprint,
            metadata
                .get("lock_digest")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        status.fingerprint = fingerprint;
        status.phase = EnvironmentPhase::Ready;
        status.uv_version = Some(
            metadata
                .get("uv_version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
        status.python = Some(python);
        status.path = Some(dir.to_path_buf());
        status.installed_at = metadata.get("installed_at").and_then(Value::as_u64);
        Some(status)
    }

    async fn run_uv(
        &self,
        uv: &UvSource,
        operation: &'static str,
        arguments: &[&str],
        offline: bool,
        deadline: Duration,
    ) -> Result<String, EnvironmentError> {
        let mut command = tokio::process::Command::new(&uv.path);
        command
            .args(arguments)
            .env_clear()
            .env("HOME", dirs_home())
            .env(
                "UV_PYTHON_INSTALL_DIR",
                python_install_dir(&self.config.runtime_root),
            )
            .envs(python_mirror_environment());
        if offline {
            command.env("UV_OFFLINE", "1");
        } else {
            command.envs(proxy_environment());
        }
        let output = tokio::time::timeout(deadline, command.output())
            .await
            .map_err(|_| EnvironmentError::UvOperationFailed {
                operation,
                stderr: format!("operation exceeded the {operation} deadline"),
            })?
            .map_err(|error| EnvironmentError::Io {
                context: format!("cannot run uv {operation}"),
                source: error,
            })?;
        if !output.status.success() {
            return Err(EnvironmentError::UvOperationFailed {
                operation,
                stderr: bounded_output(&output.stderr),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn run_sync(
    uv: &UvSource,
    project: &Path,
    staging: &Path,
    python_path: &Path,
    runtime_root: &Path,
) -> Result<(), EnvironmentError> {
    // 安装使用 uv 默认共享 cache（不强制私有 UV_CACHE_DIR）：热 cache 时
    // `--locked` 安装可离线完成；本机 Phase 2 已灌满 ~/.cache/uv。PyPI 源
    // 替换与已提交 uv.lock 的 registry 绑定冲突（--locked 会要求重锁），
    // 加速只能经共享 cache 预热或代理，不能替换 lock 的 index 身份。
    let output = std::process::Command::new(&uv.path)
        .args(["sync", "--project"])
        .arg(project)
        .args(["--locked", "--no-dev", "--python"])
        .arg(python_path)
        .env_clear()
        .env("HOME", dirs_home())
        .env("UV_PYTHON_INSTALL_DIR", python_install_dir(runtime_root))
        .env("UV_PROJECT_ENVIRONMENT", staging.join(VENV_DIR))
        .envs(proxy_environment())
        .output()
        .map_err(|error| EnvironmentError::Io {
            context: "cannot run uv sync".to_string(),
            source: error,
        })?;
    if !output.status.success() {
        return Err(EnvironmentError::UvOperationFailed {
            operation: "sync",
            stderr: bounded_output(&output.stderr),
        });
    }
    Ok(())
}

fn python_constraint(manifest: &RunnerManifest) -> &str {
    manifest.runtime.python.as_deref().unwrap_or(">=3.12,<3.13")
}

fn platform_tag() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// 在父进程 PATH 中查找可执行文件并返回绝对路径。environment manager 的
/// 子进程一律 `env_clear()`，uv 必须以绝对路径启动。
fn search_path_for(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// 检查当前进程是否位于 macOS App Bundle 中并探测内置可执行文件：
/// 1. 与当前可执行文件同目录（如 Contents/MacOS/<program>）
/// 2. 资源目录（如 Contents/Resources/<program>）
fn search_bundle_for(program: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;

    // 1. 同级目录 (例如 Contents/MacOS/<program>)
    let sibling = exe_dir.join(program);
    if sibling.is_file() {
        return Some(sibling);
    }

    // 2. Resources 目录 (例如 Contents/MacOS/../Resources/<program>)
    if let Some(contents_dir) = exe_dir.parent() {
        let res_prog = contents_dir.join("Resources").join(program);
        if res_prog.is_file() {
            return Some(res_prog);
        }
    }

    None
}

/// 常见系统与用户安装目录中的 uv 候选路径。
fn common_uv_fallback_paths() -> Vec<PathBuf> {
    let home = dirs_home();
    vec![
        PathBuf::from("/opt/homebrew/bin/uv"),
        PathBuf::from("/usr/local/bin/uv"),
        home.join(".local/bin/uv"),
        home.join(".cargo/bin/uv"),
    ]
}

/// 判断候选是否来自 fallback（而非 MACAI_UV_PATH 显式配置）。
fn is_path_fallback(candidate: &Path) -> bool {
    std::env::var_os("MACAI_UV_PATH")
        .map(|configured| PathBuf::from(&configured) != candidate)
        .unwrap_or(true)
}

/// 运行解释器路径：`uv sync`（`UV_PROJECT_ENVIRONMENT`）创建的 venv 解释器。
/// probe 与 Runner entrypoint 的 `{environment.python}` 模板都解析到这里。
fn venv_python_path(env_root: &Path) -> PathBuf {
    env_root.join(VENV_DIR).join("bin/python")
}

fn is_ready_dir(dir: &Path) -> bool {
    dir.join(METADATA_FILE).is_file() && venv_python_path(dir).is_file()
}

fn file_digest(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let contents = std::fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(&contents)))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn python_install_dir(runtime_root: &Path) -> PathBuf {
    runtime_root.join(".pythons")
}

fn inherited_environment(manifest: &RunnerManifest) -> Vec<(String, String)> {
    manifest
        .security
        .inherit_environment
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (name.clone(), value)))
        .collect()
}

/// uv 决策：daemon 统一传递已经脱敏的代理配置。只允许协议级代理变量，
/// 绝不传递凭据类变量。
fn proxy_environment() -> Vec<(String, String)> {
    ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY"]
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_string(), value))
        })
        .collect()
}

/// uv 受管 Python 下载镜像。`MACAI_UV_PYTHON_INSTALL_MIRROR` 显式设置时以
/// `UV_PYTHON_INSTALL_MIRROR` 传给 `uv python install`（astral
/// python-build-standalone 走 GitHub，部分网络下慢且易断；镜像 URL 由运维
/// 提供，需与 GitHub release 的 `<tag>/<asset>` 目录布局一致）。默认关闭。
fn python_mirror_environment() -> Vec<(String, String)> {
    std::env::var_os("MACAI_UV_PYTHON_INSTALL_MIRROR")
        .map(|value| {
            vec![(
                "UV_PYTHON_INSTALL_MIRROR".to_string(),
                value.to_string_lossy().into_owned(),
            )]
        })
        .unwrap_or_default()
}

fn bounded_output(bytes: &[u8]) -> String {
    const MAX: usize = 4 * 1024;
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if text.len() <= MAX {
        return text.to_string();
    }
    let mut cut = MAX;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

fn failure_kind(error: &EnvironmentError) -> String {
    match error {
        EnvironmentError::UvNotAvailable { .. } => "uv_unavailable".to_string(),
        EnvironmentError::PythonNotAvailable { .. } => "python_unavailable".to_string(),
        EnvironmentError::UvOperationFailed { operation, .. } => format!("uv_{operation}_failed"),
        EnvironmentError::ProbeFailed { .. } => "probe_failed".to_string(),
        EnvironmentError::Fingerprint(_) | EnvironmentError::Io { .. } => "io_error".to_string(),
        EnvironmentError::UnsupportedRuntime { .. } => "unsupported_runtime".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_uv_fallback_paths_contain_standard_system_and_user_locations() {
        let paths = common_uv_fallback_paths();
        assert!(paths.iter().any(|p| p.ends_with(".local/bin/uv")));
        assert!(paths.iter().any(|p| p.ends_with(".cargo/bin/uv")));
        assert!(paths.iter().any(|p| p == Path::new("/opt/homebrew/bin/uv")));
        assert!(paths.iter().any(|p| p == Path::new("/usr/local/bin/uv")));
    }

    #[test]
    fn path_fallback_distinguishes_explicit_env_from_fallbacks() {
        let dummy = Path::new("/custom/tools/uv");
        // 当未设置 MACAI_UV_PATH 时，任何路径均视为 fallback
        std::env::remove_var("MACAI_UV_PATH");
        assert!(is_path_fallback(dummy));

        // 当显式设置 MACAI_UV_PATH 时，与配置相同的路径不是 fallback，其余均为 fallback
        std::env::set_var("MACAI_UV_PATH", "/custom/tools/uv");
        assert!(!is_path_fallback(dummy));
        assert!(is_path_fallback(Path::new("/other/bin/uv")));
        std::env::remove_var("MACAI_UV_PATH");
    }
}
