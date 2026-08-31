//! Qwen3-ASR STT Provider: one supervised persistent Python JSONL worker.

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

struct QwenState {
    model_id: String,
    worker: WorkerProcess,
}

#[derive(Clone)]
struct QwenResident {
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

struct QwenBackendConfig {
    id: &'static str,
    label: &'static str,
    python_env: &'static str,
    script_env: &'static str,
    device_env: &'static str,
    python: &'static str,
    script: &'static str,
    default_device: &'static str,
    allowed_devices: &'static [&'static str],
    supported_devices: &'static [&'static str],
    install_hint: &'static str,
}

const MLX_BACKEND: QwenBackendConfig = QwenBackendConfig {
    id: "qwen3-asr-mlx",
    label: "Qwen3-ASR MLX",
    python_env: "AIWORK_QWEN3_ASR_MLX_PYTHON",
    script_env: "AIWORK_QWEN3_ASR_MLX_SCRIPT",
    device_env: "AIWORK_QWEN3_ASR_MLX_DEVICE",
    python: ".build/qwen3-asr-mlx-venv/bin/python",
    script: "scripts/qwen3_asr_mlx_worker.py",
    default_device: "auto",
    allowed_devices: &["auto", "metal"],
    supported_devices: &["metal"],
    install_hint: "create .build/qwen3-asr-mlx-venv and install mlx-audio==0.5.0",
};

pub struct Qwen3AsrProvider {
    state: Mutex<Option<QwenState>>,
    resident: StdRwLock<Option<QwenResident>>,
    config: &'static QwenBackendConfig,
    python_override: Option<PathBuf>,
    script_override: Option<PathBuf>,
}

impl Qwen3AsrProvider {
    pub fn mlx_from_env() -> Self {
        Self::with_config(&MLX_BACKEND)
    }

    fn with_config(config: &'static QwenBackendConfig) -> Self {
        Self {
            state: Mutex::new(None),
            resident: StdRwLock::new(None),
            config,
            python_override: None,
            script_override: None,
        }
    }

