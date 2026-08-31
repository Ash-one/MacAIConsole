//! sherpa-onnx STT Provider：隔离执行常驻 Python worker。
//!
//! 默认模型是 sherpa-onnx 的 streaming Zipformer 中文 int8 模型
//! `sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`。模型权重仍由
//! 用户放在本地目录，worker 只通过 JSONL 接收音频路径并返回转写结果。

use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock as StdRwLock;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

use ai_core::model::ModelSpec;
use ai_core::provider::{
    Capability, IsolationMode, ModelHandle, Provider, ProviderDescriptor, ProviderError,
    ProviderHealth, ProviderStatus, STTProvider,
};
use ai_core::request::TranscriptionRequest;
use ai_core::response::TranscriptionResponse;
use ai_core::AIError;

use crate::process_memory::resident_memory_bytes;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
const DEFAULT_LOAD_TIMEOUT_SECS: u64 = 600;
const DEFAULT_INFERENCE_TIMEOUT_SECS: u64 = 1_200;
const DEFAULT_NUM_THREADS: u32 = 4;

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

struct SherpaState {
    model_id: String,
    worker: WorkerProcess,
}

#[derive(Clone)]
struct SherpaResident {
    model_id: String,
    pid: Option<u32>,
    device: String,
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
struct WorkerReply {
    id: Option<u64>,
    ok: bool,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    error: Option<WorkerError>,
}

#[derive(Debug)]
struct ReplyFailure {
    error: ProviderError,
    fatal: bool,
}

pub struct SherpaOnnxProvider {
    state: Mutex<Option<SherpaState>>,
    resident: StdRwLock<Option<SherpaResident>>,
    python_override: Option<PathBuf>,
    script_override: Option<PathBuf>,
}

impl SherpaOnnxProvider {
    pub fn from_env() -> Self {
        Self {
            state: Mutex::new(None),
            resident: StdRwLock::new(None),
            python_override: None,
            script_override: None,
        }
    }

