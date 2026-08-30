//! MLX-LM Chat Provider：由 aiworkd 管理持久 Python JSONL worker（mlx-lm）。
//!
//! 模型是 MLX 格式目录（config.json + safetensors 权重）。load 时拉起 worker
//! 并等待 readiness 帧；chat 请求经 stdin JSON 帧下发，worker 逐 token 输出
//! delta 帧，映射为 OpenAI 兼容 SSE chunk。worker 崩溃不会带走 daemon；
//! 客户端提前断开时，残余生成帧按 request id 识别为 stale 并跳过，不污染
//! 下一个请求。同一 worker 一次只承载一个模型（lane 并发 = 1）。

use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

use ai_core::model::ModelSpec;
use ai_core::provider::{
    Capability, ChatProvider, ChatStream, IsolationMode, ModelHandle, Provider, ProviderDescriptor,
    ProviderError, ProviderHealth, ProviderStatus,
};
use ai_core::request::ChatRequest;
use ai_core::response::{
    ChatChoice, ChatChunk, ChatChunkChoice, ChatChunkDelta, ChatResponse, ChatResponseMessage,
    ChatUsage,
};
use ai_core::AIError;

use crate::process_memory::resident_memory_bytes;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
const DEFAULT_LOAD_TIMEOUT_SECS: u64 = 300;
const DEFAULT_INFERENCE_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MAX_TOKENS: u64 = 1024;
const DEFAULT_TEMPERATURE: f64 = 0.7;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

const PYTHON_ENV: &str = "AIWORK_MLX_LM_PYTHON";
const SCRIPT_ENV: &str = "AIWORK_MLX_LM_SCRIPT";
const DEFAULT_PYTHON: &str = ".build/mlx-lm-venv/bin/python";
const DEFAULT_SCRIPT: &str = "scripts/mlx_lm_worker.py";
const INSTALL_HINT: &str = "create .build/mlx-lm-venv and install mlx-lm==0.31.3";

struct WorkerProcess {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

struct MlxState {
    model_id: String,
    worker: WorkerProcess,
}

#[derive(Clone)]
struct MlxResident {
    model_id: String,
    pid: Option<u32>,
    device: String,
}

pub struct MlxLmProvider {
    /// Arc 化是为了让流式 chunk 循环持有锁与 resident 快照：ChatStream 要求
    /// 'static，provider 自身只能以 &self 进入 chat_stream。
    state: Arc<Mutex<Option<MlxState>>>,
    resident: Arc<StdRwLock<Option<MlxResident>>>,
}

impl MlxLmProvider {
    pub fn from_env() -> Self {
        Self {
            state: Arc::new(Mutex::new(None)),
            resident: Arc::new(StdRwLock::new(None)),
        }
    }