    fn resolve_python(&self) -> Option<PathBuf> {
        if let Some(path) = &self.python_override {
            return path.is_file().then(|| path.clone());
        }
        if let Ok(value) = env::var(self.config.python_env) {
            let candidate = PathBuf::from(value.trim());
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        resolve_project_file(Path::new(self.config.python))
    }

    fn resolve_script(&self) -> Option<PathBuf> {
        if let Some(path) = &self.script_override {
            return path.is_file().then(|| path.clone());
        }
        if let Ok(value) = env::var(self.config.script_env) {
            let candidate = PathBuf::from(value.trim());
            if candidate.is_file() {
                return Some(candidate.canonicalize().unwrap_or(candidate));
            }
        }
        resolve_project_file(Path::new(self.config.script))
            .map(|path| path.canonicalize().unwrap_or(path))
    }

    fn resident_snapshot(&self) -> Option<QwenResident> {
        self.resident
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_resident(&self, resident: Option<QwenResident>) {
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
            "AIWORK_QWEN3_ASR_LOAD_TIMEOUT_SECS",
            DEFAULT_LOAD_TIMEOUT_SECS,
        )
    }

    fn inference_timeout() -> Duration {
        env_duration(
            "AIWORK_QWEN3_ASR_INFERENCE_TIMEOUT_SECS",
            DEFAULT_INFERENCE_TIMEOUT_SECS,
        )
    }

    async fn terminate_locked(&self, guard: &mut Option<QwenState>) {
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
impl Provider for Qwen3AsrProvider {
    fn id(&self) -> &'static str {
        self.config.id
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::SpeechToText]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id().to_string(),
            capabilities: self.capabilities(),
            isolation: IsolationMode::Worker,
            supported_devices: self
                .config
                .supported_devices
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
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
            Some(format!(
                "{} Python environment was not found",
                self.config.label
            ))
        } else if !script_found {
            Some(format!("{} was not found", self.config.script))
        } else if ready {
            effective_device
                .as_ref()
                .map(|device| format!("{} worker is ready on {device}", self.config.label))
        } else {
            Some(format!("no {} model is loaded", self.config.label))
        };
        ProviderStatus {
            available: python_found && script_found,
            ready,
            effective_device,
            resident_models,
            reason,
            install_hint: (!python_found).then(|| self.config.install_hint.to_string()),
        }
    }

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, ProviderError> {
        let python = self.resolve_python().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                format!("{} Python environment was not found", self.config.label),
            )
        })?;
        let script = self.resolve_script().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                format!("{} was not found", self.config.script),
            )
        })?;
        let model_path = model.path.as_deref().ok_or_else(|| {
            ProviderError::new(
                AIError::InvalidRequest,
                format!("Qwen3-ASR model '{}' has no local model path", model.id),
            )
        })?;
        let model_dir = Path::new(model_path).canonicalize().map_err(|_| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("Qwen3-ASR model directory for '{}' was not found", model.id),
            )
        })?;
        validate_mlx_8bit_model_dir(&model_dir)?;

        let device = env::var(self.config.device_env)
            .unwrap_or_else(|_| self.config.default_device.to_string());
        if !self.config.allowed_devices.contains(&device.as_str()) {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                format!(
                    "{} must be one of: {}",
                    self.config.device_env,
                    self.config.allowed_devices.join(", ")
                ),
            ));
        }
        let mut child = Command::new(&python)
            .arg(&script)
            .arg("--model")
            .arg(&model_dir)
            .arg("--device")
            .arg(&device)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                ProviderError::new(
                    AIError::ProviderUnavailable,
                    format!("failed to start Qwen3-ASR worker: {error}"),
                )
            })?;
        let pid = child.id();
        let stdin = child.stdin.take().ok_or_else(|| {
            ProviderError::new(AIError::Internal, "Qwen3-ASR worker stdin unavailable")
        })?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            ProviderError::new(AIError::Internal, "Qwen3-ASR worker stdout unavailable")
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
                    "Qwen3-ASR worker exited before reporting readiness",
                ));
            }
            Ok(Err(error)) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::ModelLoadFailed,
                    format!("failed to read Qwen3-ASR readiness: {error}"),
                ));
            }
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::Timeout,
                    format!(
                        "Qwen3-ASR model load timed out after {} seconds",
                        Self::load_timeout().as_secs()
                    ),
                ));
            }
        }
        let reply: ReadyReply = serde_json::from_str(line.trim()).map_err(|error| {
            ProviderError::new(
                AIError::ModelLoadFailed,
                format!("invalid Qwen3-ASR readiness response: {error}"),
            )
        })?;
        if !reply.ready {
            let message = reply
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "Qwen3-ASR model could not be loaded".to_string());
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(ProviderError::new(AIError::ModelLoadFailed, message));
        }
        let effective_device = reply.device.unwrap_or_else(|| "cpu".to_string());
        *self.state.lock().await = Some(QwenState {
            model_id: model.id.clone(),
            worker: WorkerProcess {
                child,
                stdin,
                stdout,
            },
        });
        self.set_resident(Some(QwenResident {
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
                        "model '{}' is not resident; Qwen3-ASR currently holds '{}'",
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
impl STTProvider for Qwen3AsrProvider {
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
        let audio = Path::new(audio).canonicalize().map_err(|_| {
            ProviderError::new(AIError::InvalidRequest, "transcription audio was not found")
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
            ProviderError::new(AIError::ModelLoadFailed, "Qwen3-ASR model is not loaded")
        })?;
        if state.model_id != request.model {
            return Err(ProviderError::new(
                AIError::ModelNotFound,
                format!("Qwen3-ASR model '{}' is not loaded", request.model),
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
                format!("Qwen3-ASR worker is unreachable: {error}"),
            ));
        }
        if let Err(error) = state.worker.stdin.flush().await {
            self.terminate_locked(&mut guard).await;
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                format!("Qwen3-ASR worker flush failed: {error}"),
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
                    "Qwen3-ASR worker exited unexpectedly",
                ));
            }
            Ok(Err(error)) => {
                self.terminate_locked(&mut guard).await;
                return Err(ProviderError::new(
                    AIError::BackendCrashed,
                    format!("Qwen3-ASR worker read failed: {error}"),
                ));
            }
            Err(_) => {
                self.terminate_locked(&mut guard).await;
                return Err(ProviderError::new(
                    AIError::Timeout,
                    format!(
                        "Qwen3-ASR inference timed out after {} seconds; worker was restarted",
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
            format!("invalid Qwen3-ASR worker reply: {error}"),
        ),
        fatal: true,
    })?;
    if reply.id != Some(expected_id) {
        return Err(ReplyFailure {
            error: ProviderError::new(
                AIError::BackendCrashed,
                format!(
                    "Qwen3-ASR worker response id mismatch: expected {expected_id}, got {:?}",
                    reply.id
                ),
            ),
            fatal: true,
        });
    }
    if !reply.ok {
        let worker_error = reply.error.unwrap_or(WorkerError {
            code: "inference_error".to_string(),
            message: "Qwen3-ASR inference failed".to_string(),
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
                "Qwen3-ASR returned an empty transcription",
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
    let language = match normalized.as_str() {
        "zh" | "zh-cn" | "zh-tw" | "chinese" => "Chinese",
        "en" | "english" => "English",
        "yue" | "cantonese" => "Cantonese",
        "ar" | "arabic" => "Arabic",
        "de" | "german" => "German",
        "fr" | "french" => "French",
        "es" | "spanish" => "Spanish",
        "pt" | "portuguese" => "Portuguese",
        "id" | "indonesian" => "Indonesian",
        "it" | "italian" => "Italian",
        "ko" | "korean" => "Korean",
        "ru" | "russian" => "Russian",
        "th" | "thai" => "Thai",
        "vi" | "vietnamese" => "Vietnamese",
        "ja" | "japanese" => "Japanese",
        "tr" | "turkish" => "Turkish",
        "hi" | "hindi" => "Hindi",
        "ms" | "malay" => "Malay",
        "nl" | "dutch" => "Dutch",
        "sv" | "swedish" => "Swedish",
        "da" | "danish" => "Danish",
        "fi" | "finnish" => "Finnish",
        "pl" | "polish" => "Polish",
        "cs" | "czech" => "Czech",
        "fil" | "tl" | "filipino" => "Filipino",
        "fa" | "persian" => "Persian",
        "el" | "greek" => "Greek",
        "hu" | "hungarian" => "Hungarian",
        "mk" | "macedonian" => "Macedonian",
        "ro" | "romanian" => "Romanian",
        _ => {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                format!("language '{value}' is not supported by Qwen3-ASR"),
            ))
        }
    };
    Ok(language.to_string())
}

pub(crate) fn validate_mlx_8bit_model_dir(path: &Path) -> Result<(), ProviderError> {
    validate_model_dir(path)?;
    let config = std::fs::read_to_string(path.join("config.json")).map_err(|error| {
        ProviderError::new(
            AIError::InvalidRequest,
            format!("failed to read Qwen3-ASR MLX config.json: {error}"),
        )
    })?;
    let config: serde_json::Value = serde_json::from_str(&config).map_err(|error| {
        ProviderError::new(
            AIError::InvalidRequest,
            format!("invalid Qwen3-ASR MLX config.json: {error}"),
        )
    })?;
    let bits = config
        .get("quantization")
        .or_else(|| config.get("quantization_config"))
        .and_then(|value| value.get("bits"))
        .and_then(serde_json::Value::as_u64);
    if bits != Some(8) {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "qwen3-asr-mlx requires an MLX 8-bit model directory",
        ));
    }
    Ok(())
}

pub(crate) fn validate_model_dir(path: &Path) -> Result<(), ProviderError> {
    if !path.is_dir() {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "Qwen3-ASR requires a complete model directory",
        ));
    }
    for required in [
        "config.json",
        "model.safetensors",
        "preprocessor_config.json",
    ] {
        if !path.join(required).is_file() {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                format!("Qwen3-ASR model directory is missing {required}"),
            ));
        }
    }
    let has_tokenizer = [
        "tokenizer.json",
        "tokenizer_config.json",
        "vocab.json",
        "merges.txt",
    ]
    .iter()
    .any(|name| path.join(name).is_file());
    if !has_tokenizer {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "Qwen3-ASR model directory is missing tokenizer files",
        ));
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

    struct TempModelDir(PathBuf);

    impl TempModelDir {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = env::temp_dir().join(format!("macai-qwen3-asr-test-{unique}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn touch(&self, name: &str) {
            fs::write(self.0.join(name), b"test").unwrap();
        }
    }

    impl Drop for TempModelDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn validates_complete_model_directory() {
        let dir = TempModelDir::new();
        for name in [
            "config.json",
            "model.safetensors",
            "preprocessor_config.json",
            "tokenizer_config.json",
        ] {
            dir.touch(name);
        }
        validate_model_dir(&dir.0).unwrap();

        fs::remove_file(dir.0.join("config.json")).unwrap();
        let error = validate_model_dir(&dir.0).unwrap_err();
        assert_eq!(error.kind, AIError::InvalidRequest);
        assert!(error.message.contains("config.json"));
    }

    #[test]
    fn validates_mlx_8bit_model_directory() {
        let dir = TempModelDir::new();
        fs::write(
            dir.0.join("config.json"),
            br#"{"quantization":{"group_size":64,"bits":8,"mode":"affine"}}"#,
        )
        .unwrap();
        for name in [
            "model.safetensors",
            "preprocessor_config.json",
            "tokenizer_config.json",
        ] {
            dir.touch(name);
        }
        validate_mlx_8bit_model_dir(&dir.0).unwrap();

        fs::write(
            dir.0.join("config.json"),
            br#"{"quantization":{"group_size":64,"bits":4,"mode":"affine"}}"#,
        )
        .unwrap();
        let error = validate_mlx_8bit_model_dir(&dir.0).unwrap_err();
        assert_eq!(error.kind, AIError::InvalidRequest);
        assert!(error.message.contains("8-bit"));
    }

    #[test]
    fn maps_language_codes_and_rejects_unknown_values() {
        assert_eq!(canonical_language("zh").unwrap(), "Chinese");
        assert_eq!(canonical_language("EN").unwrap(), "English");
        assert_eq!(canonical_language("Filipino").unwrap(), "Filipino");
        assert_eq!(
            canonical_language("xx").unwrap_err().kind,
            AIError::InvalidRequest
        );
    }

    #[test]
    fn decodes_formatted_worker_output() {
        let response = decode_worker_reply(
            7,
            r#"{"id":7,"ok":true,"text":" 你好，世界。 ","language":"Chinese","device":"mps"}"#,
            "qwen3-asr-0.6b".to_string(),
        )
        .unwrap();
        assert_eq!(response.text, "你好，世界。");
        assert_eq!(response.language.as_deref(), Some("Chinese"));
        assert_eq!(response.model, "qwen3-asr-0.6b");
    }

    #[test]
    fn maps_worker_validation_errors_without_marking_protocol_fatal() {
        let failure = decode_worker_reply(
            9,
            r#"{"id":9,"ok":false,"error":{"code":"invalid_audio","message":"audio cannot be decoded"}}"#,
            "qwen3-asr-0.6b".to_string(),
        )
        .unwrap_err();
        assert_eq!(failure.error.kind, AIError::InvalidRequest);
        assert!(!failure.fatal);
    }

    #[test]
    fn rejects_mismatched_response_ids_as_fatal() {
        let failure = decode_worker_reply(
            11,
            r#"{"id":12,"ok":true,"text":"hello","language":"English"}"#,
            "qwen3-asr-0.6b".to_string(),
        )
        .unwrap_err();
        assert_eq!(failure.error.kind, AIError::BackendCrashed);
        assert!(failure.fatal);
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
        let dir = TempModelDir::new();
        for name in [
            "model.safetensors",
            "preprocessor_config.json",
            "tokenizer_config.json",
        ] {
            dir.touch(name);
        }
        fs::write(
            dir.0.join("config.json"),
            r#"{"quantization": {"bits": 8}}"#,
        )
        .unwrap();
        let script = dir.0.join("fake_worker.py");
        fs::write(
            &script,
            r#"import json, sys
print(json.dumps({"ready": True, "device": "cpu"}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    print(json.dumps({"id": request["id"], "ok": True, "text": "  test transcript  ", "language": "English", "device": "cpu"}), flush=True)
"#,
        )
        .unwrap();
        let audio = dir.0.join("input.wav");
        fs::write(&audio, b"RIFF-test-WAVE").unwrap();
        let provider = Qwen3AsrProvider {
            state: Mutex::new(None),
            resident: StdRwLock::new(None),
            config: &MLX_BACKEND,
            python_override: Some(python),
            script_override: Some(script),
        };
        let model = ModelSpec {
            id: "qwen-test".to_string(),
            name: "Qwen test".to_string(),
            model_type: "stt".to_string(),
            provider: "qwen3-asr-mlx".to_string(),
            source: None,
            path: Some(dir.0.to_string_lossy().into_owned()),
            format: Some("qwen3-asr-mlx".to_string()),
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
                language: Some("en".to_string()),
                response_format: Some("json".to_string()),
            })
            .await
            .unwrap();
        assert_eq!(response.text, "test transcript");
        assert_eq!(response.language.as_deref(), Some("English"));
        assert_eq!(response.model, "qwen-test");
        assert_eq!(provider.effective_device().await.as_deref(), Some("cpu"));
        provider.unload(&handle).await.unwrap();
        assert!(!provider.status().await.ready);
    }

    #[tokio::test]
    async fn observability_does_not_wait_for_active_transcription_lock() {
        let provider = Qwen3AsrProvider::mlx_from_env();
        provider.set_resident(Some(QwenResident {
            model_id: "qwen-test".to_string(),
            pid: Some(std::process::id()),
            device: "cpu".to_string(),
        }));
        let _active = provider.state.lock().await;

        let status = timeout(Duration::from_secs(1), provider.status())
            .await
            .expect("status must not wait for transcription");
        assert!(status.ready);
        assert_eq!(status.resident_models, ["qwen-test"]);
        assert_eq!(provider.effective_device().await.as_deref(), Some("cpu"));
    }
}
