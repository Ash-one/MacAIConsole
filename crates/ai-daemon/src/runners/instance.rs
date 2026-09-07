//! Runner Instance manager — daemon 侧 Runner 生命周期的组合 owner（Phase 1C）。
//!
//! 职责：经 environment manager 确保 `python-uv` 环境 ready → 从已验证
//! digest 的 staging 副本启动受监督 worker → 协议 handshake/initialize →
//! load/infer/unload/shutdown → 结构化错误映射（backend_crashed、timeout、
//! protocol violation）→ 所有退出路径清理 worker 与临时目录。
//!
//! 租约（lease）、busy guard、LRU 与 keep-alive 继续由 Runtime/scheduler
//! 裁决；本模块只拥有单个 Runner instance 的进程与协议语义。生产代码
//! 不出现 fake 或 Kokoro 专属分支。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::{
    EnvironmentError, EnvironmentManager, RunnerDescriptor, RunnerManifest, RunnerProcess,
    SupervisorError,
};

/// 单个 Runner instance：监督中的子进程 + manifest 语义。
/// shutdown 时进程被 take 出来消费，None 表示已关闭。
struct RunnerInstance {
    process: Mutex<Option<RunnerProcess>>,
    state: StdRwLock<RunnerInstanceSnapshot>,
}

/// 不获取 Runner I/O 锁即可读取的实例状态。它是 provider/runtime status 的唯一
/// resident authority；environment ready 不能替代实际进程与已加载模型状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunnerInstanceSnapshot {
    pub runner_id: String,
    pub instance_id: String,
    pub pid: Option<u32>,
    pub alive: bool,
    pub loaded_model: Option<String>,
    pub effective_device: Option<String>,
    pub resident_bytes: Option<u64>,
    pub active_requests: u32,
    pub last_error: Option<String>,
}

impl RunnerInstance {
    fn snapshot(&self) -> RunnerInstanceSnapshot {
        let mut snapshot = self
            .state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if snapshot.alive {
            if let Some(pid) = snapshot.pid {
                if let Some(bytes) = crate::process_memory::resident_memory_bytes(pid) {
                    snapshot.resident_bytes = Some(bytes);
                }
            }
        } else {
            snapshot.resident_bytes = None;
        }
        snapshot
    }

    fn update_state(&self, update: impl FnOnce(&mut RunnerInstanceSnapshot)) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut state);
    }
}

/// Runner instance manager：跨模型共享的进程与协议 owner。
pub struct RunnerInstanceManager {
    registry: super::RunnerRegistry,
    environments: EnvironmentManager,
    /// 按环境 ID 单飞安装，避免同 environment 的多个模型并发 sync。
    environment_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    instances: Mutex<HashMap<String, Arc<RunnerInstance>>>,
    temp_root: PathBuf,
    request_counter: AtomicU64,
}

#[derive(Debug)]
pub enum RunnerInstanceError {
    NoTrustedRunner {
        id: String,
        requirement: String,
    },
    Environment(EnvironmentError),
    EnvironmentNotReady {
        environment_id: String,
        phase: String,
    },
    Supervisor(SupervisorError),
    ProtocolViolation {
        message: String,
    },
    RunnerReportedError {
        code: String,
        message: String,
    },
    Io {
        context: String,
        source: std::io::Error,
    },
}

impl std::fmt::Display for RunnerInstanceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTrustedRunner { id, requirement } => write!(
                formatter,
                "no trusted Runner '{id}' satisfies '{requirement}'"
            ),
            Self::Environment(error) => write!(formatter, "environment: {error}"),
            Self::EnvironmentNotReady {
                environment_id,
                phase,
            } => write!(
                formatter,
                "environment '{environment_id}' is {phase}; an explicit install is required"
            ),
            Self::Supervisor(error) => write!(formatter, "runner supervision: {error}"),
            Self::ProtocolViolation { message } => {
                write!(formatter, "runner protocol violation: {message}")
            }
            Self::RunnerReportedError { code, message } => {
                write!(formatter, "runner error {code}: {message}")
            }
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
        }
    }
}

impl std::error::Error for RunnerInstanceError {}

