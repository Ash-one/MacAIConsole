//! Provider 抽象（文档 §7）——项目最重要的 abstraction。
//!
//! 所有推理 engine（llama.cpp / MLX / whisper / mock）必须实现统一的 `Provider`
//! interface，再按能力补上 `ChatProvider` / `STTProvider` / `TTSProvider`。

use std::fmt;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::model::ModelSpec;
use crate::request::{ChatRequest, SpeechRequest, TranscriptionRequest};
use crate::response::{ChatChunk, ChatResponse, SpeechResponse, TranscriptionResponse};
use crate::AIError;

/// 模型能力标签（文档 §7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Chat,
    Completion,
    Vision,
    Embedding,
    SpeechToText,
    TextToSpeech,
    Rerank,
    ImageGeneration,
}

impl Capability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::Chat => "chat",
            Capability::Completion => "completion",
            Capability::Vision => "vision",
            Capability::Embedding => "embedding",
            Capability::SpeechToText => "speech_to_text",
            Capability::TextToSpeech => "text_to_speech",
            Capability::Rerank => "rerank",
            Capability::ImageGeneration => "image_generation",
        }
    }

    /// 版本化 capability 契约名（Model Profile / Runner manifest 使用，
    /// 如 `chat.v1`）。与 `as_str` 的区别：这是跨进程协商的协议名。
    pub fn profile_name(&self) -> &'static str {
        match self {
            Capability::Chat => "chat.v1",
            Capability::SpeechToText => "stt.v1",
            Capability::TextToSpeech => "tts.v1",
            other => other.as_str(),
        }
    }
}

/// Provider 的故障隔离边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode {
    InProcess,
    Worker,
}

/// 不触发模型加载的静态能力描述。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    pub id: String,
    pub capabilities: Vec<Capability>,
    pub isolation: IsolationMode,
    pub supported_devices: Vec<String>,
}

/// Provider 在当前主机上的实时状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub available: bool,
    pub ready: bool,
    pub effective_device: Option<String>,
    pub resident_models: Vec<String>,
    pub reason: Option<String>,
    pub install_hint: Option<String>,
}

/// 带机器可判定类别与人类可读上下文的 Provider 错误。
#[derive(Debug, Clone)]
pub struct ProviderError {
    pub kind: AIError,
    pub message: String,
}

impl ProviderError {
    pub fn new(kind: AIError, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl From<AIError> for ProviderError {
    fn from(kind: AIError) -> Self {
        Self::new(kind, kind.as_str())
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ProviderError {}

/// 已加载模型的句柄。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelHandle {
    pub model_id: String,
    pub provider_id: String,
}

/// Provider 健康状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub ok: bool,
    pub message: Option<String>,
}

/// 统一 Provider interface（文档 §7）。
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;

    fn capabilities(&self) -> Vec<Capability>;

    fn descriptor(&self) -> ProviderDescriptor;

    async fn status(&self) -> ProviderStatus;

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, ProviderError>;

    async fn unload(&self, handle: &ModelHandle) -> Result<(), ProviderError>;

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError>;

    /// 当前 worker 的 resident memory（字节）。没有常驻 worker 时返回 None。
    async fn memory_usage_bytes(&self) -> Option<u64> {
        None
    }

    /// 当前生效的加速设备（"coreml" / "metal" / "gpu" / "cpu"）。
    /// 没有常驻 worker 或尚未探测到时返回 None。
    async fn effective_device(&self) -> Option<String> {
        None
    }
}

/// Chat 流式输出：Provider 产生 chunk 流，daemon 转发为 SSE（文档 §34）。
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>;

/// Chat 能力（文档 §7、§14、§34）。
#[async_trait]
pub trait ChatProvider: Provider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError>;

    /// 流式版本。默认实现：把非流式结果包装成单 chunk 流。
    /// 真实 Provider（llama.cpp / MLX）应覆盖为逐 token 流。
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, ProviderError> {
        let resp = self.chat(request).await?;
        let chunk = ChatChunk {
            id: resp.id,
            object: "chat.completion.chunk".to_string(),
            created: resp.created,
            model: resp.model,
            choices: resp
                .choices
                .into_iter()
                .map(|c| crate::response::ChatChunkChoice {
                    index: c.index,
                    delta: crate::response::ChatChunkDelta {
                        role: Some(c.message.role),
                        content: Some(c.message.content),
                        reasoning_content: c.message.reasoning_content,
                    },
                    finish_reason: c.finish_reason,
                })
                .collect(),
            usage: None,
        };
        Ok(Box::pin(futures::stream::once(async { Ok(chunk) })))
    }
}

/// STT 能力（文档 §7）。
#[async_trait]
pub trait STTProvider: Provider {
    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError>;
}

/// TTS 能力（文档 §7）。
#[async_trait]
pub trait TTSProvider: Provider {
    async fn synthesize(&self, request: SpeechRequest) -> Result<SpeechResponse, ProviderError>;
}
