//! 响应类型（OpenAI-compatible，文档 §13–§16、§46）。

use serde::{Deserialize, Serialize};

/// GET /v1/models 中的模型条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub object: String, // "model"
    pub created: u64,
    pub owned_by: String,
    #[serde(rename = "type")]
    pub model_type: String,
    /// 注册时的模型文件路径（OpenAI 兼容扩展字段）。GUI 用它推导原始/默认 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Chat 用量统计（文档 §46）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// POST /v1/chat/completions 非流式响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String, // "chat.completion"
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: ChatUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatResponseMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponseMessage {
    pub role: String,
    pub content: String,
}

/// SSE 流式 chunk（文档 §14、§34）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunk {
    pub id: String,
    pub object: String, // "chat.completion.chunk"
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChunkChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunkChoice {
    pub index: u32,
    pub delta: ChatChunkDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatChunkDelta {
    pub role: Option<String>,
    pub content: Option<String>,
}

/// STT 响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResponse {
    pub text: String,
    pub language: Option<String>,
    pub model: String,
}

/// TTS 响应：音频字节由 HTTP body 承载；这里只描述元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechResponse {
    pub content_type: String,
    pub bytes: u64,
    #[serde(skip)]
    pub audio: Vec<u8>,
}

/// GET /api/runtime 响应（文档 §17、§37）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub version: String,
    pub pid: u32,
    pub uptime_secs: u64,
    pub loaded_models: Vec<LoadedModelInfo>,
    pub active_requests: u64,
    /// AI 内存预算（字节）；None 表示无法探测物理内存。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_budget: Option<u64>,
    /// 物理内存总量（字节），供 GUI 内存压力条使用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_total: Option<u64>,
    /// 当前已使用物理内存（字节），供 GUI 内存压力条使用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_used: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedModelInfo {
    pub id: String,
    pub provider: String,
    pub state: String,
    pub memory_estimate: Option<u64>,
    /// worker 当前 resident memory（字节）；没有常驻 worker 时为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_usage_bytes: Option<u64>,
    pub keep_alive: Option<String>,
    pub loaded_at: Option<u64>,
    pub last_used_at: Option<u64>,
    /// 当前注册规格的上下文长度（LLM 使用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    /// llm / stt / tts —— GUI 右键菜单按类型区分。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_type: Option<String>,
    /// TTS 默认音色。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_voice: Option<String>,
}
