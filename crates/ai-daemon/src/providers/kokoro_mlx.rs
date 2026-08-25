//! Kokoro TTS Provider（MLX）：由 aiworkd 管理常驻 Python 推理子进程。
//!
//! 加载模型 = 拉起 `scripts/kokoro_worker.py`（mlx-audio），卸载 = 结束该进程。
//! 合成请求通过 stdin/stdout 的 JSON 行协议发给 worker，WAV 落在临时文件。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
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

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// worker 进程 + 其 stdin/stdout。Drop 时自动结束进程。
struct WorkerProcess {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        // 先关 stdin 让 worker 自然退出，再兜底 kill。
        let _ = self.child.start_kill();
    }
}

#[derive(Deserialize)]
struct WorkerReply {
    #[allow(dead_code)]
    id: Option<u64>,
    ok: bool,
    #[serde(default)]
    wav: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

pub struct KokoroMlxProvider {
    state: Mutex<Option<KokoroState>>,
    python: PathBuf,
    script: PathBuf,
    /// 当前驻留模型的默认音色（load 时从 spec 读取，set_default_voice 时同步）。
    default_voice: Mutex<Option<String>>,
}

struct KokoroState {
    model_id: String,
    worker: WorkerProcess,
}

impl KokoroMlxProvider {
    pub fn from_env() -> Self {
        Self {
            state: Mutex::new(None),
            python: PathBuf::from(".build/kokoro-venv/bin/python"),
            script: PathBuf::from("scripts/kokoro_worker.py"),
            default_voice: Mutex::new(None),
        }
    }

    fn resolve_python(&self) -> Option<PathBuf> {
        if let Ok(value) = std::env::var("AIWORK_KOKORO_PYTHON") {
            let value = PathBuf::from(value);
            if value.is_file() {
                return Some(value);
            }
        }
        // 工作目录为仓库根；找不到时向上探测两层，兼容不同启动方式。
        for base in [
            PathBuf::from("."),
            PathBuf::from(".."),
            PathBuf::from("../.."),
        ] {
            let candidate = base.join(&self.python);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    fn resolve_script(&self) -> Option<PathBuf> {
        for base in [
            PathBuf::from("."),
            PathBuf::from(".."),
            PathBuf::from("../.."),
        ] {
            let candidate = base.join(&self.script);
            if candidate.is_file() {
                return Some(candidate.canonicalize().unwrap_or(candidate));
            }
        }
        None
    }

    async fn probe_ready(&self) -> bool {
        let mut guard = self.state.lock().await;
        match guard.as_mut() {
            Some(state) => matches!(state.worker.child.try_wait(), Ok(None)),
            None => false,
        }
    }
}

#[async_trait]
impl Provider for KokoroMlxProvider {
    fn id(&self) -> &'static str {
        "kokoro-mlx"
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
        let ready = self.probe_ready().await;
        let python_found = self.resolve_python().is_some();
        let script_found = self.resolve_script().is_some();
        let resident = {
            let guard = self.state.lock().await;
            guard
                .as_ref()
                .map(|state| vec![state.model_id.clone()])
                .unwrap_or_default()
        };
        let reason = if !python_found {
            Some("kokoro venv python not found".to_string())
        } else if !script_found {
            Some("scripts/kokoro_worker.py not found".to_string())
        } else {
            Some("using mlx-audio (Metal GPU)".to_string())
        };
        let install_hint = (!python_found).then(|| {
            "run: .build/kokoro-venv/bin/pip install mlx-audio 'misaki[zh]' phonemizer-fork espeakng-loader"
                .to_string()
        });
        ProviderStatus {
            available: python_found && script_found,
            ready,
            effective_device: ready.then(|| "gpu".to_string()),
            resident_models: resident,
            reason,
            install_hint,
        }
    }

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, ProviderError> {
        let python = self.resolve_python().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "kokoro venv python was not found (.build/kokoro-venv)",
            )
        })?;
        let script = self.resolve_script().ok_or_else(|| {
            ProviderError::new(
                AIError::ProviderUnavailable,
                "scripts/kokoro_worker.py was not found",
            )
        })?;
        let model_dir = model.path.as_deref().ok_or_else(|| {
            ProviderError::new(
                AIError::InvalidRequest,
                format!("TTS model '{}' has no local model path", model.id),
            )
        })?;
        let model_dir = Path::new(model_dir).canonicalize().map_err(|error| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!("cannot open Kokoro model dir '{model_dir}': {error}"),
            )
        })?;

        let mut child = Command::new(&python)
            .arg(&script)
            .arg("--model")
            .arg(&model_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                ProviderError::new(
                    AIError::BackendCrashed,
                    format!("failed to start kokoro worker: {error}"),
                )
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ProviderError::new(AIError::Internal, "kokoro worker stdin unavailable")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ProviderError::new(AIError::Internal, "kokoro worker stdout unavailable")
        })?;

        *self.state.lock().await = Some(KokoroState {
            model_id: model.id.clone(),
            worker: WorkerProcess {
                child,
                stdin,
                stdout,
            },
        });
        *self.default_voice.lock().await = model.default_voice.clone();
        Ok(ModelHandle {
            model_id: model.id.clone(),
            provider_id: self.id().to_string(),
        })
    }

    async fn unload(&self, handle: &ModelHandle) -> Result<(), ProviderError> {
        let mut guard = self.state.lock().await;
        let matches = guard
            .as_ref()
            .is_some_and(|state| state.model_id == handle.model_id);
        if matches {
            if let Some(state) = guard.take() {
                let mut worker = state.worker;
                let _ = worker.stdin.shutdown().await;
                match timeout(Duration::from_secs(5), worker.child.wait()).await {
                    Ok(_) => {}
                    Err(_) => {
                        let _ = worker.child.start_kill();
                        let _ = worker.child.wait().await;
                    }
                }
            }
            *self.default_voice.lock().await = None;
        } else if let Some(state) = guard.as_ref() {
            let resident = state.model_id.clone();
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                format!(
                    "model '{}' is not resident; kokoro currently holds '{}'",
                    handle.model_id, resident
                ),
            ));
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        let status = self.status().await;
        Ok(ProviderHealth {
            ok: status.available && (!status.ready || self.probe_ready().await),
            message: status.reason,
        })
    }

    async fn memory_usage_bytes(&self) -> Option<u64> {
        let state = self.state.lock().await;
        let pid = state.as_ref()?.worker.child.id()?;
        resident_memory_bytes(pid)
    }

    async fn effective_device(&self) -> Option<String> {
        self.state.lock().await.as_ref().map(|_| "gpu".to_string())
    }
}

