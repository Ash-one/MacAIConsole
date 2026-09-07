//! RunnerProvider — 把 Runner instance manager 桥接为 ai-core Provider。
//!
//! 这是 Runner foundation 与 Runtime/scheduler 之间的唯一组合边界：
//! Runtime 用与内置 Provider 完全相同的 `Provider`/`TTSProvider` 接口调度
//! Runner-backed 模型，lease、busy guard、LRU、keep-alive 与任务历史全部
//! 走既有裁决路径。生产代码不含任何 Runner ID 或模型专属分支。

use std::path::{Path, PathBuf};
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

use crate::runners::{
    ModelProfile, ProfileArtifacts, ProfileCompatibility, ProfileDefaults, ProfileResources,
    ProfileSource, RunnerInstanceError, RunnerInstanceManager,
};

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
    /// manifest 声明的 ai-core Capability（chat.v1→Chat、tts.v1→TextToSpeech、
    /// stt.v1→SpeechToText）。
    capabilities: Vec<Capability>,
    /// capability 名到模型 ID 的映射（v1 桥接 chat.v1 / tts.v1 / stt.v1）。
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
                "chat.v1" => capabilities.push(Capability::Chat),
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

    /// ad-hoc 绑定的默认 environment：instance manager 里该 Runner 的
    /// 唯一环境（manifest `runtime.id`）。manager 未装配时返回占位，
    /// 让后续 ensure 阶段给出结构化错误。
    pub async fn default_environment_id(&self) -> String {
        for entry in self.instances.discovered() {
            if let Some(manifest) = &entry.manifest {
                if manifest.id == self.runner_id {
                    return manifest.runtime.id.clone();
                }
            }
        }
        self.runner_id.clone()
    }

    /// Runtime 注册 Runner-backed 模型时调用：保存 Profile 快照绑定。
    pub async fn bind_model(&self, binding: RunnerModelBinding) {
        let mut bindings = self.bindings.write().await;
        bindings.retain(|existing| existing.model_id != binding.model_id);
        bindings.push(binding);
    }

    /// 注册任意模型的 ad-hoc 绑定（无 bundled catalog 的引擎用，如 llama.cpp）：
    /// 以模型注册 ID 与注册目录在内存中构造 Model Profile。它不持久化
    /// snapshot、不参与 catalog digest——模型注册记录（models.db）本身才是
    /// 这个模型的持久身份。同 ID 重新注册时原子替换 path/profile 绑定，保证它与
    /// 即将写入的 ModelSpec 一致，并返回 false。
    pub async fn bind_adhoc_model(
        &self,
        model_id: &str,
        model_dir: &Path,
        adapter: &str,
        format: &str,
    ) -> Result<bool, String> {
        let directory = model_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| format!("model path has no directory name: {}", model_dir.display()))?;
        let profile = ModelProfile {
            schema: "macai.model.v1".to_string(),
            id: model_id.to_string(),
            name: model_id.to_string(),
            capabilities: self
                .capabilities
                .iter()
                .map(|capability| capability.profile_name().to_string())
                .collect(),
            runner: self.runner_id.clone(),
            adapter: adapter.to_string(),
            format: format.to_string(),
            source: ProfileSource {
                source_type: "local".to_string(),
                repo: String::new(),
                revision: String::new(),
            },
            artifacts: ProfileArtifacts {
                directory,
                files: Vec::new(),
            },
            defaults: ProfileDefaults::default(),
            resources: ProfileResources::default(),
            routing: None,
            compatibility: ProfileCompatibility {
                runner: ">=0.1,<2".to_string(),
            },
        };
        profile
            .validate()
            .map_err(|error| format!("ad-hoc profile for '{model_id}' is invalid: {error}"))?;
        let environment_id = self.default_environment_id().await;
        let binding = RunnerModelBinding {
            model_id: model_id.to_string(),
            profile,
            environment_id,
            artifact_root: model_dir.to_path_buf(),
        };
        let mut bindings = self.bindings.write().await;
        let existed = bindings
            .iter()
            .any(|existing| existing.model_id == model_id);
        bindings.retain(|existing| {
            existing.profile.id != binding.profile.id && existing.model_id != binding.model_id
        });
        bindings.push(binding);
        Ok(!existed)
    }

    /// 同步注册表的 ad-hoc 模型改名；catalog Profile ID 不走此路径。
    pub async fn rename_bound_model(&self, old_id: &str, new_id: &str) -> Result<bool, String> {
        let mut bindings = self.bindings.write().await;
        if bindings
            .iter()
            .any(|binding| binding.model_id == new_id && binding.model_id != old_id)
        {
            return Err(format!("Runner model binding '{new_id}' already exists"));
        }
        let Some(binding) = bindings
            .iter_mut()
            .find(|binding| binding.model_id == old_id)
        else {
            return Ok(false);
        };
        binding.model_id = new_id.to_string();
        binding.profile.id = new_id.to_string();
        binding.profile.name = new_id.to_string();
        Ok(true)
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
                    "invalid_request" | "invalid_audio" => AIError::InvalidRequest,
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

#[async_trait]
impl ai_core::provider::ChatProvider for RunnerProvider {
    /// chat.v1 请求：messages 直接承载 conversation；delta 事件映射为逐 token
    /// SSE chunk，usage 由 result 帧回带。无授权输出目录（纯文本协议）。
    async fn chat(
        &self,
        request: ai_core::request::ChatRequest,
    ) -> Result<ai_core::response::ChatResponse, ProviderError> {
        let mut stream = self.chat_stream(request).await?;
        use futures::StreamExt;
        let mut text = String::new();
        let mut usage = None;
        let mut finish_reason = None;
        let mut header = None;
        while let Some(item) = stream.next().await {
            let chunk = item?;
            if header.is_none() {
                header = Some((chunk.id.clone(), chunk.created, chunk.model.clone()));
            }
            for choice in chunk.choices {
                if let Some(content) = choice.delta.content {
                    text.push_str(&content);
                }
                if choice.finish_reason.is_some() {
                    finish_reason = choice.finish_reason;
                }
                if chunk.usage.is_some() {
                    usage = chunk.usage.clone();
                }
            }
        }
        let (id, created, model) = header.ok_or_else(|| {
            ProviderError::new(AIError::Internal, "chat stream produced no chunks")
        })?;
        Ok(ai_core::response::ChatResponse {
            id,
            object: "chat.completion".to_string(),
            created,
            model,
            choices: vec![ai_core::response::ChatChoice {
                index: 0,
                message: ai_core::response::ChatResponseMessage {
                    role: "assistant".to_string(),
                    content: text,
                },
                finish_reason,
            }],
            usage: usage.unwrap_or_default(),
        })
    }

    async fn chat_stream(
        &self,
        request: ai_core::request::ChatRequest,
    ) -> Result<ai_core::provider::ChatStream, ProviderError> {
        if request.messages.is_empty() {
            return Err(ProviderError::new(
                AIError::InvalidRequest,
                "chat request has no messages",
            ));
        }
        let binding = self.binding(&request.model).await?;
        let runner_id = binding.profile.runner.clone();
        let model = request.model.clone();
        let chat_request = json!({
            "messages": request.messages,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
        });
        let (deadline, _) = self.deadlines(&binding)?;
        let instances = self.instances.clone();
        let completion_id = format!("chatcmpl-runner-{}", uuid_like());
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let stream = async_stream::stream! {
            // v1 单实例/单并发：infer 内部持有进程 I/O 锁，天然排队。
            let (tx, mut rx) = futures::channel::mpsc::channel::<
                Result<ai_core::response::ChatChunk, ProviderError>,
            >(8);
            let driver_tx = tx.clone();
            let infer_runner_id = runner_id.clone();
            let infer_model = model.clone();
            let infer_completion = completion_id.clone();
            let driver = tokio::spawn(async move {
                let mut tx = driver_tx;
                let events = instances.infer(
                    &infer_runner_id,
                    "chat.v1",
                    chat_request,
                    json!({}),
                    deadline,
                    |event| match event {
                        crate::runners::InferEvent::Delta(payload) => {
                            let _ = tx.try_send(Ok(chunk_with(
                                &infer_completion,
                                created,
                                &infer_model,
                                payload.get("text").and_then(Value::as_str).unwrap_or_default(),
                                None,
                                None,
                            )));
                        }
                        crate::runners::InferEvent::Result(payload) => {
                            let usage = payload.get("usage").cloned().unwrap_or_else(|| json!({}));
                            let _ = tx.try_send(Ok(chunk_with(
                                &infer_completion,
                                created,
                                &infer_model,
                                "",
                                Some(
                                    payload
                                        .get("finish_reason")
                                        .and_then(Value::as_str)
                                        .unwrap_or("stop")
                                        .to_string(),
                                ),
                                Some(ai_core::response::ChatUsage {
                                    prompt_tokens: usage
                                        .get("prompt_tokens")
                                        .and_then(Value::as_u64)
                                        .unwrap_or(0),
                                    completion_tokens: usage
                                        .get("completion_tokens")
                                        .and_then(Value::as_u64)
                                        .unwrap_or(0),
                                    total_tokens: usage
                                        .get("total_tokens")
                                        .and_then(Value::as_u64)
                                        .unwrap_or(0),
                                }),
                            )));
                        }
                        _ => {}
                    },
                );
                match events.await {
                    Ok(_) => {}
                    Err(error) => {
                        let _ = tx.try_send(Err(RunnerProvider::map_error(error)));
                    }
                }
            });
            drop(tx);
            use futures::StreamExt;
            // OpenAI 流式契约：首个 chunk 带 role，最后一个 chunk 带 finish_reason+usage。
            yield Ok(ai_core::response::ChatChunk {
                id: completion_id.clone(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.clone(),
                choices: vec![ai_core::response::ChatChunkChoice {
                    index: 0,
                    delta: ai_core::response::ChatChunkDelta {
                        role: Some("assistant".to_string()),
                        content: Some(String::new()),
                    },
                    finish_reason: None,
                }],
                usage: None,
            });
            while let Some(item) = rx.next().await {
                yield item;
            }
            if let Err(join_error) = driver.await {
                if !join_error.is_cancelled() && !join_error.is_panic() {
                    yield Err(ProviderError::new(
                        AIError::Internal,
                        format!("chat driver failed: {join_error}"),
                    ));
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

/// 带可选 finish_reason/usage 的流式 chunk 构造。
#[allow(clippy::too_many_arguments)]
fn chunk_with(
    id: &str,
    created: u64,
    model: &str,
    content: &str,
    finish_reason: Option<String>,
    usage: Option<ai_core::response::ChatUsage>,
) -> ai_core::response::ChatChunk {
    ai_core::response::ChatChunk {
        id: id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created,
        model: model.to_string(),
        choices: vec![ai_core::response::ChatChunkChoice {
            index: 0,
            delta: ai_core::response::ChatChunkDelta {
                role: None,
                content: Some(content.to_string()),
            },
            finish_reason,
        }],
        usage,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_reported_load_and_audio_errors_keep_public_semantics() {
        let load = RunnerProvider::map_error(RunnerInstanceError::RunnerReportedError {
            code: "model_load_failed".to_string(),
            message: "cannot load model".to_string(),
        });
        assert_eq!(load.kind, AIError::ModelLoadFailed);
        assert_eq!(load.message, "cannot load model");

        let audio = RunnerProvider::map_error(RunnerInstanceError::RunnerReportedError {
            code: "invalid_audio".to_string(),
            message: "audio is not PCM WAV".to_string(),
        });
        assert_eq!(audio.kind, AIError::InvalidRequest);
        assert_eq!(audio.message, "audio is not PCM WAV");
    }
}