    fn resolve_python(&self) -> Option<PathBuf> {
        if let Some(path) = &self.python_override {
            return path.is_file().then(|| path.clone());
        }
        if let Ok(value) = env::var("AIWORK_SHERPA_ONNX_PYTHON") {
            let candidate = PathBuf::from(value.trim());
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        resolve_project_file(Path::new(".build/sherpa-onnx-venv/bin/python"))
    }

    fn resolve_script(&self) -> Option<PathBuf> {
        if let Some(path) = &self.script_override {
            return path.is_file().then(|| path.clone());
        }
        if let Ok(value) = env::var("AIWORK_SHERPA_ONNX_SCRIPT") {
            let candidate = PathBuf::from(value.trim());
            if candidate.is_file() {
                return Some(candidate.canonicalize().unwrap_or(candidate));
            }
        }
        resolve_project_file(Path::new("scripts/sherpa_onnx_worker.py"))
            .map(|path| path.canonicalize().unwrap_or(path))
    }

    fn resident_snapshot(&self) -> Option<SherpaResident> {
        self.resident
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_resident(&self, resident: Option<SherpaResident>) {
        *self
            .resident
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = resident;
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
            self.set_resident(None);
        }
        ready
    }

    fn load_timeout() -> Duration {
        env_duration(
            "AIWORK_SHERPA_ONNX_LOAD_TIMEOUT_SECS",
            DEFAULT_LOAD_TIMEOUT_SECS,
        )
    }

    fn inference_timeout() -> Duration {
        env_duration(
            "AIWORK_SHERPA_ONNX_INFERENCE_TIMEOUT_SECS",
            DEFAULT_INFERENCE_TIMEOUT_SECS,
        )
    }

    fn device() -> Result<String, ProviderError> {
        let value = env::var("AIWORK_SHERPA_ONNX_DEVICE")
            .unwrap_or_else(|_| "cpu".to_string())
            .trim()
            .to_ascii_lowercase();
        let device = match value.as_str() {
            // auto intentionally resolves to CPU: this is the portable default and
            // avoids silently changing execution backends on different sherpa wheels.
            "auto" | "cpu" => "cpu",
            "coreml" => "coreml",
            _ => {
                return Err(ProviderError::new(
                    AIError::InvalidRequest,
                    "AIWORK_SHERPA_ONNX_DEVICE must be one of: auto, cpu, coreml",
                ))
            }
        };
        Ok(device.to_string())
    }

    async fn terminate_locked(&self, guard: &mut Option<SherpaState>) {
        if let Some(state) = guard.take() {
            self.set_resident(None);
            let mut worker = state.worker;
            let _ = worker.stdin.shutdown().await;
            if timeout(Duration::from_secs(5), worker.child.wait())
                .await
                .is_err()
            {
                let _ = worker.child.start_kill();
                let _ = worker.child.wait().await;
            }
        }
    }
}

#[async_trait]
impl Provider for SherpaOnnxProvider {
    fn id(&self) -> &'static str {
        "sherpa-onnx"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::SpeechToText]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id().to_string(),
            capabilities: self.capabilities(),
            isolation: IsolationMode::Worker,
            supported_devices: vec!["cpu".to_string(), "coreml".to_string()],
        }
    }

    async fn status(&self) -> ProviderStatus {
        let ready = self.probe_ready();
        let python_found = self.resolve_python().is_some();
        let script_found = self.resolve_script().is_some();
        let resident = self.resident_snapshot();
        let effective_device = ready
            .then(|| resident.as_ref().map(|value| value.device.clone()))
            .flatten();
        let resident_models = resident
            .map(|value| vec![value.model_id])
            .unwrap_or_default();
        let reason = if !python_found {
            Some("sherpa-onnx Python environment was not found".to_string())
        } else if !script_found {
            Some("scripts/sherpa_onnx_worker.py was not found".to_string())
        } else if ready {
            effective_device
                .as_ref()
                .map(|device| format!("sherpa-onnx worker is ready on {device}"))
        } else {
            Some("no sherpa-onnx model is loaded".to_string())
        };
        ProviderStatus {
            available: python_found && script_found,
            ready,
            effective_device,
            resident_models,
            reason,
            install_hint: (!python_found).then(|| {
                "run scripts/setup-sherpa-onnx.sh or set AIWORK_SHERPA_ONNX_PYTHON".to_string()
            }),
        }
    }

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, ProviderError> {
        let python = self.resolve_python().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "sherpa-onnx Python environment was not found; run scripts/setup-sherpa-onnx.sh",
            )
        })?;
        let script = self.resolve_script().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "scripts/sherpa_onnx_worker.py was not found",
            )
        })?;
        let model_path = model.path.as_deref().ok_or_else(|| {
            ProviderError::new(
                AIError::InvalidRequest,
                format!("sherpa-onnx model '{}' has no local model path", model.id),
            )
        })?;
        let model_dir = Path::new(model_path).canonicalize().map_err(|error| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("cannot open sherpa-onnx model directory '{model_path}': {error}"),
            )
        })?;
        validate_model_dir(&model_dir)?;
        let device = Self::device()?;
        let num_threads = env::var("AIWORK_SHERPA_ONNX_THREADS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_NUM_THREADS);

        let mut child = Command::new(&python)
            .arg(&script)
            .arg("--model-dir")
            .arg(&model_dir)
            .arg("--device")
            .arg(&device)
            .arg("--num-threads")
            .arg(num_threads.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                ProviderError::new(
                    AIError::ProviderUnavailable,
                    format!("failed to start sherpa-onnx worker: {error}"),
                )
            })?;
        let pid = child.id();
        let stdin = child.stdin.take().ok_or_else(|| {
            ProviderError::new(AIError::Internal, "sherpa-onnx worker stdin unavailable")
        })?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            ProviderError::new(AIError::Internal, "sherpa-onnx worker stdout unavailable")
        })?;

        let mut line = String::new();
        let readiness = timeout(Self::load_timeout(), async {
            BufReader::new(&mut stdout).read_line(&mut line).await
        })
        .await;
        match readiness {
            Ok(Ok(count)) if count > 0 => {}
            Ok(Ok(_)) => {
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::ModelLoadFailed,
                    "sherpa-onnx worker exited before reporting readiness",
                ));
            }
            Ok(Err(error)) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::ModelLoadFailed,
                    format!("failed to read sherpa-onnx readiness: {error}"),
                ));
            }
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::Timeout,
                    format!(
                        "sherpa-onnx model load timed out after {} seconds",
                        Self::load_timeout().as_secs()
                    ),
                ));
            }
        }
        let reply: ReadyReply = serde_json::from_str(line.trim()).map_err(|error| {
            ProviderError::new(
                AIError::ModelLoadFailed,
                format!("invalid sherpa-onnx readiness response: {error}"),
            )
        })?;
        if !reply.ready {
            let message = reply
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "sherpa-onnx model could not be loaded".to_string());
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(ProviderError::new(AIError::ModelLoadFailed, message));
        }
        let effective_device = reply.device.unwrap_or(device);
        *self.state.lock().await = Some(SherpaState {
            model_id: model.id.clone(),
            worker: WorkerProcess {
                child,
                stdin,
                stdout,
            },
        });
        self.set_resident(Some(SherpaResident {
            model_id: model.id.clone(),
            pid,
            device: effective_device,
        }));
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
                        "model '{}' is not resident; sherpa-onnx currently holds '{}'",
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
impl STTProvider for SherpaOnnxProvider {
    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        let language = request
            .language
            .as_deref()
            .map(canonical_language)
            .transpose()?;
        let audio = request.file.as_deref().ok_or_else(|| {
            ProviderError::new(AIError::InvalidRequest, "transcription file is required")
        })?;
        let audio = Path::new(audio).canonicalize().map_err(|error| {
            ProviderError::new(
                AIError::InvalidRequest,
                format!("cannot open transcription audio '{audio}': {error}"),
            )
        })?;
        if audio.metadata().map(|value| value.len()).unwrap_or(0) == 0 {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                "transcription audio is empty",
            ));
        }

        let request_id = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let payload = serde_json::json!({
            "id": request_id,
            "audio": audio,
            "language": language,
        });
        let mut guard = self.state.lock().await;
        let state = guard.as_mut().ok_or_else(|| {
            ProviderError::new(AIError::ModelLoadFailed, "sherpa-onnx model is not loaded")
        })?;
        if state.model_id != request.model {
            return Err(ProviderError::new(
                AIError::ModelNotFound,
                format!("sherpa-onnx model '{}' is not loaded", request.model),
            ));
        }
        if let Err(error) = state
            .worker
            .stdin
            .write_all(format!("{payload}\n").as_bytes())
            .await
        {
            self.terminate_locked(&mut guard).await;
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                format!("sherpa-onnx worker is unreachable: {error}"),
            ));
        }
        if let Err(error) = state.worker.stdin.flush().await {
            self.terminate_locked(&mut guard).await;
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                format!("sherpa-onnx worker flush failed: {error}"),
            ));
        }

        let mut line = String::new();
        let read_result = timeout(Self::inference_timeout(), async {
            BufReader::new(&mut state.worker.stdout)
                .read_line(&mut line)
                .await
        })
        .await;
        match read_result {
            Ok(Ok(count)) if count > 0 => {}
            Ok(Ok(_)) => {
                self.terminate_locked(&mut guard).await;
                return Err(ProviderError::new(
                    AIError::BackendCrashed,
                    "sherpa-onnx worker exited unexpectedly",
                ));
            }
            Ok(Err(error)) => {
                self.terminate_locked(&mut guard).await;
                return Err(ProviderError::new(
                    AIError::BackendCrashed,
                    format!("sherpa-onnx worker read failed: {error}"),
                ));
            }
            Err(_) => {
                self.terminate_locked(&mut guard).await;
                return Err(ProviderError::new(
                    AIError::Timeout,
                    format!(
                        "sherpa-onnx inference timed out after {} seconds; worker was restarted",
                        Self::inference_timeout().as_secs()
                    ),
                ));
            }
        }

        match decode_worker_reply(request_id, &line, request.model) {
            Ok(response) => Ok(response),
            Err(failure) => {
                if failure.fatal {
                    self.terminate_locked(&mut guard).await;
                }
                Err(failure.error)
            }
        }
    }
}

