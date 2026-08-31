//! Qwen3-TTS Provider（MLX）：由 aiworkd 管理常驻 Python 推理子进程。
//!
//! 加载模型 = 拉起 `scripts/qwen3_tts_worker.py`，卸载 = 结束该进程。
//! 合成请求通过 stdin/stdout 的 JSON 行协议发给 worker，WAV 落在临时文件。

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
    ProviderHealth, ProviderStatus, TTSProvider,
};
use ai_core::request::SpeechRequest;
use ai_core::response::SpeechResponse;
use ai_core::AIError;

use crate::process_memory::resident_memory_bytes;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
const DEFAULT_VOICE: &str = "Vivian";
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);
const INFERENCE_TIMEOUT: Duration = Duration::from_secs(300);

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
    wav: Option<String>,
    #[serde(default)]
    error: Option<WorkerError>,
}

pub struct Qwen3TtsProvider {
    state: Mutex<Option<QwenState>>,
    /// 只读观测快照独立于 worker I/O 锁，避免推理期间阻塞 /api/runtime。
    resident: StdRwLock<Option<QwenResident>>,
    python: PathBuf,
    script: PathBuf,
    #[cfg(test)]
    python_override: Option<PathBuf>,
    #[cfg(test)]
    script_override: Option<PathBuf>,
    default_voice: Mutex<Option<String>>,
}

