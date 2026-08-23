//! 请求类型（OpenAI-compatible + 内部扩展，文档 §14–§16）。

use serde::{Deserialize, Serialize};

/// Chat 消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// POST /v1/chat/completions 请求（文档 §14）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    /// 必须支持 stream=true（文档 §14：Streaming 是第一版必需功能）。
    pub stream: bool,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
}

/// POST /v1/audio/transcriptions 请求（文档 §15）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionRequest {
    pub model: String,
    /// 文件路径（第一阶段由 daemon 本地读取）。
    pub file: Option<String>,
    pub language: Option<String>,
    pub response_format: Option<String>,
}

/// POST /v1/audio/speech 请求（文档 §16）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeechRequest {
    pub model: String,
    pub input: String,
    pub voice: Option<String>,
    pub format: Option<String>, // wav / mp3 ...
    pub speed: Option<f64>,
}
