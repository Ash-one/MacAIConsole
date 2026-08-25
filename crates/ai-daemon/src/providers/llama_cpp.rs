//! llama.cpp Provider：由 aiworkd 管理持久 `llama-server` 子进程。
//!
//! 模型在 load 时启动并保持常驻，后续 chat 请求复用同一 Metal context；
//! unload 终止子进程并释放统一内存。llama.cpp 崩溃不会带走 daemon。

use std::collections::VecDeque;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{Client, Response};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{sleep, Instant};

use ai_core::model::ModelSpec;
use ai_core::provider::{
    Capability, ChatProvider, ChatStream, IsolationMode, ModelHandle, Provider, ProviderDescriptor,
    ProviderError, ProviderHealth, ProviderStatus,
};
use ai_core::request::ChatRequest;
use ai_core::response::{ChatChunk, ChatResponse};
use ai_core::AIError;

use crate::process_memory::resident_memory_bytes;

const DEFAULT_PORT: u16 = 11436;
const LOG_TAIL_LINES: usize = 40;

struct ProcessState {
    child: Child,
    model_id: String,
    effective_device: Arc<StdMutex<String>>,
    log_tail: Arc<StdMutex<VecDeque<String>>>,
}

pub struct LlamaCppProvider {
    client: Client,
    port: u16,
    process: Mutex<Option<ProcessState>>,
}