fn decode_worker_reply(
    expected_id: u64,
    line: &str,
    model: String,
) -> Result<TranscriptionResponse, ReplyFailure> {
    let reply: WorkerReply = serde_json::from_str(line.trim()).map_err(|error| ReplyFailure {
        error: ProviderError::new(
            AIError::BackendCrashed,
            format!("invalid sherpa-onnx worker reply: {error}"),
        ),
        fatal: true,
    })?;
    if reply.id != Some(expected_id) {
        return Err(ReplyFailure {
            error: ProviderError::new(
                AIError::BackendCrashed,
                format!(
                    "sherpa-onnx worker response id mismatch: expected {expected_id}, got {:?}",
                    reply.id
                ),
            ),
            fatal: true,
        });
    }
    if !reply.ok {
        let worker_error = reply.error.unwrap_or(WorkerError {
            code: "inference_error".to_string(),
            message: "sherpa-onnx inference failed".to_string(),
        });
        return Err(ReplyFailure {
            error: ProviderError::new(worker_error_kind(&worker_error.code), worker_error.message),
            fatal: false,
        });
    }
    let text = reply.text.unwrap_or_default().trim().to_string();
    if text.is_empty() {
        return Err(ReplyFailure {
            error: ProviderError::new(
                AIError::BackendCrashed,
                "sherpa-onnx returned an empty transcription",
            ),
            fatal: true,
        });
    }
    let language = reply
        .language
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let _device = reply.device;
    Ok(TranscriptionResponse {
        text,
        language,
        model,
    })
}