    fn resident_snapshot(&self) -> Option<MlxResident> {
        self.resident
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn probe_ready(&self) -> bool {
        let Ok(mut guard) = self.state.try_lock() else {
            return self.resident_snapshot().is_some();
        };
        let ready = match guard.as_mut() {
            Some(state) => matches!(state.worker.child.try_wait(), Ok(None)),
            None => false,
        };
        if !ready {
            set_resident(&self.resident, None);
        }
        ready
    }

    async fn terminate_locked(&self, guard: &mut Option<MlxState>) {
        terminate_worker(guard, &self.resident).await;
    }
}

async fn terminate_worker(
    guard: &mut Option<MlxState>,
    resident: &Arc<StdRwLock<Option<MlxResident>>>,
) {
    if let Some(state) = guard.take() {
        set_resident(resident, None);
        let mut worker = state.worker;
        let _ = worker.stdin.shutdown().await;
        if timeout(SHUTDOWN_GRACE, worker.child.wait()).await.is_err() {
            let _ = worker.child.start_kill();
            let _ = worker.child.wait().await;
        }
    }
}

fn set_resident(resident: &StdRwLock<Option<MlxResident>>, value: Option<MlxResident>) {
    *resident
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = value;
}

fn resolve_python() -> Option<PathBuf> {
    if let Ok(value) = env::var(PYTHON_ENV) {
        let candidate = PathBuf::from(value.trim());
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    resolve_project_file(Path::new(DEFAULT_PYTHON))
}

fn resolve_script() -> Option<PathBuf> {
    if let Ok(value) = env::var(SCRIPT_ENV) {
        let candidate = PathBuf::from(value.trim());
        if candidate.is_file() {
            return Some(candidate.canonicalize().unwrap_or(candidate));
        }
    }
    resolve_project_file(Path::new(DEFAULT_SCRIPT)).map(|path| path.canonicalize().unwrap_or(path))
}

fn resolve_project_file(relative: &Path) -> Option<PathBuf> {
    [".", "..", "../.."]
        .into_iter()
        .map(|base| PathBuf::from(base).join(relative))
        .find(|candidate| candidate.is_file())
}

fn load_timeout() -> Duration {
    env_duration("AIWORK_MLX_LM_LOAD_TIMEOUT_SECS", DEFAULT_LOAD_TIMEOUT_SECS)
}

fn inference_timeout() -> Duration {
    env_duration(
        "AIWORK_MLX_LM_INFERENCE_TIMEOUT_SECS",
        DEFAULT_INFERENCE_TIMEOUT_SECS,
    )
}

fn env_duration(name: &str, default_secs: u64) -> Duration {
    let seconds = env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_secs);
    Duration::from_secs(seconds)
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Deserialize)]
struct WorkerError {
    code: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct ReadyReply {
    ready: bool,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    error: Option<WorkerError>,
}

#[derive(Debug, Deserialize)]
struct WorkerFrame {
    id: Option<u64>,
    #[serde(default)]
    delta: Option<String>,
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    tokens: Option<u64>,
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    error: Option<WorkerError>,
}

#[derive(Debug)]
enum WorkerEvent {
    Delta(String),
    /// 上一个被客户端放弃的请求残留帧：跳过，直到当前 id 的帧出现。
    Stale,
    Final {
        prompt_tokens: Option<u64>,
        tokens: Option<u64>,
        finish_reason: Option<String>,
    },
    Failed {
        kind: AIError,
        message: String,
    },
}

/// OpenAI finish_reason 语义：max_tokens 截断报 "length"，自然结束报 "stop"。
/// 缺失或未知值（旧版 worker）按 "stop" 处理。
fn normalize_finish_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("length") => "length",
        _ => "stop",
    }
}

#[derive(Debug)]
struct FrameFailure {
    error: ProviderError,
    fatal: bool,
}

fn classify_frame(expected_id: u64, line: &str) -> Result<WorkerEvent, FrameFailure> {
    let frame: WorkerFrame = match serde_json::from_str(line.trim()) {
        Ok(frame) => frame,
        Err(error) => {
            return Err(FrameFailure {
                error: ProviderError::new(
                    AIError::BackendCrashed,
                    format!("invalid MLX-LM worker frame: {error}"),
                ),
                fatal: true,
            })
        }
    };
    if frame.id != Some(expected_id) {
        return Ok(WorkerEvent::Stale);
    }
    if let Some(error) = frame.error {
        return Ok(WorkerEvent::Failed {
            kind: worker_error_kind(&error.code),
            message: error.message,
        });
    }
    if let Some(delta) = frame.delta {
        return Ok(WorkerEvent::Delta(delta));
    }
    if frame.ok == Some(true) {
        return Ok(WorkerEvent::Final {
            prompt_tokens: frame.prompt_tokens,
            tokens: frame.tokens,
            finish_reason: frame.finish_reason,
        });
    }
    if frame.ok == Some(false) {
        return Ok(WorkerEvent::Failed {
            kind: AIError::BackendCrashed,
            message: "MLX-LM worker reported an unspecified failure".to_string(),
        });
    }
    Err(FrameFailure {
        error: ProviderError::new(
            AIError::BackendCrashed,
            "MLX-LM worker sent a frame with neither delta nor ok",
        ),
        fatal: true,
    })
}

fn worker_error_kind(code: &str) -> AIError {
    match code {
        "invalid_request" => AIError::InvalidRequest,
        "model_load_failed" => AIError::ModelLoadFailed,
        _ => AIError::BackendCrashed,
    }
}

fn chat_payload(
    request_id: u64,
    request: &ChatRequest,
) -> Result<serde_json::Value, ProviderError> {
    if request.messages.is_empty() {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "chat request has no messages",
        ));
    }
    Ok(serde_json::json!({
        "id": request_id,
        "messages": request.messages,
        "max_tokens": request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "temperature": request.temperature.unwrap_or(DEFAULT_TEMPERATURE),
    }))
}

