//! RunnerProvider — 把 Runner instance manager 桥接为 ai-core Provider。
//!
//! 这是 Runner foundation 与 Runtime/scheduler 之间的唯一组合边界：
//! Runtime 用与内置 Provider 完全相同的 `Provider`/`TTSProvider` 接口调度
//! Runner-backed 模型，lease、busy guard、LRU、keep-alive 与任务历史全部
//! 走既有裁决路径。生产代码不含任何 Runner ID 或模型专属分支。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use ai_core::model::ModelSpec;
use ai_core::provider::{
    Capability, IsolationMode, ModelHandle, Provider, ProviderDescriptor, ProviderError,
    ProviderHealth, ProviderStatus, TTSProvider,
};
use ai_core::request::SpeechRequest;
use ai_core::response::SpeechResponse;
use ai_core::AIError;

use crate::runners::{ModelProfile, RunnerInstanceError, RunnerInstanceManager};

/// 单个 Runner-backed 模型的桥接配置。
#[derive(Clone)]
pub struct RunnerModelBinding {
    /// 模型在 Runtime 注册表中的 ID（ModelSpec.id）。
    pub model_id: String,
    /// Model Profile 快照（discovery 时校验过）。
    pub profile: ModelProfile,
    /// Runner manifest 声明的 environment ID（状态查询用）。
    pub environment_id: String,
    /// 模型根目录（daemon 侧授权 artifact root）。
    pub artifact_root: PathBuf,
}

/// ai-core Provider 桥接。一个实例对应一个 Runner ID；instance 复用与
/// 协议细节全部委托给 RunnerInstanceManager。
pub struct RunnerProvider {
    runner_id: String,
    /// manifest 声明的 ai-core Capability（tts.v1→TextToSpeech、stt.v1→SpeechToText）。
    capabilities: Vec<Capability>,
    /// capability 名到模型 ID 的映射（当前 v1 只桥接 tts.v1）。
    bindings: tokio::sync::RwLock<Vec<RunnerModelBinding>>,
    instances: Arc<RunnerInstanceManager>,
    temp_root: PathBuf,
}

impl RunnerProvider {
    pub fn new(
        runner_id: String,
        manifest_capabilities: &[String],
        instances: Arc<RunnerInstanceManager>,
        temp_root: PathBuf,
    ) -> Self {
        let mut capabilities = Vec::new();
        for name in manifest_capabilities {
            match name.as_str() {
                "tts.v1" => capabilities.push(Capability::TextToSpeech),
                "stt.v1" => capabilities.push(Capability::SpeechToText),
                _ => {}
            }
        }
        Self {
            runner_id,
            capabilities,
            bindings: tokio::sync::RwLock::new(Vec::new()),
            instances,
            temp_root,
        }
    }

    /// Runtime 注册 Runner-backed 模型时调用：保存 Profile 快照绑定。
    pub async fn bind_model(&self, binding: RunnerModelBinding) {
        self.bindings.write().await.retain(|existing| {
            existing.profile.id != binding.profile.id && existing.model_id != binding.model_id
        });
        self.bindings.write().await.push(binding);
    }

    pub async fn unbind_model(&self, model_id: &str) {
        self.bindings
            .write()
            .await
            .retain(|b| b.model_id != model_id);
    }