impl From<SupervisorError> for RunnerInstanceError {
    fn from(error: SupervisorError) -> Self {
        match error {
            SupervisorError::RunnerReported { code, message } => {
                Self::RunnerReportedError { code, message }
            }
            error => Self::Supervisor(error),
        }
    }
}

impl From<EnvironmentError> for RunnerInstanceError {
    fn from(error: EnvironmentError) -> Self {
        Self::Environment(error)
    }
}

/// infer 协议事件流的一个事件。
#[derive(Debug)]
pub enum InferEvent {
    Accepted(Value),
    Progress(Value),
    Delta(Value),
    Metrics(Value),
    /// 终态：成功结果（含 output 路径或内联 payload）。
    Result(Value),
    /// 终态：Runner 报告的失败。
    Error {
        code: String,
        message: String,
    },
    /// 终态：正常取消。
    Cancelled,
}

impl RunnerInstanceManager {
    pub fn new(
        registry: super::RunnerRegistry,
        environments: EnvironmentManager,
        temp_root: PathBuf,
    ) -> Self {
        Self {
            registry,
            environments,
            environment_locks: Mutex::new(HashMap::new()),
            instances: Mutex::new(HashMap::new()),
            temp_root,
            request_counter: AtomicU64::new(0),
        }
    }

    pub fn environments(&self) -> &EnvironmentManager {
        &self.environments
    }

    /// discovery 快照（管理面/状态用）。返回克隆，不含任何进行中状态。
    pub fn discovered(&self) -> Vec<super::RunnerDescriptor> {
        self.registry.entries().iter().cloned().collect()
    }

    /// 返回一个无需等待推理 I/O 的实例快照。若进程锁空闲，会顺便用 try_wait
    /// 刷新已退出进程；推理进行中则返回独立状态锁中的最近快照。
    pub async fn instance_snapshot(&self, runner_id: &str) -> Option<RunnerInstanceSnapshot> {
        let instance = self.instances.lock().await.get(runner_id).cloned()?;
        if let Ok(mut guard) = instance.process.try_lock() {
            if let Some(process) = guard.as_mut() {
                match process.try_wait() {
                    Ok(Some(status)) => instance.update_state(|state| {
                        state.alive = false;
                        state.loaded_model = None;
                        state.active_requests = 0;
                        state.last_error = Some(format!("runner exited with {status}"));
                    }),
                    Ok(None) => {}
                    Err(error) => instance.update_state(|state| {
                        state.alive = false;
                        state.loaded_model = None;
                        state.active_requests = 0;
                        state.last_error = Some(format!("cannot inspect runner process: {error}"));
                    }),
                }
            }
        }
        Some(instance.snapshot())
    }

    fn next_id(&self, prefix: &str) -> String {
        let n = self.request_counter.fetch_add(1, Ordering::SeqCst);
        format!("{prefix}-{n}")
    }

    /// 解析 Model Profile 声明的 Runner（SemVer 范围显式选择，无发现顺序 fallback）。
    pub fn resolve_runner(
        &self,
        runner_id: &str,
        requirement: &str,
    ) -> Result<RunnerDescriptor, RunnerInstanceError> {
        self.registry
            .trusted(runner_id, requirement)
            .cloned()
            .map_err(|_| RunnerInstanceError::NoTrustedRunner {
                id: runner_id.to_string(),
                requirement: requirement.to_string(),
            })
    }