impl LlamaCppProvider {
    pub fn from_env() -> Self {
        let port = env::var("AIWORK_LLAMA_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(300))
                .build()
                .expect("build llama.cpp HTTP client"),
            port,
            process: Mutex::new(None),
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn resolve_binary() -> Option<PathBuf> {
        if let Ok(value) = env::var("AIWORK_LLAMA_SERVER") {
            let value = value.trim();
            if !value.is_empty() {
                return resolve_executable(value);
            }
        }

        if let Ok(cwd) = env::current_dir() {
            let development = cwd.join(".build/llama.cpp/bin/llama-server");
            if is_executable_file(&development) {
                return Some(development);
            }
        }

        resolve_executable("llama-server")
    }

    async fn probe_health(&self) -> bool {
        self.client
            .get(format!("{}/health", self.base_url()))
            .send()
            .await
            .map(|response| response.status().is_success())
            .unwrap_or(false)
    }

    async fn stop_process(mut state: ProcessState) {
        let _ = state.child.kill().await;
        let _ = state.child.wait().await;
    }

    fn log_tail(state: &ProcessState) -> String {
        state
            .log_tail
            .lock()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }

    fn effective_device(state: &ProcessState) -> String {
        state
            .effective_device
            .lock()
            .map(|device| device.clone())
            .unwrap_or_else(|_| "cpu".to_string())
    }

    fn detect_effective_device(logs: &str) -> String {
        let logs = logs.to_ascii_lowercase();
        if logs.contains("ggml_metal")
            || logs.contains("mtl0")
            || logs.contains("| mtl :")
            || (logs.contains("metal") && logs.contains("offload"))
        {
            "metal".to_string()
        } else {
            "cpu".to_string()
        }
    }

    async fn backend_error(response: Response, kind: AIError) -> ProviderError {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/error/message")
                    .or_else(|| value.get("message"))
                    .and_then(|message| message.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| body.trim().to_string());
        ProviderError::new(
            kind,
            if message.is_empty() {
                format!("llama-server returned HTTP {status}")
            } else {
                format!("llama-server returned HTTP {status}: {message}")
            },
        )
    }
}

#[async_trait]
impl Provider for LlamaCppProvider {
    fn id(&self) -> &'static str {
        "llama.cpp"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::Chat, Capability::Completion]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id().to_string(),
            capabilities: self.capabilities(),
            isolation: IsolationMode::Worker,
            supported_devices: vec!["metal".to_string(), "cpu".to_string()],
        }
    }

    async fn status(&self) -> ProviderStatus {
        let Some(binary) = Self::resolve_binary() else {
            return ProviderStatus {
                available: false,
                ready: false,
                effective_device: None,
                resident_models: vec![],
                reason: Some("llama-server executable was not found".to_string()),
                install_hint: Some(
                    "build llama.cpp or set AIWORK_LLAMA_SERVER=/path/to/llama-server".to_string(),
                ),
            };
        };

        let mut process = self.process.lock().await;
        let mut resident_models = Vec::new();
        let mut reason = None;
        let mut alive = false;
        let mut effective_device = None;
        if let Some(state) = process.as_mut() {
            match state.child.try_wait() {
                Ok(None) => {
                    alive = true;
                    resident_models.push(state.model_id.clone());
                    effective_device = Some(Self::effective_device(state));
                }
                Ok(Some(exit)) => {
                    reason = Some(format!("llama-server exited with {exit}"));
                    *process = None;
                }
                Err(error) => {
                    reason = Some(format!("could not inspect llama-server: {error}"));
                }
            }
        }
        drop(process);

        ProviderStatus {
            available: true,
            ready: alive && self.probe_health().await,
            effective_device,
            resident_models,
            reason: reason.or_else(|| Some(format!("using {}", binary.display()))),
            install_hint: None,
        }
    }

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, ProviderError> {
        let path = model.path.as_deref().ok_or_else(|| {
            ProviderError::new(
                AIError::InvalidRequest,
                format!("model '{}' has no local GGUF path", model.id),
            )
        })?;
        let path = Path::new(path).canonicalize().map_err(|error| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("cannot open GGUF '{}': {error}", path),
            )
        })?;
        if path.extension().and_then(|value| value.to_str()) != Some("gguf") {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                format!("llama.cpp requires a .gguf model: {}", path.display()),
            ));
        }

        let binary = Self::resolve_binary().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "llama-server executable was not found; build llama.cpp or set AIWORK_LLAMA_SERVER",
            )
        })?;

        let mut slot = self.process.lock().await;
        if let Some(mut current) = slot.take() {
            let same_model =
                current.model_id == model.id && matches!(current.child.try_wait(), Ok(None));
            if same_model && self.probe_health().await {
                *slot = Some(current);
                return Ok(ModelHandle {
                    model_id: model.id.clone(),
                    provider_id: self.id().to_string(),
                });
            }
            Self::stop_process(current).await;
        }

        if tokio::net::TcpStream::connect(("127.0.0.1", self.port))
            .await
            .is_ok()
        {
            return Err(ProviderError::new(
                AIError::ProviderUnavailable,
                format!(
                    "internal llama.cpp port {} is already used by another process",
                    self.port
                ),
            ));
        }

        let context_length = model.context_length.unwrap_or(4096).max(512);
        let mut command = Command::new(&binary);
        command
            .arg("-m")
            .arg(&path)
            .arg("-a")
            .arg(&model.id)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(self.port.to_string())
            .arg("-c")
            .arg(context_length.to_string())
            .arg("-ngl")
            .arg("99")
            .arg("-np")
            .arg("1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        tracing::info!(
            provider = self.id(),
            model = %model.id,
            path = %path.display(),
            binary = %binary.display(),
            port = self.port,
            "starting llama-server"
        );

        let mut child = command.spawn().map_err(|error| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                format!("failed to start '{}': {error}", binary.display()),
            )
        })?;
        let log_tail = Arc::new(StdMutex::new(VecDeque::new()));
        let effective_device = Arc::new(StdMutex::new("cpu".to_string()));
        if let Some(stderr) = child.stderr.take() {
            let captured = Arc::clone(&log_tail);
            let captured_device = Arc::clone(&effective_device);
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "llama-server", "{line}");
                    if Self::detect_effective_device(&line) == "metal" {
                        if let Ok(mut device) = captured_device.lock() {
                            *device = "metal".to_string();
                        }
                    }
                    if let Ok(mut tail) = captured.lock() {
                        if tail.len() == LOG_TAIL_LINES {
                            tail.pop_front();
                        }
                        tail.push_back(line);
                    }
                }
            });
        }

        let mut state = ProcessState {
            child,
            model_id: model.id.clone(),
            effective_device,
            log_tail,
        };
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if self.probe_health().await {
                for _ in 0..10 {
                    if Self::effective_device(&state) == "metal" {
                        break;
                    }
                    sleep(Duration::from_millis(20)).await;
                }
                *slot = Some(state);
                return Ok(ModelHandle {
                    model_id: model.id.clone(),
                    provider_id: self.id().to_string(),
                });
            }
            match state.child.try_wait() {
                Ok(Some(exit)) => {
                    let tail = Self::log_tail(&state);
                    return Err(ProviderError::new(
                        AIError::ModelLoadFailed,
                        format!(
                            "llama-server exited during model load ({exit}){}",
                            if tail.is_empty() {
                                String::new()
                            } else {
                                format!("\n{tail}")
                            }
                        ),
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(ProviderError::new(
                        AIError::BackendCrashed,
                        format!("could not inspect llama-server: {error}"),
                    ));
                }
            }
            if Instant::now() >= deadline {
                let tail = Self::log_tail(&state);
                Self::stop_process(state).await;
                return Err(ProviderError::new(
                    AIError::ModelLoadFailed,
                    format!(
                        "llama-server did not become ready within 120 seconds{}",
                        if tail.is_empty() {
                            String::new()
                        } else {
                            format!("\n{tail}")
                        }
                    ),
                ));
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    async fn unload(&self, handle: &ModelHandle) -> Result<(), ProviderError> {
        let mut slot = self.process.lock().await;
        if let Some(state) = slot.take() {
            if state.model_id != handle.model_id {
                let resident = state.model_id.clone();
                *slot = Some(state);
                return Err(ProviderError::new(
                    AIError::InvalidRequest,
                    format!(
                        "model '{}' is not resident; llama.cpp currently holds '{}'",
                        handle.model_id, resident
                    ),
                ));
            }
            Self::stop_process(state).await;
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        let status = self.status().await;
        Ok(ProviderHealth {
            ok: status.available && (status.resident_models.is_empty() || status.ready),
            message: status.reason,
        })
    }

    async fn memory_usage_bytes(&self) -> Option<u64> {
        let process = self.process.lock().await;
        let pid = process.as_ref()?.child.id()?;
        resident_memory_bytes(pid)
    }
}

#[async_trait]
impl ChatProvider for LlamaCppProvider {
    async fn chat(&self, mut request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        request.stream = false;
        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url()))
            .json(&request)
            .send()
            .await
            .map_err(|error| {
                ProviderError::new(
                    AIError::BackendCrashed,
                    format!("cannot reach llama-server: {error}"),
                )
            })?;
        if !response.status().is_success() {
            return Err(Self::backend_error(response, AIError::InvalidRequest).await);
        }
        response.json::<ChatResponse>().await.map_err(|error| {
            ProviderError::new(
                AIError::BackendCrashed,
                format!("invalid llama-server chat response: {error}"),
            )
        })
    }

    async fn chat_stream(&self, mut request: ChatRequest) -> Result<ChatStream, ProviderError> {
        request.stream = true;
        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url()))
            .json(&request)
            .send()
            .await
            .map_err(|error| {
                ProviderError::new(
                    AIError::BackendCrashed,
                    format!("cannot reach llama-server: {error}"),
                )
            })?;
        if !response.status().is_success() {
            return Err(Self::backend_error(response, AIError::InvalidRequest).await);
        }

        let upstream = response.bytes_stream();
        let stream = async_stream::stream! {
            futures::pin_mut!(upstream);
            let mut buffer = Vec::new();
            while let Some(part) = upstream.next().await {
                match part {
                    Ok(bytes) => buffer.extend_from_slice(&bytes),
                    Err(error) => {
                        yield Err(ProviderError::new(
                            AIError::BackendCrashed,
                            format!("llama-server stream failed: {error}"),
                        ));
                        return;
                    }
                }

                while let Some(event) = take_sse_event(&mut buffer) {
                    match parse_sse_event(&event) {
                        Ok(ParsedEvent::Chunk(chunk)) => yield Ok(chunk),
                        Ok(ParsedEvent::Done) => return,
                        Ok(ParsedEvent::Skip) => {}
                        Err(error) => {
                            yield Err(error);
                            return;
                        }
                    }
                }
            }
            if buffer.iter().any(|byte| !byte.is_ascii_whitespace()) {
                yield Err(ProviderError::new(
                    AIError::BackendCrashed,
                    "llama-server ended with an incomplete SSE event",
                ));
            }
        };
        Ok(Box::pin(stream))
    }
}