    fn binding_for<'a>(
        bindings: &'a [RunnerModelBinding],
        model_id: &str,
    ) -> Result<&'a RunnerModelBinding, ProviderError> {
        bindings
            .iter()
            .find(|binding| binding.model_id == model_id)
            .ok_or_else(|| {
                ProviderError::new(
                    AIError::ModelNotFound,
                    format!("model '{model_id}' is not bound to this Runner"),
                )
            })
    }

    async fn binding(&self, model_id: &str) -> Result<RunnerModelBinding, ProviderError> {
        let bindings = self.bindings.read().await;
        let binding = Self::binding_for(&bindings, model_id)?.clone();
        Ok(binding)
    }

    fn deadlines(
        &self,
        binding: &RunnerModelBinding,
    ) -> Result<(Duration, Duration), ProviderError> {
        let descriptor = self
            .instances
            .resolve_runner(
                &binding.profile.runner,
                &binding.profile.compatibility.runner,
            )
            .map_err(Self::map_error)?;
        descriptor
            .manifest
            .map(|manifest| {
                (
                    Duration::from_secs(manifest.timeouts.inference_seconds),
                    Duration::from_secs(manifest.timeouts.shutdown_seconds),
                )
            })
            .ok_or_else(|| {
                ProviderError::new(
                    AIError::ProviderUnavailable,
                    "trusted Runner descriptor has no manifest",
                )
            })
    }

    fn map_error(error: RunnerInstanceError) -> ProviderError {
        match error {
            RunnerInstanceError::NoTrustedRunner { .. } => {
                ProviderError::new(AIError::ProviderUnavailable, error.to_string())
            }
            RunnerInstanceError::Environment(_)
            | RunnerInstanceError::EnvironmentNotReady { .. } => {
                ProviderError::new(AIError::ProviderUnavailable, error.to_string())
            }
            RunnerInstanceError::Supervisor(SupervisorError::Deadline(_)) => {
                ProviderError::new(AIError::Timeout, error.to_string())
            }
            RunnerInstanceError::Supervisor(_) => {
                ProviderError::new(AIError::BackendCrashed, error.to_string())
            }
            RunnerInstanceError::ProtocolViolation { .. } => {
                ProviderError::new(AIError::Internal, error.to_string())
            }
            RunnerInstanceError::RunnerReportedError { code, message } => {
                let kind = match code.as_str() {
                    "invalid_request" => AIError::InvalidRequest,
                    "model_not_found" => AIError::ModelNotFound,
                    "model_load_failed" => AIError::ModelLoadFailed,
                    "provider_unavailable" => AIError::ProviderUnavailable,
                    "timeout" => AIError::Timeout,
                    "cancelled" => AIError::InvalidRequest,
                    "backend_crashed" => AIError::BackendCrashed,
                    _ => AIError::Internal,
                };
                ProviderError::new(kind, message)
            }
            RunnerInstanceError::Io { .. } => {
                ProviderError::new(AIError::Internal, error.to_string())
            }
        }
    }
}

use crate::runners::SupervisorError;

#[async_trait]
impl Provider for RunnerProvider {
    fn id(&self) -> &str {
        &self.runner_id
    }

