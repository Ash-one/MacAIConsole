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
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::{
    EnvironmentError, EnvironmentManager, RunnerDescriptor, RunnerManifest, RunnerProcess,
    SupervisorError,
};

/// 单个 Runner instance：监督中的子进程 + manifest 语义。
/// shutdown 时进程被 take 出来消费，None 表示已关闭。
struct RunnerInstance {
    manifest: RunnerManifest,
    process: Mutex<Option<RunnerProcess>>,
    /// 环境的运行解释器（`<env_root>/.venv/bin/python`，manifest
    /// `{environment.python}` 模板的解析结果）。
    runtime_python: PathBuf,
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
        Self::Supervisor(error)
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
        let lock = self
            .environment_locks
            .lock()
            .await
            .entry(environment_id.clone())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        let status = self
            .environments
            .ensure_environment(manifest, package_root, Duration::from_secs(600))
            .await?;
        if status.phase != super::EnvironmentPhase::Ready {
            return Err(RunnerInstanceError::EnvironmentNotReady {
                environment_id,
                phase: status.phase.as_str().to_string(),
            });
        }
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

        let mut instances = self.instances.lock().await;
        if let Some(existing) = instances.get(runner_id) {
            // 复用 instance：同一 Runner 只有一个常驻实例（v1 容量边界）。
            let mut guard = existing.process.lock().await;
            let process = guard
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
                .await?;
            validate_loaded(&reply, model_id)?;
            return Ok(());
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
                .await?;
            validate_loaded(&reply, model_id)?;
        }
        instances.insert(runner_id.to_string(), instance);
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
        Ok(RunnerInstance {
            manifest: manifest.clone(),
            process: Mutex::new(Some(process)),
            runtime_python,
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
        let mut guard = instance.process.lock().await;
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
        process
            .request(
                "unload",
                &self.next_id("unload"),
                json!({}),
                "unloaded",
                deadline,
            )
            .await?;
        Ok(())
    }

    /// 关闭并移除一个 instance：graceful shutdown → kill fallback → 清理临时目录。
    pub async fn shutdown_instance(&self, runner_id: &str) -> Result<(), RunnerInstanceError> {
        let instance = self.instances.lock().await.remove(runner_id);
        let Some(instance) = instance else {
            return Ok(());
        };
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
fn validate_loaded(reply: &super::Envelope, model_id: &str) -> Result<(), RunnerInstanceError> {
    if reply.message_type != "loaded" {
        return Err(RunnerInstanceError::ProtocolViolation {
            message: format!("expected 'loaded', received '{}'", reply.message_type),
        });
    }
    let reported = reply
        .payload
        .get("model_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !reported.is_empty() && reported != model_id {
        return Err(RunnerInstanceError::ProtocolViolation {
            message: format!("loaded model_id '{reported}' does not match '{model_id}'"),
        });
    }
    Ok(())
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