impl Qwen3TtsProvider {
    pub fn from_env() -> Self {
        Self {
            state: Mutex::new(None),
            resident: StdRwLock::new(None),
            python: PathBuf::from(".build/kokoro-venv/bin/python"),
            script: PathBuf::from("scripts/qwen3_tts_worker.py"),
            #[cfg(test)]
            python_override: None,
            #[cfg(test)]
            script_override: None,
            default_voice: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn with_test_paths(python: PathBuf, script: PathBuf) -> Self {
        let mut provider = Self::from_env();
        provider.python_override = Some(python);
        provider.script_override = Some(script);
        provider
    }

    fn resolve_python(&self) -> Option<PathBuf> {
        #[cfg(test)]
        if let Some(path) = &self.python_override {
            return path.is_file().then(|| path.clone());
        }
        if let Ok(value) = env::var("AIWORK_QWEN3_TTS_PYTHON") {
            let candidate = PathBuf::from(value.trim());
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        resolve_project_file(&self.python)
    }

    fn resolve_script(&self) -> Option<PathBuf> {
        #[cfg(test)]
        if let Some(path) = &self.script_override {
            return path
                .is_file()
                .then(|| path.canonicalize().unwrap_or(path.clone()));
        }
        if let Ok(value) = env::var("AIWORK_QWEN3_TTS_SCRIPT") {
            let candidate = PathBuf::from(value.trim());
            if candidate.is_file() {
                return Some(candidate.canonicalize().unwrap_or(candidate));
            }
        }
        resolve_project_file(&self.script).map(|path| path.canonicalize().unwrap_or(path))
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
            *self.default_voice.lock().await = None;
        }
    }
}

#[async_trait]
impl Provider for Qwen3TtsProvider {
    fn id(&self) -> &'static str {
        "qwen3-tts"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::TextToSpeech]
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id().to_string(),
            capabilities: self.capabilities(),
            isolation: IsolationMode::Worker,
            supported_devices: vec!["gpu".to_string()],
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
            Some("Qwen3-TTS Python environment was not found".to_string())
        } else if !script_found {
            Some("scripts/qwen3_tts_worker.py was not found".to_string())
        } else if ready {
            effective_device
                .as_ref()
                .map(|device| format!("Qwen3-TTS worker is ready on {device}"))
        } else {
            Some("no Qwen3-TTS model is loaded".to_string())
        };
        ProviderStatus {
            available: python_found && script_found,
            ready,
            effective_device,
            resident_models,
            reason,
            install_hint: (!python_found || !script_found)
                .then(|| "run: .build/kokoro-venv/bin/pip install -U mlx-audio".to_string()),
        }
    }

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, ProviderError> {
        let python = self.resolve_python().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "Qwen3-TTS Python environment was not found (.build/kokoro-venv)",
            )
        })?;
        let script = self.resolve_script().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "scripts/qwen3_tts_worker.py was not found",
            )
        })?;
        let model_path = model.path.as_deref().ok_or_else(|| {
            ProviderError::new(
                AIError::InvalidRequest,
                format!("Qwen3-TTS model '{}' has no local model path", model.id),
            )
        })?;
        let model_dir = Path::new(model_path).canonicalize().map_err(|error| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("cannot open Qwen3-TTS model dir '{model_path}': {error}"),
            )
        })?;
        if !model_dir.is_dir() {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                format!("Qwen3-TTS model path is not a directory: {model_dir:?}"),
            ));
        }

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
                    AIError::BackendCrashed,
                    format!("failed to start Qwen3-TTS worker: {error}"),
                )
            })?;
        let pid = child.id();
        let stdin = child.stdin.take().ok_or_else(|| {
            ProviderError::new(AIError::Internal, "Qwen3-TTS worker stdin unavailable")
        })?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            ProviderError::new(AIError::Internal, "Qwen3-TTS worker stdout unavailable")
        })?;

        let mut line = String::new();
        let readiness = timeout(LOAD_TIMEOUT, async {
            BufReader::new(&mut stdout).read_line(&mut line).await
        })
        .await;
        match readiness {
            Ok(Ok(count)) if count > 0 => {}
            Ok(Ok(_)) => {
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::ModelLoadFailed,
                    "Qwen3-TTS worker exited before reporting readiness",
                ));
            }
            Ok(Err(error)) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::ModelLoadFailed,
                    format!("failed to read Qwen3-TTS readiness: {error}"),
                ));
            }
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ProviderError::new(
                    AIError::Timeout,
                    "Qwen3-TTS model load timed out after 600 seconds",
                ));
            }
        }
        let reply: ReadyReply = serde_json::from_str(line.trim()).map_err(|error| {
            ProviderError::new(
                AIError::ModelLoadFailed,
                format!("invalid Qwen3-TTS readiness response: {error}"),
            )
        })?;
        if !reply.ready {
            let message = reply
                .error
                .map(|error| format!("{}: {}", error.code, error.message))
                .unwrap_or_else(|| "Qwen3-TTS model could not be loaded".to_string());
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(ProviderError::new(AIError::ModelLoadFailed, message));
        }
        let device = reply.device.unwrap_or_else(|| "gpu".to_string());
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
            device,
        }));
        *self.default_voice.lock().await = model.default_voice.clone();
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
                        "model '{}' is not resident; Qwen3-TTS currently holds '{}'",
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
impl TTSProvider for Qwen3TtsProvider {
    async fn synthesize(&self, request: SpeechRequest) -> Result<SpeechResponse, ProviderError> {
        validate_request(&request)?;
        let speed = request.speed.unwrap_or(1.0);
        let voice = {
            let default_voice = self.default_voice.lock().await;
            resolve_voice(request.voice.as_deref(), default_voice.as_deref())
        };
        let request_id = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let payload = serde_json::json!({
            "id": request_id,
            "text": request.input,
            "voice": voice,
            "speed": speed,
            "language": "Auto",
        });
        let mut guard = self.state.lock().await;
        let state = guard.as_mut().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "Qwen3-TTS model is not loaded",
            )
        })?;
        if state.model_id != request.model {
            return Err(ProviderError::new(
                AIError::ModelNotFound,
                format!("Qwen3-TTS model '{}' is not loaded", request.model),
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
                format!("Qwen3-TTS worker is unreachable: {error}"),
            ));
        }
        if let Err(error) = state.worker.stdin.flush().await {
            self.terminate_locked(&mut guard).await;
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                format!("Qwen3-TTS worker flush failed: {error}"),
            ));
        }

        let mut line = String::new();
        let read_result = timeout(INFERENCE_TIMEOUT, async {
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
                    "Qwen3-TTS worker exited unexpectedly",
                ));
            }
            Ok(Err(error)) => {
                self.terminate_locked(&mut guard).await;
                return Err(ProviderError::new(
                    AIError::BackendCrashed,
                    format!("Qwen3-TTS worker read failed: {error}"),
                ));
            }
            Err(_) => {
                self.terminate_locked(&mut guard).await;
                return Err(ProviderError::new(
                    AIError::Timeout,
                    "Qwen3-TTS synthesis timed out after 300 seconds; worker was restarted",
                ));
            }
        }

        let reply: WorkerReply = match serde_json::from_str(line.trim()) {
            Ok(reply) => reply,
            Err(error) => {
                self.terminate_locked(&mut guard).await;
                return Err(ProviderError::new(
                    AIError::BackendCrashed,
                    format!("invalid Qwen3-TTS worker reply: {error}"),
                ));
            }
        };
        if reply.id != Some(request_id) {
            self.terminate_locked(&mut guard).await;
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                format!(
                    "Qwen3-TTS worker response id mismatch: expected {request_id}, got {:?}",
                    reply.id
                ),
            ));
        }
        if !reply.ok {
            let worker_error = reply.error.unwrap_or(WorkerError {
                code: "inference_error".to_string(),
                message: "Qwen3-TTS inference failed".to_string(),
            });
            let kind = if worker_error.code == "invalid_request" {
                AIError::InvalidRequest
            } else {
                AIError::BackendCrashed
            };
            return Err(ProviderError::new(kind, worker_error.message));
        }
        let wav_path = match reply.wav {
            Some(path) => path,
            None => {
                self.terminate_locked(&mut guard).await;
                return Err(ProviderError::new(
                    AIError::BackendCrashed,
                    "Qwen3-TTS worker returned no audio path",
                ));
            }
        };
        let audio = match tokio::fs::read(&wav_path).await {
            Ok(audio) => audio,
            Err(error) => {
                self.terminate_locked(&mut guard).await;
                return Err(ProviderError::new(
                    AIError::BackendCrashed,
                    format!("cannot read synthesized WAV: {error}"),
                ));
            }
        };
        let _ = tokio::fs::remove_file(&wav_path).await;
        if audio.len() < 44 || &audio[0..4] != b"RIFF" || &audio[8..12] != b"WAVE" {
            self.terminate_locked(&mut guard).await;
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                "Qwen3-TTS produced an invalid WAV file",
            ));
        }
        Ok(SpeechResponse {
            content_type: "audio/wav".to_string(),
            bytes: audio.len() as u64,
            audio,
        })
    }
}