    fn capabilities(&self) -> Vec<Capability> {
        self.capabilities.clone()
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.runner_id.clone(),
            capabilities: self.capabilities.clone(),
            isolation: IsolationMode::Worker,
            // v1 manifest 尚未声明设备集合；实际设备只能由 loaded frame 报告。
            supported_devices: Vec::new(),
        }
    }

    async fn status(&self) -> ProviderStatus {
        let bindings = self.bindings.read().await;
        let environment = bindings.first().and_then(|binding| {
            self.instances
                .environments()
                .status(&binding.environment_id)
        });
        let available = environment
            .as_ref()
            .is_some_and(|status| status.phase == crate::runners::EnvironmentPhase::Ready);
        let snapshot = self.instances.instance_snapshot(&self.runner_id).await;
        let resident = snapshot
            .as_ref()
            .filter(|state| state.alive)
            .and_then(|state| state.loaded_model.clone())
            .into_iter()
            .collect();
        let ready = available
            && snapshot
                .as_ref()
                .is_some_and(|state| state.alive && state.loaded_model.is_some());
        let reason = if bindings.is_empty() {
            Some("no Runner-backed model is bound".to_string())
        } else if !available {
            let binding = &bindings[0];
            Some(match environment {
                Some(status) => format!(
                    "environment '{}' is {}",
                    binding.environment_id,
                    status.phase.as_str()
                ),
                None => format!(
                    "environment '{}' has not been installed",
                    binding.environment_id
                ),
            })
        } else {
            snapshot.as_ref().and_then(|state| state.last_error.clone())
        };
        ProviderStatus {
            available,
            ready,
            effective_device: snapshot.and_then(|state| state.effective_device),
            resident_models: resident,
            reason,
            install_hint: None,
        }
    }

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, ProviderError> {
        let binding = self.binding(&model.id).await?;
        let artifact_root = binding.artifact_root.canonicalize().map_err(|error| {
            ProviderError::new(
                AIError::ModelNotFound,
                format!(
                    "Runner model artifact '{}' is unavailable: {error}",
                    binding.artifact_root.display()
                ),
            )
        })?;
        if let Some(path) = model.path.as_deref() {
            let requested = std::fs::canonicalize(path).map_err(|error| {
                ProviderError::new(
                    AIError::ModelNotFound,
                    format!("cannot open registered Runner model '{path}': {error}"),
                )
            })?;
            if requested != artifact_root {
                return Err(ProviderError::new(
                    AIError::InvalidRequest,
                    format!(
                        "registered path '{}' does not match Profile artifact root '{}'",
                        requested.display(),
                        artifact_root.display()
                    ),
                ));
            }
        }
        let runner_id = binding.profile.runner.clone();
        let requirement = binding.profile.compatibility.runner.clone();
        let profile_payload = profile_load_payload(&binding, model);
        self.instances
            .load_model(&runner_id, &requirement, &model.id, profile_payload)
            .await
            .map_err(Self::map_error)?;
        Ok(ModelHandle {
            model_id: model.id.clone(),
            provider_id: self.runner_id.clone(),
        })
    }

    async fn unload(&self, handle: &ModelHandle) -> Result<(), ProviderError> {
        let binding = self.binding(&handle.model_id).await?;
        let runner_id = binding.profile.runner.clone();
        let (_, deadline) = self.deadlines(&binding)?;
        self.instances
            .unload_model(&runner_id, deadline)
            .await
            .map_err(Self::map_error)
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        if let Some(snapshot) = self.instances.instance_snapshot(&self.runner_id).await {
            if !snapshot.alive {
                return Err(ProviderError::new(
                    AIError::BackendCrashed,
                    snapshot
                        .last_error
                        .unwrap_or_else(|| "runner process is not alive".to_string()),
                ));
            }
        }
        Ok(ProviderHealth {
            ok: true,
            message: None,
        })
    }

    async fn memory_usage_bytes(&self) -> Option<u64> {
        self.instances
            .instance_snapshot(&self.runner_id)
            .await
            .filter(|state| state.alive)
            .and_then(|state| state.resident_bytes)
    }

    async fn effective_device(&self) -> Option<String> {
        self.instances
            .instance_snapshot(&self.runner_id)
            .await
            .filter(|state| state.alive)
            .and_then(|state| state.effective_device)
    }
}

