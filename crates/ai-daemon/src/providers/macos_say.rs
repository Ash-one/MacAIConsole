//! macOS 系统 TTS Provider：使用 `say` 合成，再用 `afconvert` 输出 PCM WAV。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

use ai_core::model::ModelSpec;
use ai_core::provider::{
    Capability, IsolationMode, ModelHandle, Provider, ProviderDescriptor, ProviderError,
    ProviderHealth, ProviderStatus, TTSProvider,
};
use ai_core::request::SpeechRequest;
use ai_core::response::SpeechResponse;
use ai_core::AIError;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const SAY: &str = "/usr/bin/say";
const AFCONVERT: &str = "/usr/bin/afconvert";

pub struct MacOSSayProvider {
    loaded_model: Mutex<Option<String>>,
}

impl MacOSSayProvider {
    pub fn new() -> Self {
        Self {
            loaded_model: Mutex::new(None),
        }
    }

    fn available() -> bool {
        Path::new(SAY).is_file() && Path::new(AFCONVERT).is_file()
    }
}

#[async_trait]
impl Provider for MacOSSayProvider {
    fn id(&self) -> &'static str {
        "macos-say"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::TextToSpeech]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id().to_string(),
            capabilities: self.capabilities(),
            isolation: IsolationMode::Worker,
            supported_devices: vec!["system".to_string()],
        }
    }

    async fn status(&self) -> ProviderStatus {
        let loaded = self.loaded_model.lock().await.clone();
        ProviderStatus {
            available: Self::available(),
            ready: Self::available() && loaded.is_some(),
            effective_device: loaded.as_ref().map(|_| "system".to_string()),
            resident_models: loaded.into_iter().collect(),
            reason: Some(format!("using {SAY} and {AFCONVERT}")),
            install_hint: (!Self::available())
                .then(|| "macOS say/afconvert tools are required".to_string()),
        }
    }

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, ProviderError> {
        if !Self::available() {
            return Err(ProviderError::new(
                AIError::ProviderUnavailable,
                "macOS say/afconvert tools are unavailable",
            ));
        }
        *self.loaded_model.lock().await = Some(model.id.clone());
        Ok(ModelHandle {
            model_id: model.id.clone(),
            provider_id: self.id().to_string(),
        })
    }

    async fn unload(&self, handle: &ModelHandle) -> Result<(), ProviderError> {
        let mut loaded = self.loaded_model.lock().await;
        if loaded.as_deref() == Some(&handle.model_id) {
            *loaded = None;
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth {
            ok: Self::available(),
            message: Some(format!("using {SAY} and {AFCONVERT}")),
        })
    }
}

#[async_trait]
impl TTSProvider for MacOSSayProvider {
    async fn synthesize(&self, request: SpeechRequest) -> Result<SpeechResponse, ProviderError> {
        if request.input.trim().is_empty() {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                "speech input must not be empty",
            ));
        }
        if request.input.chars().count() > 10_000 {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                "speech input exceeds the 10000 character limit",
            ));
        }
        let format = request.format.as_deref().unwrap_or("wav");
        if format != "wav" {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                "macos-say currently supports format='wav' only",
            ));
        }
        let speed = request.speed.unwrap_or(1.0);
        if !(0.25..=4.0).contains(&speed) {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                "speech speed must be between 0.25 and 4.0",
            ));
        }
        let voice = request
            .voice
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Tingting");
        let rate = (180.0 * speed).round().clamp(80.0, 500.0) as u32;
        let temp = TempSpeech::new();

        let say_output = timeout(
            Duration::from_secs(120),
            Command::new(SAY)
                .arg("-v")
                .arg(voice)
                .arg("-r")
                .arg(rate.to_string())
                .arg("-o")
                .arg(&temp.aiff)
                .arg("--")
                .arg(&request.input)
                .stdin(Stdio::null())
                .output(),
        )
        .await
        .map_err(|_| ProviderError::new(AIError::Timeout, "macOS say timed out"))?
        .map_err(|error| {
            ProviderError::new(
                AIError::BackendCrashed,
                format!("failed to start macOS say: {error}"),
            )
        })?;
        if !say_output.status.success() {
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                format!(
                    "macOS say exited with {}: {}",
                    say_output.status,
                    String::from_utf8_lossy(&say_output.stderr).trim()
                ),
            ));
        }

        let convert_output = timeout(
            Duration::from_secs(60),
            Command::new(AFCONVERT)
                .arg("-f")
                .arg("WAVE")
                .arg("-d")
                .arg("LEI16@16000")
                .arg("-c")
                .arg("1")
                .arg(&temp.aiff)
                .arg(&temp.wav)
                .stdin(Stdio::null())
                .output(),
        )
        .await
        .map_err(|_| ProviderError::new(AIError::Timeout, "afconvert timed out"))?
        .map_err(|error| {
            ProviderError::new(
                AIError::BackendCrashed,
                format!("failed to start afconvert: {error}"),
            )
        })?;
        if !convert_output.status.success() {
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                format!(
                    "afconvert exited with {}: {}",
                    convert_output.status,
                    String::from_utf8_lossy(&convert_output.stderr).trim()
                ),
            ));
        }

        let audio = tokio::fs::read(&temp.wav).await.map_err(|error| {
            ProviderError::new(
                AIError::Internal,
                format!("cannot read synthesized WAV: {error}"),
            )
        })?;
        if audio.len() < 44 || &audio[0..4] != b"RIFF" || &audio[8..12] != b"WAVE" {
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                "macOS TTS produced an invalid WAV file",
            ));
        }
        Ok(SpeechResponse {
            content_type: "audio/wav".to_string(),
            bytes: audio.len() as u64,
            audio,
        })
    }
}

struct TempSpeech {
    directory: PathBuf,
    aiff: PathBuf,
    wav: PathBuf,
}

impl TempSpeech {
    fn new() -> Self {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let directory = std::env::temp_dir().join(format!(
            "macai-tts-{}-{nanos}-{counter}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&directory);
        Self {
            aiff: directory.join("speech.aiff"),
            wav: directory.join("speech.wav"),
            directory,
        }
    }
}

impl Drop for TempSpeech {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_tools_exist_on_supported_host() {
        assert!(MacOSSayProvider::available());
    }
}
