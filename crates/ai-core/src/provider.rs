//! Provider 抽象（文档 §7）——项目最重要的 abstraction。
//!
//! 所有推理 engine（llama.cpp / MLX / whisper / mock）必须实现统一的 `Provider`
//! interface，再按能力补上 `ChatProvider` / `STTProvider` / `TTSProvider`。

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
}

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

/// 统一 Provider interface（文档 §7 原文）。
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;

    fn capabilities(&self) -> Vec<Capability>;

    async fn load(&self, model: &ModelSpec) -> Result<ModelHandle, AIError>;

    async fn unload(&self, handle: &ModelHandle) -> Result<(), AIError>;

    async fn health_check(&self) -> Result<ProviderHealth, AIError>;
}

/// Chat 流式输出：Provider 产生 chunk 流，daemon 转发为 SSE（文档 §34）。
pub type ChatStream = Pin<Box<dyn Stream<Item = ChatChunk> + Send>>;

/// Chat 能力（文档 §7、§14、§34）。
#[async_trait]
pub trait ChatProvider: Provider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AIError>;

    /// 流式版本。默认实现：把非流式结果包装成单 chunk 流。
    /// 真实 Provider（llama.cpp / MLX）应覆盖为逐 token 流。
    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, AIError> {
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
                    },
                    finish_reason: c.finish_reason,
                })
                .collect(),
        };
        Ok(Box::pin(futures::stream::once(async { chunk })))
    }
}

/// STT 能力（文档 §7）。
#[async_trait]
pub trait STTProvider: Provider {
    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, AIError>;
}

/// TTS 能力（文档 §7）。
#[async_trait]
pub trait TTSProvider: Provider {
    async fn synthesize(&self, request: SpeechRequest) -> Result<SpeechResponse, AIError>;
}