fn validate_request(request: &SpeechRequest) -> Result<(), ProviderError> {
    if request.input.trim().is_empty() {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "speech input must not be empty",
        ));
    }
    if request.input.chars().count() > 5_000 {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "speech input exceeds the 5000 character limit",
        ));
    }
    if request.format.as_deref().unwrap_or("wav") != "wav" {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "qwen3-tts currently supports format='wav' only",
        ));
    }
    if !((0.25..=4.0).contains(&request.speed.unwrap_or(1.0))) {
        return Err(ProviderError::new(
            AIError::InvalidRequest,
            "speech speed must be between 0.25 and 4.0",
        ));
    }
    Ok(())
}

fn resolve_voice(request_voice: Option<&str>, default_voice: Option<&str>) -> String {
    request_voice
        .and_then(non_empty_trimmed)
        .or_else(|| default_voice.and_then(non_empty_trimmed))
        .unwrap_or_else(|| DEFAULT_VOICE.to_string())
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn resolve_project_file(relative: &Path) -> Option<PathBuf> {
    [".", "..", "../.."]
        .into_iter()
        .map(|base| PathBuf::from(base).join(relative))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn request(input: &str, format: &str, speed: f64) -> SpeechRequest {
        SpeechRequest {
            model: "qwen-test".to_string(),
            input: input.to_string(),
            voice: None,
            format: Some(format.to_string()),
            speed: Some(speed),
        }
    }

    #[test]
    fn voice_resolution_prefers_explicit_default_and_builtin() {
        assert_eq!(
            resolve_voice(Some("Vivian, happy"), Some("Ryan")),
            "Vivian, happy"
        );
        assert_eq!(resolve_voice(Some("   "), Some(" Ryan ")), "Ryan");
        assert_eq!(resolve_voice(None, None), DEFAULT_VOICE);
    }

    #[tokio::test]
    async fn validates_before_touching_worker_state() {
        let provider = Qwen3TtsProvider::from_env();
        let overlong = "字".repeat(5_001);
        for invalid in [
            request("   ", "wav", 1.0),
            request(&overlong, "wav", 1.0),
            request("你好", "mp3", 1.0),
            request("你好", "wav", 0.24),
            request("你好", "wav", 4.01),
        ] {
            assert_eq!(
                provider.synthesize(invalid).await.unwrap_err().kind,
                AIError::InvalidRequest
            );
        }
    }

    #[tokio::test]
    async fn accepts_validation_boundaries_before_unloaded_error() {
        let provider = Qwen3TtsProvider::from_env();
        for valid in [
            request(&"字".repeat(5_000), "wav", 1.0),
            request("你好", "wav", 0.25),
            request("你好", "wav", 4.0),
        ] {
            assert_eq!(
                provider.synthesize(valid).await.unwrap_err().kind,
                AIError::ProviderUnavailable
            );
        }
    }

    #[tokio::test]
    async fn observability_does_not_wait_for_active_synthesis_lock() {
        let provider = Qwen3TtsProvider::from_env();
        provider.set_resident(Some(QwenResident {
            model_id: "qwen-test".to_string(),
            pid: Some(std::process::id()),
            device: "gpu".to_string(),
        }));
        let _active = provider.state.lock().await;
        let status = timeout(Duration::from_secs(1), provider.status())
            .await
            .unwrap();
        assert!(status.ready);
        assert_eq!(status.resident_models, ["qwen-test"]);
        assert_eq!(provider.effective_device().await.as_deref(), Some("gpu"));
    }

    fn fake_worker(mode: &str) -> (Qwen3TtsProvider, PathBuf) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("macai-qwen3-tts-{nonce}"));
        let model_dir = root.join("model");
        fs::create_dir_all(&model_dir).expect("create fake model directory");
        let script = root.join("worker.sh");
        let script_body = r#"#!/bin/sh
model_dir="$2"
printf '{"ready":true,"device":"mock"}\n'
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "__MODE__" in
        success)
            printf '%s' "$line" > "$model_dir/request.json"
            printf 'RIFF1234WAVE........................................' > "$model_dir/result.wav"
            printf '{"id":%s,"ok":true,"wav":"%s"}\n' "$id" "$model_dir/result.wav"
            ;;
        worker_error)
            printf '{"id":%s,"ok":false,"error":{"code":"inference_error","message":"mock synthesis failed"}}\n' "$id"
            ;;
        malformed)
            printf 'this is not json\n'
            ;;
        mismatched_id)
            printf '{"id":999999,"ok":true,"wav":"%s/result.wav"}\n' "$model_dir"
            ;;
        missing_wav)
            printf '{"id":%s,"ok":true}\n' "$id"
            ;;
        invalid_wav)
            printf 'invalid wav' > "$model_dir/result.wav"
            printf '{"id":%s,"ok":true,"wav":"%s/result.wav"}\n' "$id" "$model_dir"
            ;;
    esac