    /// 确保环境 ready（单飞），返回最终状态。phase 未达 ready 时返回
    /// EnvironmentNotReady，把安装决策留给显式触发方。
    pub async fn ensure_environment(
        &self,
        manifest: &RunnerManifest,
        package_root: &Path,
    ) -> Result<super::EnvironmentStatus, RunnerInstanceError> {
        let environment_id = manifest.runtime.id.clone();
        tracing::info!(
            environment = %environment_id,
            package = %package_root.display(),
            "runner environment ensure started (install or verify)"
        );
        let lock = self
            .environment_locks
            .lock()
            .await
            .entry(environment_id.clone())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        let status = match self
            .environments
            .ensure_environment(manifest, package_root, Duration::from_secs(600))
            .await
        {
            Ok(status) => status,
            Err(error) => {
                tracing::warn!(
                    environment = %environment_id,
                    %error,
                    "runner environment ensure failed"
                );
                return Err(RunnerInstanceError::Environment(error));
            }
        };
        if status.phase != super::EnvironmentPhase::Ready {
            tracing::warn!(
                environment = %environment_id,
                phase = status.phase.as_str(),
                "runner environment not ready"
            );
            return Err(RunnerInstanceError::EnvironmentNotReady {
                environment_id,
                phase: status.phase.as_str().to_string(),
            });
        }
        tracing::info!(
            environment = %environment_id,
            python = ?status.runtime_python(),
            "runner environment ready"
        );
        Ok(status)
    }

    /// 启动（或复用）一个已 load 模型的 Runner instance。
    ///
    /// 流程：resolve → ensure_environment → spawn（digest 复核 + staging 副本）
    /// → initialize → load。manifest `timeouts` 决定各阶段 deadline。
    pub async fn load_model(
        &self,
        runner_id: &str,
        requirement: &str,
        model_id: &str,
        profile_payload: Value,
    ) -> Result<(), RunnerInstanceError> {
        let descriptor = self.resolve_runner(runner_id, requirement)?;
        let manifest =
            descriptor
                .manifest
                .clone()
                .ok_or_else(|| RunnerInstanceError::NoTrustedRunner {
                    id: runner_id.to_string(),
                    requirement: requirement.to_string(),
                })?;

        // 环境必须 ready（Phase 1B manager 保证）。运行解释器固定为
        // <env_root>/.venv/bin/python（lock 依赖已 sync 进该 venv）；
        // status.python 只是 uv `--python` 输入的基础解释器，不能执行依赖。
        let status = self.ensure_environment(&manifest, &descriptor.root).await?;
        let runtime_python =
            status
                .runtime_python()
                .ok_or_else(|| RunnerInstanceError::EnvironmentNotReady {
                    environment_id: manifest.runtime.id.clone(),
                    phase: status.phase.as_str().to_string(),
                })?;

        let existing = self.instances.lock().await.get(runner_id).cloned();
        if let Some(existing) = existing {
            let snapshot = self
                .instance_snapshot(runner_id)
                .await
                .unwrap_or_else(|| existing.snapshot());
            if !snapshot.alive {
                self.shutdown_instance(runner_id).await?;
            } else {
                // v1 明确是单实例/单并发；Runtime 在切换模型前先走 unload。
                let mut guard = existing.process.lock().await;
                let process =
                    guard
                        .as_mut()
                        .ok_or_else(|| RunnerInstanceError::ProtocolViolation {
                            message: "instance process was closed".to_string(),
                        })?;
                let reply = process
                    .request(
                        "load",
                        &self.next_id("load"),
                        profile_payload,
                        "loaded",
                        Duration::from_secs(manifest.timeouts.load_seconds),
                    )
                    .await;
                match reply {
                    Ok(reply) => {
                        let metadata =
                            match validate_loaded(&reply, model_id, &manifest.capabilities) {
                                Ok(metadata) => metadata,
                                Err(error) => {
                                    existing.update_state(|state| {
                                        state.alive = false;
                                        state.loaded_model = None;
                                        state.resident_bytes = None;
                                        state.last_error = Some(error.to_string());
                                    });
                                    let process = guard.take();
                                    drop(guard);
                                    if let Some(process) = process {
                                        let _ = process.shutdown().await;
                                    }
                                    self.instances.lock().await.remove(runner_id);
                                    return Err(error);
                                }
                            };
                        existing.update_state(|state| {
                            state.alive = true;
                            state.loaded_model = Some(model_id.to_string());
                            state.effective_device = metadata.effective_device;
                            state.resident_bytes = metadata.resident_bytes;
                            state.last_error = None;
                        });
                        return Ok(());
                    }
                    Err(error) => {
                        existing.update_state(|state| {
                            state.alive = false;
                            state.loaded_model = None;
                            state.resident_bytes = None;
                            state.last_error = Some(error.to_string());
                        });
                        let process = guard.take();
                        drop(guard);
                        if let Some(process) = process {
                            let _ = process.shutdown().await;
                        }
                        self.instances.lock().await.remove(runner_id);
                        return Err(error.into());
                    }
                }
            }
        }

        let instance = Arc::new(
            self.spawn_instance(&descriptor, &manifest, runtime_python)
                .await?,
        );
        {
            let mut guard = instance.process.lock().await;
            let process = guard
                .as_mut()
                .ok_or_else(|| RunnerInstanceError::ProtocolViolation {
                    message: "instance process was closed during load".to_string(),
                })?;
            let reply = process
                .request(
                    "load",
                    &self.next_id("load"),
                    profile_payload,
                    "loaded",
                    Duration::from_secs(manifest.timeouts.load_seconds),
                )
                .await;
            match reply {
                Ok(reply) => {
                    let metadata = match validate_loaded(&reply, model_id, &manifest.capabilities) {
                        Ok(metadata) => metadata,
                        Err(error) => {
                            instance.update_state(|state| {
                                state.alive = false;
                                state.loaded_model = None;
                                state.resident_bytes = None;
                                state.last_error = Some(error.to_string());
                            });
                            let process = guard.take();
                            drop(guard);
                            if let Some(process) = process {
                                let _ = process.shutdown().await;
                            }
                            return Err(error);
                        }
                    };
                    instance.update_state(|state| {
                        state.alive = true;
                        state.loaded_model = Some(model_id.to_string());
                        state.effective_device = metadata.effective_device;
                        state.resident_bytes = metadata.resident_bytes;
                        state.last_error = None;
                    });
                }
                Err(error) => {
                    instance.update_state(|state| {
                        state.alive = false;
                        state.loaded_model = None;
                        state.resident_bytes = None;
                        state.last_error = Some(error.to_string());
                    });
                    let process = guard.take();
                    drop(guard);
                    if let Some(process) = process {
                        let _ = process.shutdown().await;
                    }
                    return Err(error.into());
                }
            }
        }
        self.instances
            .lock()
            .await
            .insert(runner_id.to_string(), instance);
        Ok(())
    }