fn worker_error_kind(code: &str) -> AIError {
    match code {
        "invalid_audio" | "invalid_language" | "invalid_request" => AIError::InvalidRequest,
        "model_load_failed" => AIError::ModelLoadFailed,
        _ => AIError::BackendCrashed,
    }
}

fn canonical_language(value: &str) -> Result<String, ProviderError> {
    let value = value.trim();
    let normalized = value.to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "zh" | "zh-cn" | "zh-hans" | "chinese" | "中文" => Ok("Chinese".to_string()),
        _ => Err(ProviderError::new(
            AIError::InvalidRequest,
            format!("language '{value}' is not supported by sherpa-onnx zh-int8-2025"),
        )),
    }
}

/// Validate the official zh-int8-2025 streaming Zipformer model directory.
pub(crate) fn validate_model_dir(path: &Path) -> Result<(), ProviderError> {
    if !path.is_dir() {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "sherpa-onnx requires a complete model directory",
        ));
    }
    for required in [
        "tokens.txt",
        "encoder.int8.onnx",
        "decoder.onnx",
        "joiner.int8.onnx",
    ] {
        if !path.join(required).is_file() {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                format!("sherpa-onnx model directory is missing {required}"),
            ));
        }
    }
    Ok(())
}

fn resolve_project_file(relative: &Path) -> Option<PathBuf> {
    [".", "..", "../.."]
        .into_iter()
        .map(|base| PathBuf::from(base).join(relative))
        .find(|candidate| candidate.is_file())
}

