//! whisper.cpp STT Provider：隔离执行 `whisper-cli` 完成本地离线转写。

use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;
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

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct WhisperState {
    model_id: String,
    model_path: PathBuf,
    effective_device: Option<String>,
}

pub struct WhisperCppProvider {
    state: Mutex<Option<WhisperState>>,
}

impl WhisperCppProvider {
    pub fn from_env() -> Self {
        Self {
            state: Mutex::new(None),
        }
    }

    fn resolve_binary() -> Option<PathBuf> {
        if let Ok(value) = env::var("AIWORK_WHISPER_CLI") {
            let value = value.trim();
            if !value.is_empty() {
                return resolve_executable(value);
            }
        }
        if let Ok(cwd) = env::current_dir() {
            let development = cwd.join(".build/whisper.cpp/bin/whisper-cli");
            if development.is_file() {
                return Some(development);
            }
        }
        resolve_executable("whisper-cli")
    }

    fn detect_device(stderr: &str) -> String {
        let logs = stderr.to_ascii_lowercase();
        if logs.contains("metal = 1") || logs.contains("metal: true") || logs.contains("ggml_metal")
        {
            "metal".to_string()
        } else {
            "cpu".to_string()
        }
    }
}

#[async_trait]
impl Provider for WhisperCppProvider {
    fn id(&self) -> &'static str {
        "whisper.cpp"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::SpeechToText]
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
        let binary = Self::resolve_binary();
        let state = self.state.lock().await.clone();
        let model_ready = state
            .as_ref()
            .is_some_and(|loaded| loaded.model_path.is_file());
        ProviderStatus {
            available: binary.is_some(),
            ready: binary.is_some() && model_ready,
            effective_device: state
                .as_ref()
                .and_then(|loaded| loaded.effective_device.clone()),
            resident_models: state
                .as_ref()
                .map(|loaded| vec![loaded.model_id.clone()])
                .unwrap_or_default(),
            reason: match (binary, state) {
                (None, _) => Some("whisper-cli executable was not found".to_string()),
                (Some(_), None) => Some("no Whisper model is loaded".to_string()),
                (Some(_), Some(loaded)) if !loaded.model_path.is_file() => Some(format!(
                    "Whisper model was not found: {}",
                    loaded.model_path.display()
                )),
                (Some(binary), Some(_)) => Some(format!("using {}", binary.display())),
            },
            install_hint: Self::resolve_binary()
                .is_none()
                .then(|| "run scripts/build-whisper-cli.sh or set AIWORK_WHISPER_CLI".to_string()),
        }
    }

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, ProviderError> {
        Self::resolve_binary().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "whisper-cli was not found; run scripts/build-whisper-cli.sh or set AIWORK_WHISPER_CLI",
            )
        })?;
        let path = model.path.as_deref().ok_or_else(|| {
            ProviderError::new(
                AIError::InvalidRequest,
                format!("STT model '{}' has no local model path", model.id),
            )
        })?;
        let path = Path::new(path).canonicalize().map_err(|error| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("cannot open Whisper model '{path}': {error}"),
            )
        })?;
        if path.extension().and_then(|value| value.to_str()) != Some("bin") {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                format!("whisper.cpp requires a .bin model: {}", path.display()),
            ));
        }
        *self.state.lock().await = Some(WhisperState {
            model_id: model.id.clone(),
            model_path: path,
            effective_device: None,
        });
        Ok(ModelHandle {
            model_id: model.id.clone(),
            provider_id: self.id().to_string(),
        })
    }

    async fn unload(&self, handle: &ModelHandle) -> Result<(), ProviderError> {
        let mut state = self.state.lock().await;
        if state
            .as_ref()
            .is_some_and(|loaded| loaded.model_id == handle.model_id)
        {
            *state = None;
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        let status = self.status().await;
        Ok(ProviderHealth {
            ok: status.ready,
            message: status.reason,
        })
    }
}