    async fn spawn_instance(
        &self,
        descriptor: &RunnerDescriptor,
        manifest: &RunnerManifest,
        runtime_python: PathBuf,
    ) -> Result<RunnerInstance, RunnerInstanceError> {
        let instance_temp = self.temp_root.join(format!(
            "runner-instance-{}-{}",
            std::process::id(),
            self.next_id("spawn")
        ));
        std::fs::create_dir_all(&instance_temp).map_err(|error| RunnerInstanceError::Io {
            context: format!("cannot create instance temp {}", instance_temp.display()),
            source: error,
        })?;
        let spawn_result = RunnerProcess::spawn(descriptor, &runtime_python, &instance_temp).await;
        let mut process = match spawn_result {
            Ok(process) => process,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&instance_temp);
                return Err(error.into());
            }
        };
        if let Err(error) = process.initialize(&manifest.id, &instance_temp).await {
            let _ = process.shutdown().await;
            let _ = std::fs::remove_dir_all(&instance_temp);
            return Err(error.into());
        }
        let instance_id = self.next_id("instance");
        let pid = process.pid();
        Ok(RunnerInstance {
            process: Mutex::new(Some(process)),
            state: StdRwLock::new(RunnerInstanceSnapshot {
                runner_id: manifest.id.clone(),
                instance_id,
                pid,
                alive: true,
                loaded_model: None,
                effective_device: None,
                resident_bytes: None,
                active_requests: 0,
                last_error: None,
            }),
        })
    }

    /// 发起一次推理，消费协议事件直到 terminal frame。
    /// `on_event` 在 daemon 侧逐事件调用；返回 terminal 语义。
    pub async fn infer<F>(
        &self,
        runner_id: &str,
        capability: &str,
        request: Value,
        output: Value,
        deadline: Duration,
        mut on_event: F,
    ) -> Result<Value, RunnerInstanceError>
    where
        F: FnMut(InferEvent),
    {
        let instance = self
            .instances
            .lock()
            .await
            .get(runner_id)
            .cloned()
            .ok_or_else(|| RunnerInstanceError::ProtocolViolation {
                message: format!("no loaded instance for runner '{runner_id}'"),
            })?;
        let request_id = self.next_id("infer");
        instance
            .update_state(|state| state.active_requests = state.active_requests.saturating_add(1));
        let mut guard = instance.process.lock().await;
        let result = async {
            let process = guard
                .as_mut()
                .ok_or_else(|| RunnerInstanceError::ProtocolViolation {
                    message: "instance process was closed".to_string(),
                })?;
            process
                .send(&super::Envelope::new(
                    "infer",
                    &request_id,
                    json!({
                        "capability": capability,
                        "request": request,
                        "output": output,
                    }),
                ))
                .await?;
            loop {
                let event = tokio::time::timeout(deadline, process.receive(deadline))
                    .await
                    .map_err(|_| {
                        RunnerInstanceError::Supervisor(SupervisorError::Deadline("inference"))
                    })??;
                if event.id != request_id {
                    return Err(RunnerInstanceError::ProtocolViolation {
                        message: format!(
                            "event id '{}' does not match inference '{request_id}'",
                            event.id
                        ),
                    });
                }
                match event.message_type.as_str() {
                    "accepted" => on_event(InferEvent::Accepted(event.payload)),
                    "progress" => on_event(InferEvent::Progress(event.payload)),
                    "delta" => on_event(InferEvent::Delta(event.payload)),
                    "metrics" => on_event(InferEvent::Metrics(event.payload)),
                    "result" => {
                        if let Some(device) = event
                            .payload
                            .get("device")
                            .or_else(|| event.payload.get("effective_device"))
                            .and_then(Value::as_str)
                            .filter(|value| !value.trim().is_empty())
                        {
                            instance.update_state(|state| {
                                state.effective_device = Some(device.to_string())
                            });
                        }
                        on_event(InferEvent::Result(event.payload.clone()));
                        return Ok(event.payload);
                    }
                    "error" => {
                        let code = event
                            .payload
                            .get("code")
                            .and_then(Value::as_str)
                            .unwrap_or("internal")
                            .to_string();
                        let message = event
                            .payload
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        on_event(InferEvent::Error {
                            code: code.clone(),
                            message: message.clone(),
                        });
                        return Err(RunnerInstanceError::RunnerReportedError { code, message });
                    }
                    "cancelled" => {
                        on_event(InferEvent::Cancelled);
                        return Err(RunnerInstanceError::RunnerReportedError {
                            code: "cancelled".to_string(),
                            message: "inference cancelled".to_string(),
                        });
                    }
                    other => {
                        return Err(RunnerInstanceError::ProtocolViolation {
                            message: format!("unexpected inference event '{other}'"),
                        });
                    }
                }
            }
        }
        .await;
        instance.update_state(|state| {
            state.active_requests = state.active_requests.saturating_sub(1);
            if matches!(
                result,
                Err(RunnerInstanceError::Supervisor(_))
                    | Err(RunnerInstanceError::ProtocolViolation { .. })
            ) {
                state.alive = false;
                state.loaded_model = None;
                state.last_error = result.as_ref().err().map(ToString::to_string);
            }
        });
        result
    }

    /// 卸载模型（实例内的模型状态）；进程保留。
    pub async fn unload_model(
        &self,
        runner_id: &str,
        deadline: Duration,
    ) -> Result<(), RunnerInstanceError> {
        let instance = self
            .instances
            .lock()
            .await
            .get(runner_id)
            .cloned()
            .ok_or_else(|| RunnerInstanceError::ProtocolViolation {
                message: format!("no loaded instance for runner '{runner_id}'"),
            })?;
        let mut guard = instance.process.lock().await;
        let process = guard
            .as_mut()
            .ok_or_else(|| RunnerInstanceError::ProtocolViolation {
                message: "instance process was closed".to_string(),
            })?;
        let result = process
            .request(
                "unload",
                &self.next_id("unload"),
                json!({}),
                "unloaded",
                deadline,
            )
            .await;
        match result {
            Ok(_) => {
                instance.update_state(|state| {
                    state.loaded_model = None;
                    state.resident_bytes = None;
                    state.last_error = None;
                });
                Ok(())
            }
            Err(error) => {
                instance.update_state(|state| {
                    state.alive = false;
                    state.loaded_model = None;
                    state.last_error = Some(error.to_string());
                });
                Err(error.into())
            }
        }
    }

    /// 关闭并移除一个 instance：graceful shutdown → kill fallback → 清理临时目录。
    pub async fn shutdown_instance(&self, runner_id: &str) -> Result<(), RunnerInstanceError> {
        let instance = self.instances.lock().await.remove(runner_id);
        let Some(instance) = instance else {
            return Ok(());
        };
        instance.update_state(|state| {
            state.alive = false;
            state.loaded_model = None;
            state.active_requests = 0;
        });
        let mut guard = instance.process.lock().await;
        // shutdown() 消费 RunnerProcess；None 表示已被关闭。
        let Some(process) = guard.take() else {
            return Ok(());
        };
        let _ = process.shutdown().await;
        Ok(())
    }
}

