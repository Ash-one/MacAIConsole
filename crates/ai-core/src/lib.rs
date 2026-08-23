//! ai-core — 共享的核心类型。
//!
//! 包含所有 client（GUI / CLI / API）与 daemon 之间共享的领域模型：
//! Provider 抽象、Capability、ModelSpec、请求/响应结构与统一错误模型。

pub mod errors;
pub mod model;
pub mod provider;
pub mod request;
pub mod response;

pub use errors::{AIError, ApiErrorBody};
pub use model::{ModelSpec, ModelState};
pub use provider::{
    Capability, ChatProvider, ModelHandle, Provider, ProviderHealth, STTProvider, TTSProvider,
};
pub use request::{ChatMessage, ChatRequest, SpeechRequest, TranscriptionRequest};
pub use response::{
    ChatChunk, ChatResponse, ChatUsage, ModelEntry, RuntimeInfo, TranscriptionResponse,
};