enum ParsedEvent {
    Chunk(ChatChunk),
    Done,
    Skip,
}

fn take_sse_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    let (position, delimiter_len) = match (lf, crlf) {
        (Some(a), Some(b)) if a <= b => (a, 2),
        (Some(_), Some(b)) => (b, 4),
        (Some(a), None) => (a, 2),
        (None, Some(b)) => (b, 4),
        (None, None) => return None,
    };
    let event = buffer[..position].to_vec();
    buffer.drain(..position + delimiter_len);
    Some(event)
}

fn parse_sse_event(event: &[u8]) -> Result<ParsedEvent, ProviderError> {
    let text = std::str::from_utf8(event).map_err(|error| {
        ProviderError::new(
            AIError::BackendCrashed,
            format!("llama-server emitted non-UTF-8 SSE data: {error}"),
        )
    })?;
    let data = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return Ok(ParsedEvent::Skip);
    }
    if data == "[DONE]" {
        return Ok(ParsedEvent::Done);
    }
    serde_json::from_str::<ChatChunk>(&data)
        .map(ParsedEvent::Chunk)
        .map_err(|error| {
            ProviderError::new(
                AIError::BackendCrashed,
                format!("invalid llama-server SSE chunk: {error}"),
            )
        })
}

fn resolve_executable(value: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(value);
    if candidate.components().count() > 1 || candidate.is_absolute() {
        return is_executable_file(&candidate).then_some(candidate);
    }
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|directory| directory.join(value))
            .find(|path| is_executable_file(path))
    })
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_effective_device_from_worker_logs() {
        assert_eq!(
            LlamaCppProvider::detect_effective_device(
                "device_info: - MTL0 : Apple M1 Pro (10922 MiB free)"
            ),
            "metal"
        );
        assert_eq!(
            LlamaCppProvider::detect_effective_device(
                "ggml_metal_init: allocating\nload_tensors: offloaded 30/30 layers to GPU"
            ),
            "metal"
        );
        assert_eq!(
            LlamaCppProvider::detect_effective_device("load_tensors: CPU buffer size"),
            "cpu"
        );
    }

    #[test]
    fn splits_lf_and_crlf_sse_frames() {
        let mut buffer = b"data: one\n\ndata: two\r\n\r\npartial".to_vec();
        assert_eq!(take_sse_event(&mut buffer).unwrap(), b"data: one");
        assert_eq!(take_sse_event(&mut buffer).unwrap(), b"data: two");
        assert_eq!(buffer, b"partial");
    }

    #[test]
    fn parses_done_and_chat_chunks() {
        assert!(matches!(
            parse_sse_event(b"data: [DONE]").unwrap(),
            ParsedEvent::Done
        ));
        let event = br#"data: {"id":"x","object":"chat.completion.chunk","created":1,"model":"tiny","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":null}]}"#;
        match parse_sse_event(event).unwrap() {
            ParsedEvent::Chunk(chunk) => {
                assert_eq!(chunk.model, "tiny");
                assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hi"));
            }
            _ => panic!("expected chunk"),
        }
    }
}