fn env_duration(name: &str, default_secs: u64) -> Duration {
    let seconds = env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default_secs);
    Duration::from_secs(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn validates_official_model_file_layout() {
        let path = env::temp_dir().join(format!(
            "macai-sherpa-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        for name in [
            "tokens.txt",
            "encoder.int8.onnx",
            "decoder.onnx",
            "joiner.int8.onnx",
        ] {
            fs::write(path.join(name), b"test").unwrap();
        }
        validate_model_dir(&path).unwrap();
        fs::remove_file(path.join("tokens.txt")).unwrap();
        let error = validate_model_dir(&path).unwrap_err();
        assert_eq!(error.kind, AIError::InvalidRequest);
        assert!(error.message.contains("tokens.txt"));
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn accepts_only_the_chinese_model_language() {
        assert_eq!(canonical_language("zh-CN").unwrap(), "Chinese");
        assert_eq!(canonical_language("Chinese").unwrap(), "Chinese");
        assert_eq!(
            canonical_language("en").unwrap_err().kind,
            AIError::InvalidRequest
        );
    }

    #[test]
    fn decodes_worker_success_and_validation_failure() {
        let response = decode_worker_reply(
            7,
            r#"{"id":7,"ok":true,"text":" 你好，世界。 ","language":"Chinese","device":"cpu"}"#,
            "zh-int8-2025".to_string(),
        )
        .unwrap();
        assert_eq!(response.text, "你好，世界。");
        assert_eq!(response.language.as_deref(), Some("Chinese"));

        let failure = decode_worker_reply(
            8,
            r#"{"id":8,"ok":false,"error":{"code":"invalid_audio","message":"audio cannot be decoded"}}"#,
            "zh-int8-2025".to_string(),
        )
        .unwrap_err();
        assert_eq!(failure.error.kind, AIError::InvalidRequest);
        assert!(!failure.fatal);
    }

    #[tokio::test]
    async fn provider_round_trip_formats_fake_worker_inference() {
        let Some(python) = [
            "/usr/bin/python3",
            "/opt/homebrew/bin/python3",
            "/usr/local/bin/python3",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file()) else {
            return;
        };
        let path = env::temp_dir().join(format!(
            "macai-sherpa-provider-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        for name in [
            "tokens.txt",
            "encoder.int8.onnx",
            "decoder.onnx",
            "joiner.int8.onnx",
        ] {
            fs::write(path.join(name), b"fixture").unwrap();
        }
        let script = path.join("fake_worker.py");
        fs::write(
            &script,
            r#"import json, sys
print(json.dumps({"ready": True, "device": "cpu"}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    print(json.dumps({"id": request["id"], "ok": True, "text": "  test transcript  ", "language": "Chinese", "device": "cpu"}), flush=True)
"#,
        )
        .unwrap();
        let audio = path.join("input.wav");
        fs::write(&audio, b"RIFF-test-WAVE").unwrap();
        let provider = SherpaOnnxProvider {
            state: Mutex::new(None),
            resident: StdRwLock::new(None),
            python_override: Some(python),
            script_override: Some(script),
        };
        let model = ModelSpec {
            id: "zh-int8-2025-test".to_string(),
            name: "Sherpa test".to_string(),
            model_type: "stt".to_string(),
            provider: "sherpa-onnx".to_string(),
            source: None,
            path: Some(path.to_string_lossy().into_owned()),
            format: Some("sherpa-onnx-zh-int8-2025".to_string()),
            size_bytes: None,
            memory_estimate: None,
            keep_alive: Some("always".to_string()),
            context_length: None,
            default_voice: None,
        };

        let handle = provider.load(&model).await.unwrap();
        let response = provider
            .transcribe(TranscriptionRequest {
                model: model.id.clone(),
                file: Some(audio.to_string_lossy().into_owned()),
                language: Some("zh-CN".to_string()),
                response_format: Some("json".to_string()),
            })
            .await
            .unwrap();
        assert_eq!(response.text, "test transcript");
        assert_eq!(response.language.as_deref(), Some("Chinese"));
        assert_eq!(response.model, model.id);
        assert_eq!(provider.effective_device().await.as_deref(), Some("cpu"));

        provider.unload(&handle).await.unwrap();
        assert!(!provider.status().await.ready);
        let _ = fs::remove_dir_all(path);
    }
}