#[async_trait]
impl TTSProvider for RunnerProvider {
    async fn synthesize(&self, request: SpeechRequest) -> Result<SpeechResponse, ProviderError> {
        let binding = self.binding(&request.model).await?;
        let runner_id = binding.profile.runner.clone();
        // per-request 输出目录：daemon 授权，Runner 只能写这里。
        let request_dir = self.temp_root.join(format!("tts-request-{}", uuid_like()));
        std::fs::create_dir_all(&request_dir).map_err(|error| {
            ProviderError::new(
                AIError::Internal,
                format!("cannot create request output dir: {error}"),
            )
        })?;
        let output = json!({
            "directory": request_dir.display().to_string(),
            "allowed_extensions": ["wav"],
        });
        let tts_request = json!({
            "text": request.input,
            "voice": request.voice,
            "speed": request.speed,
            "format": request.format.unwrap_or_else(|| "wav".to_string()),
            "language": "auto",
        });
        let (deadline, _) = self.deadlines(&binding)?;
        let result = self
            .instances
            .infer(&runner_id, "tts.v1", tts_request, output, deadline, |_| {})
            .await;
        match result {
            Ok(payload) => {
                let path = payload
                    .get("path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .ok_or_else(|| {
                        ProviderError::new(
                            AIError::Internal,
                            "runner result did not include an output path",
                        )
                    })?;
                // daemon 校验输出路径仍在授权目录内，然后读取并清理。
                let canonical = path.canonicalize().map_err(|error| {
                    ProviderError::new(
                        AIError::BackendCrashed,
                        format!("runner output is unreadable: {error}"),
                    )
                })?;
                let allowed = request_dir.canonicalize().unwrap_or(request_dir.clone());
                if !canonical.starts_with(&allowed) {
                    let _ = std::fs::remove_dir_all(&request_dir);
                    return Err(ProviderError::new(
                        AIError::Internal,
                        "runner returned a path outside the authorized output directory",
                    ));
                }
                let audio = std::fs::read(&canonical).map_err(|error| {
                    ProviderError::new(
                        AIError::BackendCrashed,
                        format!("cannot read runner output: {error}"),
                    )
                })?;
                let bytes = audio.len() as u64;
                let _ = std::fs::remove_dir_all(&request_dir);
                Ok(SpeechResponse {
                    content_type: payload
                        .get("content_type")
                        .and_then(Value::as_str)
                        .unwrap_or("audio/wav")
                        .to_string(),
                    bytes,
                    audio,
                })
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(&request_dir);
                Err(Self::map_error(error))
            }
        }
    }
}

#[async_trait]
impl ai_core::provider::STTProvider for RunnerProvider {
    /// STT 请求：daemon 已把上传音频归一化为 PCM WAV（`request.file`），Runner
    /// worker 读该路径并返回文本。无授权输出目录（读取而非写出），结果经
    /// infer result 帧返回 text/language/device。
    async fn transcribe(
        &self,
        request: ai_core::request::TranscriptionRequest,
    ) -> Result<ai_core::response::TranscriptionResponse, ProviderError> {
        let binding = self.binding(&request.model).await?;
        let runner_id = binding.profile.runner.clone();
        let audio = request.file.clone().ok_or_else(|| {
            ProviderError::new(
                AIError::InvalidRequest,
                "transcription requires an audio file",
            )
        })?;
        let stt_request = json!({
            "audio": audio,
            "language": request.language,
        });
        let (deadline, _) = self.deadlines(&binding)?;
        let result = self
            .instances
            .infer(
                &runner_id,
                "stt.v1",
                stt_request,
                json!({}),
                deadline,
                |_| {},
            )
            .await;
        match result {
            Ok(payload) => {
                let text = payload
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ProviderError::new(
                            AIError::Internal,
                            "runner result did not include transcription text",
                        )
                    })?
                    .to_string();
                let language = payload
                    .get("language")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                Ok(ai_core::response::TranscriptionResponse {
                    text,
                    language,
                    model: request.model.clone(),
                })
            }
            Err(error) => Err(Self::map_error(error)),
        }
    }
}

/// Model Profile 到 Runner `load` payload 的规范化映射。
fn profile_load_payload(binding: &RunnerModelBinding, model: &ModelSpec) -> Value {
    json!({
        "model_id": model.id,
        "profile_id": binding.profile.id,
        "adapter": binding.profile.adapter,
        "capabilities": binding.profile.capabilities,
        "model_root": binding.artifact_root.display().to_string(),
        "defaults": {
            "voice": binding.profile.defaults.voice,
            "format": binding.profile.defaults.format,
            "keep_alive": binding.profile.defaults.keep_alive,
        },
    })
}

/// 简单的本地唯一名（时间 + 进程 + 计数）。
fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}-{}", std::process::id(), t, n)
}