done
"#
        .replace("__MODE__", mode);
        fs::write(&script, script_body).expect("write fake worker");
        let provider = Qwen3TtsProvider::with_test_paths(PathBuf::from("/bin/sh"), script);
        (provider, model_dir)
    }

    async fn loaded_fake_provider(
        mode: &str,
        default_voice: Option<&str>,
    ) -> (Qwen3TtsProvider, ModelHandle, PathBuf) {
        let (provider, model_dir) = fake_worker(mode);
        let spec = ModelSpec {
            id: "qwen-test".to_string(),
            name: "fake Qwen3-TTS".to_string(),
            model_type: "tts".to_string(),
            provider: "qwen3-tts".to_string(),
            source: None,
            path: Some(model_dir.to_string_lossy().into_owned()),
            format: None,
            size_bytes: None,
            memory_estimate: None,
            keep_alive: None,
            context_length: None,
            default_voice: default_voice.map(str::to_string),
        };
        let handle = provider.load(&spec).await.expect("fake worker must load");
        (provider, handle, model_dir)
    }

    #[tokio::test]
    async fn round_trip_maps_request_and_validates_wav_response() {
        let (provider, handle, model_dir) = loaded_fake_provider("success", Some("Ryan")).await;
        let response = provider
            .synthesize(SpeechRequest {
                model: "qwen-test".to_string(),
                input: "hello".to_string(),
                voice: None,
                format: None,
                speed: Some(1.25),
            })
            .await
            .expect("fake synthesis must succeed");
        assert_eq!(response.content_type, "audio/wav");
        assert_eq!(response.bytes, response.audio.len() as u64);
        assert!(response.audio.starts_with(b"RIFF"));
        assert_eq!(&response.audio[8..12], b"WAVE");
        let payload = fs::read_to_string(model_dir.join("request.json")).expect("request captured");
        assert!(payload.contains("\"text\":\"hello\""));
        assert!(payload.contains("\"voice\":\"Ryan\""));
        assert!(payload.contains("\"speed\":1.25"));
        assert!(payload.contains("\"language\":\"Auto\""));
        provider
            .unload(&handle)
            .await
            .expect("fake worker must unload");
        fs::remove_dir_all(model_dir.parent().unwrap()).expect("remove fake worker files");
    }

    #[tokio::test]
    async fn worker_error_is_structured_and_keeps_worker_ready() {
        let (provider, handle, model_dir) = loaded_fake_provider("worker_error", None).await;
        let error = provider
            .synthesize(request("hello", "wav", 1.0))
            .await
            .expect_err("worker error must fail synthesis");
        assert_eq!(error.kind, AIError::BackendCrashed);
        assert!(error.message.contains("mock synthesis failed"));
        assert!(provider.status().await.ready);
        provider
            .unload(&handle)
            .await
            .expect("fake worker must unload");
        fs::remove_dir_all(model_dir.parent().unwrap()).expect("remove fake worker files");
    }

    #[tokio::test]
    async fn malformed_and_mismatched_worker_replies_are_isolated() {
        for mode in ["malformed", "mismatched_id", "missing_wav"] {
            let (provider, _handle, model_dir) = loaded_fake_provider(mode, None).await;
            let error = provider
                .synthesize(request("hello", "wav", 1.0))
                .await
                .expect_err("bad worker reply must fail synthesis");
            assert_eq!(error.kind, AIError::BackendCrashed, "mode={mode}");
            assert!(!provider.status().await.ready, "mode={mode}");
            fs::remove_dir_all(model_dir.parent().unwrap()).expect("remove fake worker files");
        }
    }

    #[tokio::test]
    async fn invalid_wav_is_rejected_and_temporary_file_is_removed() {
        let (provider, handle, model_dir) = loaded_fake_provider("invalid_wav", None).await;
        let error = provider
            .synthesize(request("hello", "wav", 1.0))
            .await
            .expect_err("invalid WAV must fail synthesis");
        assert_eq!(error.kind, AIError::BackendCrashed);
        assert!(!model_dir.join("result.wav").exists());
        provider
            .unload(&handle)
            .await
            .expect("fake worker must unload");
        fs::remove_dir_all(model_dir.parent().unwrap()).expect("remove fake worker files");
    }

    #[tokio::test]
    async fn missing_runtime_files_report_unavailable_without_loading() {
        let provider = Qwen3TtsProvider::with_test_paths(
            PathBuf::from("/path/that/does/not/exist"),
            PathBuf::from("/another/missing/worker.py"),
        );
        let status = provider.status().await;
        assert!(!status.available);
        assert!(!status.ready);
        assert!(status.reason.is_some());
        assert!(status.install_hint.is_some());
    }

    #[tokio::test]
    async fn failed_readiness_is_reported_as_model_load_failure() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("macai-qwen3-tts-load-failure-{nonce}"));
        fs::create_dir_all(root.join("model")).expect("create fake model directory");
        let script = root.join("worker.sh");
        fs::write(
            &script,
            "#!/bin/sh\nprintf '{\"ready\":false,\"error\":{\"code\":\"model_load_failed\",\"message\":\"mock load failure\"}}\\n'\n",
        )
        .expect("write fake worker");
        let provider = Qwen3TtsProvider::with_test_paths(PathBuf::from("/bin/sh"), script);
        let model = ModelSpec {
            id: "qwen-test".to_string(),
            name: "fake Qwen3-TTS".to_string(),
            model_type: "tts".to_string(),
            provider: "qwen3-tts".to_string(),
            source: None,
            path: Some(root.join("model").to_string_lossy().into_owned()),
            format: None,
            size_bytes: None,
            memory_estimate: None,
            keep_alive: None,
            context_length: None,
            default_voice: None,
        };
        let error = provider.load(&model).await.expect_err("load must fail");
        assert_eq!(error.kind, AIError::ModelLoadFailed);
        assert!(error.message.contains("mock load failure"));
        fs::remove_dir_all(root).expect("remove fake worker files");
    }
}