#[async_trait]
impl STTProvider for WhisperCppProvider {
    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        let binary = Self::resolve_binary().ok_or_else(|| {
            ProviderError::new(AIError::ProviderUnavailable, "whisper-cli was not found")
        })?;
        let state = self.state.lock().await.clone().ok_or_else(|| {
            ProviderError::new(AIError::ModelLoadFailed, "no Whisper model is loaded")
        })?;
        if state.model_id != request.model {
            return Err(ProviderError::new(
                AIError::ModelNotFound,
                format!("Whisper model '{}' is not loaded", request.model),
            ));
        }
        let audio = request.file.as_deref().ok_or_else(|| {
            ProviderError::new(AIError::InvalidRequest, "transcription file is required")
        })?;
        let audio = Path::new(audio).canonicalize().map_err(|error| {
            ProviderError::new(
                AIError::InvalidRequest,
                format!("cannot open audio '{audio}': {error}"),
            )
        })?;
        if audio.extension().and_then(|value| value.to_str()) != Some("wav") {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                "whisper.cpp STT currently accepts PCM WAV input",
            ));
        }

        let output = TempOutput::new("whisper-result");
        let mut command = Command::new(&binary);
        command
            .arg("-m")
            .arg(&state.model_path)
            .arg("-f")
            .arg(&audio)
            .arg("-oj")
            .arg("-of")
            .arg(&output.prefix)
            .arg("-nt")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(language) = request
            .language
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            command.arg("-l").arg(language);
        }

        let child_output = timeout(Duration::from_secs(300), command.output())
            .await
            .map_err(|_| {
                ProviderError::new(AIError::Timeout, "whisper-cli timed out after 300 seconds")
            })?
            .map_err(|error| {
                ProviderError::new(
                    AIError::BackendCrashed,
                    format!("failed to start '{}': {error}", binary.display()),
                )
            })?;
        let stderr = String::from_utf8_lossy(&child_output.stderr).to_string();
        if !child_output.status.success() {
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                format!(
                    "whisper-cli exited with {}: {}",
                    child_output.status,
                    stderr.trim()
                ),
            ));
        }

        let json_path = output.prefix.with_extension("json");
        let bytes = tokio::fs::read(&json_path).await.map_err(|error| {
            ProviderError::new(
                AIError::BackendCrashed,
                format!(
                    "whisper-cli did not produce '{}': {error}",
                    json_path.display()
                ),
            )
        })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            ProviderError::new(
                AIError::BackendCrashed,
                format!("invalid whisper-cli JSON output: {error}"),
            )
        })?;
        let (text, language) = parse_whisper_json(&value)?;

        if let Some(loaded) = self.state.lock().await.as_mut() {
            loaded.effective_device = Some(Self::detect_device(&stderr));
        }
        Ok(TranscriptionResponse {
            text,
            language,
            model: request.model,
        })
    }
}

fn parse_whisper_json(value: &Value) -> Result<(String, Option<String>), ProviderError> {
    let text = value
        .get("transcription")
        .and_then(Value::as_array)
        .map(|segments| {
            segments
                .iter()
                .filter_map(|segment| segment.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .or_else(|| {
            value
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
        .trim()
        .to_string();
    if text.is_empty() {
        return Err(ProviderError::new(
            AIError::BackendCrashed,
            "whisper-cli returned an empty transcription",
        ));
    }
    let language = value
        .pointer("/result/language")
        .or_else(|| value.get("language"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((text, language))
}

struct TempOutput {
    prefix: PathBuf,
}

impl TempOutput {
    fn new(label: &str) -> Self {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        Self {
            prefix: env::temp_dir().join(format!(
                "macai-{label}-{}-{nanos}-{counter}",
                std::process::id()
            )),
        }
    }
}

impl Drop for TempOutput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.prefix.with_extension("json"));
    }
}

fn resolve_executable(value: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(value);
    if candidate.components().count() > 1 || candidate.is_absolute() {
        return candidate.is_file().then_some(candidate);
    }
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|directory| directory.join(value))
            .find(|path| path.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whisper_json_segments() {
        let value = serde_json::json!({
            "result": {"language": "zh"},
            "transcription": [
                {"text": " 你好"},
                {"text": "，世界。"}
            ]
        });
        let (text, language) = parse_whisper_json(&value).unwrap();
        assert_eq!(text, "你好，世界。");
        assert_eq!(language.as_deref(), Some("zh"));
    }

    #[test]
    fn detects_metal_from_whisper_logs() {
        assert_eq!(
            WhisperCppProvider::detect_device("system_info: METAL = 1"),
            "metal"
        );
        assert_eq!(WhisperCppProvider::detect_device("METAL = 0"), "cpu");
    }
}