#[async_trait]
impl TTSProvider for KokoroMlxProvider {
    async fn synthesize(&self, request: SpeechRequest) -> Result<SpeechResponse, ProviderError> {
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
        let format = request.format.as_deref().unwrap_or("wav");
        if format != "wav" {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                "kokoro-mlx currently supports format='wav' only",
            ));
        }
        let speed = request.speed.unwrap_or(1.0);
        if !(0.25..=4.0).contains(&speed) {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                "speech speed must be between 0.25 and 4.0",
            ));
        }
        let default_voice_guard = self.default_voice.lock().await;
        let voice = request
            .voice
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(default_voice_guard.as_deref())
            .unwrap_or("zf_001");
        let request_id = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let payload = serde_json::json!({
            "id": request_id,
            "text": request.input,
            "voice": voice,
            "speed": speed,
        });

        // 序列化访问 worker：同一时间只处理一个请求。
        let mut guard = self.state.lock().await;
        let state = guard.as_mut().ok_or_else(|| {
            ProviderError::new(AIError::ProviderUnavailable, "kokoro model is not loaded")
        })?;

        state
            .worker
            .stdin
            // JSON 行协议：必须以 \n 结尾，worker 按行读取。
            .write_all(format!("{}\n", payload).as_bytes())
            .await
            .map_err(|error| {
                ProviderError::new(
                    AIError::BackendCrashed,
                    format!("kokoro worker is unreachable: {error}"),
                )
            })?;
        state.worker.stdin.flush().await.map_err(|error| {
            ProviderError::new(
                AIError::BackendCrashed,
                format!("kokoro worker flush failed: {error}"),
            )
        })?;

        let mut reader = BufReader::new(&mut state.worker.stdout);
        tracing::info!(request_id, "kokoro payload written, awaiting worker reply");
        let mut line = String::new();
        let read_result = timeout(Duration::from_secs(300), reader.read_line(&mut line)).await;
        match read_result {
            Ok(Ok(n)) if n > 0 => {}
            Ok(Ok(_)) => {
                return Err(ProviderError::new(
                    AIError::BackendCrashed,
                    "kokoro worker exited unexpectedly",
                ))
            }
            Ok(Err(error)) => {
                return Err(ProviderError::new(
                    AIError::BackendCrashed,
                    format!("kokoro worker read failed: {error}"),
                ))
            }
            Err(_) => {
                return Err(ProviderError::new(
                    AIError::Timeout,
                    "kokoro synthesis timed out after 300s",
                ))
            }
        }

        let reply: WorkerReply = serde_json::from_str(line.trim()).map_err(|error| {
            ProviderError::new(
                AIError::BackendCrashed,
                format!("invalid kokoro worker reply: {error}"),
            )
        })?;
        if !reply.ok {
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                reply
                    .error
                    .unwrap_or_else(|| "kokoro synthesis failed".to_string()),
            ));
        }
        let wav_path = reply.wav.ok_or_else(|| {
            ProviderError::new(
                AIError::BackendCrashed,
                "kokoro worker returned no audio path",
            )
        })?;

        let audio = tokio::fs::read(&wav_path).await.map_err(|error| {
            ProviderError::new(
                AIError::Internal,
                format!("cannot read synthesized WAV '{wav_path}': {error}"),
            )
        })?;
        let _ = tokio::fs::remove_file(&wav_path).await;
        if audio.len() < 44 || &audio[0..4] != b"RIFF" || &audio[8..12] != b"WAVE" {
            return Err(ProviderError::new(
                AIError::BackendCrashed,
                "kokoro produced an invalid WAV file",
            ));
        }
        Ok(SpeechResponse {
            content_type: "audio/wav".to_string(),
            bytes: audio.len() as u64,
            audio,
        })
    }
}