async fn write_request(
    worker: &mut WorkerProcess,
    payload: &serde_json::Value,
) -> Result<(), ProviderError> {
    let mut line = payload.to_string();
    line.push('\n');
    worker
        .stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|error| {
            ProviderError::new(
                AIError::BackendCrashed,
                format!("MLX-LM worker is unreachable: {error}"),
            )
        })?;
    worker.stdin.flush().await.map_err(|error| {
        ProviderError::new(
            AIError::BackendCrashed,
            format!("MLX-LM worker flush failed: {error}"),
        )
    })
}

fn chunk(
    completion_id: &str,
    created: u64,
    model: &str,
    delta: ChatChunkDelta,
    finish_reason: Option<&str>,
    usage: Option<ChatUsage>,
) -> ChatChunk {
    ChatChunk {
        id: completion_id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created,
        model: model.to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta,
            finish_reason: finish_reason.map(str::to_string),
        }],
        usage,
    }
}

fn final_usage(prompt_tokens: Option<u64>, tokens: Option<u64>) -> ChatUsage {
    let prompt_tokens = prompt_tokens.unwrap_or(0);
    let completion_tokens = tokens.unwrap_or(0);
    ChatUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    }
}

#[async_trait]
impl Provider for MlxLmProvider {
    fn id(&self) -> &'static str {
        "mlx-lm"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::Chat, Capability::Completion]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id().to_string(),
            capabilities: self.capabilities(),
            isolation: IsolationMode::Worker,
            supported_devices: vec!["metal".to_string()],
        }
    }

    async fn status(&self) -> ProviderStatus {
        let ready = self.probe_ready();
        let python_found = resolve_python().is_some();
        let script_found = resolve_script().is_some();
        let resident = self.resident_snapshot();
        let effective_device = ready
            .then(|| resident.as_ref().map(|value| value.device.clone()))
            .flatten();
        let resident_models = resident
            .map(|value| vec![value.model_id])
            .unwrap_or_default();
        let reason = if !python_found {
            Some("MLX-LM Python environment was not found".to_string())
        } else if !script_found {
            Some("scripts/mlx_lm_worker.py was not found".to_string())
        } else if ready {
            effective_device
                .as_ref()
                .map(|device| format!("MLX-LM worker is ready on {device}"))
        } else {
            Some("no MLX-LM model is loaded".to_string())
        };
        ProviderStatus {
            available: python_found && script_found,
            ready,
            effective_device,
            resident_models,
            reason,
            install_hint: (!python_found).then(|| INSTALL_HINT.to_string()),
        }
    }

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, ProviderError> {
        let python = resolve_python().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "MLX-LM Python environment was not found",
            )
        })?;
        let script = resolve_script().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "scripts/mlx_lm_worker.py was not found",
            )
        })?;
        let model_path = model.path.as_deref().ok_or_else(|| {
            ProviderError::new(
                AIError::InvalidRequest,
                format!("MLX-LM model '{}' has no local model path", model.id),
            )
        })?;
        let model_dir = Path::new(model_path).canonicalize().map_err(|_| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("MLX-LM model directory for '{}' was not found", model.id),
            )
        })?;
        validate_model_dir(&model_dir)?;

        let mut child = Command::new(&python)
            .arg(&script)
            .arg("--model")
            .arg(&model_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                ProviderError::new(
                    AIError::ProviderUnavailable,
                    format!("failed to start MLX-LM worker: {error}"),
                )
            })?;
        let pid = child.id();
        let stdin = child.stdin.take().ok_or_else(|| {
            ProviderError::new(AIError::Internal, "MLX-LM worker stdin unavailable")
        })?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            ProviderError::new(AIError::Internal, "MLX-LM worker stdout unavailable")
        })?;

        let mut line = String::new();
        let readiness = timeout(load_timeout(), async {
            BufReader::new(&mut stdout).read_line(&mut line).await
        })
        .await;
        match readiness {
            Ok(Ok(count)) if count > 0 => {}
            Ok(Ok(_)) => {
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::ModelLoadFailed,
                    "MLX-LM worker exited before reporting readiness",
                ));
            }
            Ok(Err(error)) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::ModelLoadFailed,
                    format!("failed to read MLX-LM readiness: {error}"),
                ));
            }
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::Timeout,
                    format!(
                        "MLX-LM model load timed out after {} seconds",
                        load_timeout().as_secs()
                    ),
                ));
            }
        }
        let reply: ReadyReply = serde_json::from_str(line.trim()).map_err(|error| {
            ProviderError::new(
                AIError::ModelLoadFailed,
                format!("invalid MLX-LM readiness response: {error}"),
            )
        })?;
        if !reply.ready {
            let message = reply
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "MLX-LM model could not be loaded".to_string());
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(ProviderError::new(AIError::ModelLoadFailed, message));
        }
        let effective_device = reply.device.unwrap_or_else(|| "metal".to_string());
        *self.state.lock().await = Some(MlxState {
            model_id: model.id.clone(),
            worker: WorkerProcess {
                child,
                stdin,
                stdout,
            },
        });
        set_resident(
            &self.resident,
            Some(MlxResident {
                model_id: model.id.clone(),
                pid,
                device: effective_device,
            }),
        );
        Ok(ModelHandle {
            model_id: model.id.clone(),
            provider_id: self.id().to_string(),
        })
    }

    async fn unload(&self, handle: &ModelHandle) -> Result<(), ProviderError> {
        let mut guard = self.state.lock().await;
        if let Some(state) = guard.as_ref() {
            if state.model_id != handle.model_id {
                return Err(ProviderError::new(
                    AIError::InvalidRequest,
                    format!(
                        "model '{}' is not resident; MLX-LM currently holds '{}'",
                        handle.model_id, state.model_id
                    ),
                ));
            }
        }
        self.terminate_locked(&mut guard).await;
        Ok(())
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        let status = self.status().await;
        Ok(ProviderHealth {
            ok: status.available && (!status.ready || self.probe_ready()),
            message: status.reason,
        })
    }

    async fn memory_usage_bytes(&self) -> Option<u64> {
        resident_memory_bytes(self.resident_snapshot()?.pid?)
    }

    async fn effective_device(&self) -> Option<String> {
        self.resident_snapshot().map(|value| value.device)
    }
}