/// `loaded` 回复的最小契约校验（runner-protocol-v1 §load）。
struct LoadedMetadata {
    effective_device: Option<String>,
    resident_bytes: Option<u64>,
}

fn validate_loaded(
    reply: &super::Envelope,
    model_id: &str,
    expected_capabilities: &[String],
) -> Result<LoadedMetadata, RunnerInstanceError> {
    if reply.message_type != "loaded" {
        return Err(RunnerInstanceError::ProtocolViolation {
            message: format!("expected 'loaded', received '{}'", reply.message_type),
        });
    }
    let reported = reply
        .payload
        .get("model_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RunnerInstanceError::ProtocolViolation {
            message: "loaded reply omitted model_id".to_string(),
        })?;
    if reported != model_id {
        return Err(RunnerInstanceError::ProtocolViolation {
            message: format!("loaded model_id '{reported}' does not match '{model_id}'"),
        });
    }
    let effective_device = reply
        .payload
        .get("effective_device")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| RunnerInstanceError::ProtocolViolation {
            message: "loaded reply omitted effective_device".to_string(),
        })?;
    let capabilities = reply
        .payload
        .get("capabilities")
        .and_then(Value::as_array)
        .ok_or_else(|| RunnerInstanceError::ProtocolViolation {
            message: "loaded reply omitted capabilities".to_string(),
        })?;
    for expected in expected_capabilities {
        if !capabilities
            .iter()
            .any(|value| value.as_str() == Some(expected.as_str()))
        {
            return Err(RunnerInstanceError::ProtocolViolation {
                message: format!("loaded reply omitted capability '{expected}'"),
            });
        }
    }
    let max_concurrency = reply
        .payload
        .get("limits")
        .and_then(|limits| limits.get("max_concurrency"))
        .and_then(Value::as_u64)
        .ok_or_else(|| RunnerInstanceError::ProtocolViolation {
            message: "loaded reply omitted limits.max_concurrency".to_string(),
        })?;
    if max_concurrency != 1 {
        return Err(RunnerInstanceError::ProtocolViolation {
            message: format!("runner v1 requires max_concurrency=1, received {max_concurrency}"),
        });
    }
    Ok(LoadedMetadata {
        effective_device: Some(effective_device),
        resident_bytes: reply.payload.get("resident_bytes").and_then(Value::as_u64),
    })
}

/// daemon shutdown 收口：关闭当前所有存活 instance（Runtime 持有 manager）。
/// runner_id → instance 由内部 map 拥有；这里通过快照枚举关闭。
pub async fn shutdown_all_instances(manager: &RunnerInstanceManager) {
    let ids: Vec<String> = manager.instances.lock().await.keys().cloned().collect();
    for id in ids {
        if let Err(error) = manager.shutdown_instance(&id).await {
            tracing::warn!(runner = %id, %error, "runner instance shutdown failed");
        } else {
            tracing::info!(runner = %id, "runner instance shut down");
        }
    }
}