#[async_trait]
impl ChatProvider for MlxLmProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let model = request.model.clone();
        let mut stream = self.chat_stream(request).await?;
        let (mut id, mut created) = (String::new(), 0u64);
        let (mut content, mut finish_reason) = (String::new(), None);
        let mut usage = ChatUsage::default();
        while let Some(item) = stream.next().await {
            match item {
                Ok(chunk) => {
                    id = chunk.id;
                    created = chunk.created;
                    if let Some(choice) = chunk.choices.first() {
                        if let Some(text) = &choice.delta.content {
                            content.push_str(text);
                        }
                        finish_reason = choice.finish_reason.clone();
                    }
                    if let Some(value) = chunk.usage {
                        usage = value;
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(ChatResponse {
            id,
            object: "chat.completion".to_string(),
            created,
            model,
            choices: vec![ChatChoice {
                index: 0,
                message: ChatResponseMessage {
                    role: "assistant".to_string(),
                    content,
                },
                finish_reason,
            }],
            usage,
        })
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, ProviderError> {
        let request_id = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let payload = chat_payload(request_id, &request)?;
        let model = request.model.clone();

        // 无锁快速失败：resident 快照足以判断模型是否在位；流内还会在持锁后复查。
        match self.resident_snapshot().map(|value| value.model_id) {
            Some(resident) if resident == model => {}
            Some(resident) => {
                return Err(ProviderError::new(
                    AIError::ModelNotFound,
                    format!("MLX-LM model '{model}' is not loaded; '{resident}' is resident"),
                ))
            }
            None => {
                return Err(ProviderError::new(
                    AIError::ModelLoadFailed,
                    "MLX-LM model is not loaded",
                ))
            }
        }

        let state = self.state.clone();
        let resident = self.resident.clone();
        let stream = async_stream::stream! {
            // 锁持有至流结束：lane 并发为 1，后续请求排队等前一个生成完成。
            let mut guard = state.lock().await;
            let write_result = match guard.as_mut() {
                Some(state) if state.model_id == model => {
                    write_request(&mut state.worker, &payload).await
                }
                Some(state) => Err(ProviderError::new(
                    AIError::ModelNotFound,
                    format!("MLX-LM model '{model}' is not loaded; '{}' is resident", state.model_id),
                )),
                None => Err(ProviderError::new(
                    AIError::ModelLoadFailed,
                    "MLX-LM model is not loaded",
                )),
            };
            if let Err(error) = write_result {
                if matches!(error.kind, AIError::BackendCrashed) {
                    terminate_worker(&mut guard, &resident).await;
                }
                yield Err(error);
                return;
            }

            let completion_id = format!("chatcmpl-mlx-{request_id}");
            let created = unix_now_secs();
            yield Ok(chunk(
                &completion_id,
                created,
                &model,
                ChatChunkDelta {
                    role: Some("assistant".to_string()),
                    content: Some(String::new()),
                },
                None,
                None,
            ));
            loop {
                let mut line = String::new();
                let read_result = {
                    // 每轮重新借用，fatal 分支需要在借用结束后 terminate。
                    let worker = match guard.as_mut() {
                        Some(state) => &mut state.worker,
                        None => {
                            yield Err(ProviderError::new(
                                AIError::ModelLoadFailed,
                                "MLX-LM worker was terminated",
                            ));
                            return;
                        }
                    };
                    timeout(
                        inference_timeout(),
                        BufReader::new(&mut worker.stdout).read_line(&mut line),
                    )
                    .await
                };
                match read_result {
                    Ok(Ok(count)) if count > 0 => {}
                    Ok(Ok(_)) => {
                        terminate_worker(&mut guard, &resident).await;
                        yield Err(ProviderError::new(
                            AIError::BackendCrashed,
                            "MLX-LM worker exited unexpectedly",
                        ));
                        return;
                    }
                    Ok(Err(error)) => {
                        terminate_worker(&mut guard, &resident).await;
                        yield Err(ProviderError::new(
                            AIError::BackendCrashed,
                            format!("MLX-LM worker read failed: {error}"),
                        ));
                        return;
                    }
                    Err(_) => {
                        terminate_worker(&mut guard, &resident).await;
                        yield Err(ProviderError::new(
                            AIError::Timeout,
                            format!(
                                "MLX-LM inference stalled for over {} seconds; worker was terminated",
                                inference_timeout().as_secs()
                            ),
                        ));
                        return;
                    }
                }
                match classify_frame(request_id, &line) {
                    Ok(WorkerEvent::Delta(text)) => {
                        yield Ok(chunk(
                            &completion_id,
                            created,
                            &model,
                            ChatChunkDelta {
                                role: None,
                                content: Some(text),
                            },
                            None,
                            None,
                        ));
                    }
                    Ok(WorkerEvent::Stale) => {}
                    Ok(WorkerEvent::Final {
                        prompt_tokens,
                        tokens,
                        finish_reason,
                    }) => {
                        yield Ok(chunk(
                            &completion_id,
                            created,
                            &model,
                            ChatChunkDelta::default(),
                            Some(normalize_finish_reason(finish_reason.as_deref())),
                            Some(final_usage(prompt_tokens, tokens)),
                        ));
                        return;
                    }
                    Ok(WorkerEvent::Failed { kind, message }) => {
                        yield Err(ProviderError::new(kind, message));
                        return;
                    }
                    Err(failure) => {
                        if failure.fatal {
                            terminate_worker(&mut guard, &resident).await;
                        }
                        yield Err(failure.error);
                        return;
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

pub(crate) fn validate_model_dir(path: &Path) -> Result<(), ProviderError> {
    if !path.is_dir() {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "MLX-LM requires a complete model directory",
        ));
    }
    if !path.join("config.json").is_file() {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "MLX-LM model directory is missing config.json",
        ));
    }
    let has_weights = std::fs::read_dir(path)
        .map_err(|error| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("cannot inspect MLX-LM model directory: {error}"),
            )
        })?
        .any(|entry| {
            entry.ok().is_some_and(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".safetensors")
            })
        });
    if !has_weights {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "MLX-LM model directory is missing .safetensors weights",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn classify_frame_maps_delta_final_and_error() {
        let delta = classify_frame(7, r#"{"id": 7, "delta": "Hel"}"#).unwrap();
        assert!(matches!(delta, WorkerEvent::Delta(text) if text == "Hel"));

        let final_frame = classify_frame(
            7,
            r#"{"id": 7, "ok": true, "text": "Hello", "prompt_tokens": 9, "tokens": 4,
                "finish_reason": "length"}"#,
        )
        .unwrap();
        assert!(matches!(
            final_frame,
            WorkerEvent::Final {
                prompt_tokens: Some(9),
                tokens: Some(4),
                finish_reason: Some(reason)
            } if reason == "length"
        ));

        // 旧版 worker 不带 finish_reason 字段也能解析。
        assert!(matches!(
            classify_frame(7, r#"{"id": 7, "ok": true, "text": "Hi"}"#).unwrap(),
            WorkerEvent::Final {
                finish_reason: None,
                ..
            }
        ));

        let failure = classify_frame(
            7,
            r#"{"id": 7, "ok": false, "error": {"code": "invalid_request", "message": "bad"}}"#,
        )
        .unwrap();
        assert!(matches!(
            failure,
            WorkerEvent::Failed {
                kind: AIError::InvalidRequest,
                ..
            }
        ));
    }

    #[test]
    fn classify_frame_treats_foreign_ids_as_stale_not_fatal() {
        let stale = classify_frame(9, r#"{"id": 8, "delta": "old"}"#).unwrap();
        assert!(matches!(stale, WorkerEvent::Stale));

        let stale_final = classify_frame(9, r#"{"id": 8, "ok": true, "text": "old"}"#).unwrap();
        assert!(matches!(stale_final, WorkerEvent::Stale));
    }

    #[test]
    fn classify_frame_rejects_unparseable_and_empty_frames() {
        let garbage = classify_frame(1, "not json").unwrap_err();
        assert!(garbage.fatal);

        let empty = classify_frame(1, r#"{"id": 1}"#).unwrap_err();
        assert!(empty.fatal);
    }

    #[test]
    fn chat_payload_applies_defaults_and_validates_messages() {
        let mut request: ChatRequest = serde_json::from_str(
            r#"{"model": "qwen3", "messages": [{"role": "user", "content": "hi"}]}"#,
        )
        .unwrap();
        let payload = chat_payload(3, &request).unwrap();
        assert_eq!(payload["id"], 3);
        assert_eq!(payload["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(payload["temperature"], DEFAULT_TEMPERATURE);

        request.max_tokens = Some(64);
        request.temperature = Some(0.2);
        let payload = chat_payload(3, &request).unwrap();
        assert_eq!(payload["max_tokens"], 64);
        assert_eq!(payload["temperature"], 0.2);

        let empty = ChatRequest::default();
        assert!(matches!(
            chat_payload(1, &empty).unwrap_err().kind,
            AIError::InvalidRequest
        ));
    }

    #[test]
    fn final_usage_sums_prompt_and_completion_tokens() {
        let usage = final_usage(Some(9), Some(4));
        assert_eq!(usage.prompt_tokens, 9);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.total_tokens, 13);

        let missing = final_usage(None, None);
        assert_eq!(missing.total_tokens, 0);
    }

    #[test]
    fn finish_reason_normalizes_to_openai_semantics() {
        assert_eq!(normalize_finish_reason(Some("length")), "length");
        assert_eq!(normalize_finish_reason(Some("stop")), "stop");
        // 未知值与缺失字段（旧版 worker）都按自然结束处理。
        assert_eq!(normalize_finish_reason(Some("weird")), "stop");
        assert_eq!(normalize_finish_reason(None), "stop");
    }

    #[test]
    fn validate_model_dir_requires_config_and_weights() {
        let directory =
            std::env::temp_dir().join(format!("mlx-lm-validate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();

        assert!(validate_model_dir(&directory).is_err());
        fs::write(directory.join("config.json"), "{}").unwrap();
        assert!(validate_model_dir(&directory).is_err());
        fs::write(directory.join("model.safetensors"), b"weights").unwrap();
        assert!(validate_model_dir(&directory).is_ok());

        let _ = fs::remove_dir_all(&directory);
        assert!(validate_model_dir(&directory).is_err());
    }
}
