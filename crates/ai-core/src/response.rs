//! 响应类型（OpenAI-compatible，文档 §13–§16、§46）。

use serde::{Deserialize, Serialize};

use crate::request::ChatMessage;

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
    /// 终帧的用量统计（OpenAI stream_options.include_usage 语义）；
    /// 其余 chunk 恒为 None 且不出现在 wire 上。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatUsage>,
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

/// 任务列表中的轻量摘要。时间戳统一使用 Unix 毫秒。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    /// chat / stt / tts；保留字符串以便未来扩展而不破坏客户端解码。
    pub kind: String,
    /// running / succeeded / failed / cancelled；保留字符串以便未来扩展。
    pub status: String,
    pub model: String,
    pub provider: Option<String>,
    pub input_preview: String,
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    /// STT 输入音频时长（毫秒）；其他任务类型或无法解析时为空。
    pub audio_duration_ms: Option<u64>,
    pub error: Option<String>,
}

/// 任务详情中的请求信息。音频本体和临时文件路径永远不进入该结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskRequestDetail {
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub input_text: Option<String>,
    pub file_name: Option<String>,
    pub file_size_bytes: Option<u64>,
    /// STT 输入音频时长（毫秒），由 daemon 从 WAV 元数据计算。
    pub audio_duration_ms: Option<u64>,
    pub language: Option<String>,
    pub voice: Option<String>,
    pub format: Option<String>,
    pub speed: Option<f64>,
    pub stream: Option<bool>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
}

/// 任务详情中的结果信息。TTS 只记录元数据，不保存音频字节。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskResultDetail {
    pub output_text: Option<String>,
    pub language: Option<String>,
    pub finish_reason: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    /// 生成吞吐（completion_tokens / 任务总时长秒），由 daemon 在任务完成时统一计算。
    pub tokens_per_second: Option<f64>,
    pub content_type: Option<String>,
    pub byte_count: Option<u64>,
}

/// GET /api/tasks/{id} 的完整任务记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDetail {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub model: String,
    pub provider: Option<String>,
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub request: TaskRequestDetail,
    pub result: TaskResultDetail,
    pub request_truncated: bool,
    pub result_truncated: bool,
    pub error: Option<String>,
}

impl TaskDetail {
    pub fn summary(&self) -> TaskSummary {
        TaskSummary {
            id: self.id.clone(),
            kind: self.kind.clone(),
            status: self.status.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            input_preview: task_input_preview(&self.kind, &self.request),
            started_at_ms: self.started_at_ms,
            completed_at_ms: self.completed_at_ms,
            duration_ms: self.duration_ms,
            audio_duration_ms: self.request.audio_duration_ms,
            error: self.error.clone(),
        }
    }
}

fn task_input_preview(kind: &str, request: &TaskRequestDetail) -> String {
    let text = match kind {
        "chat" => request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .or_else(|| request.messages.last())
            .map(|message| message.content.as_str())
            .unwrap_or(""),
        "stt" => request.file_name.as_deref().unwrap_or(""),
        "tts" => request.input_text.as_deref().unwrap_or(""),
        _ => request
            .input_text
            .as_deref()
            .or_else(|| request.file_name.as_deref())
            .unwrap_or(""),
    };
    let mut preview: String = text.chars().take(117).collect();
    if text.chars().count() > 117 {
        preview.push('…');
    }
    preview
}

/// GET /api/tasks 的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskListResponse {
    pub running: Vec<TaskSummary>,
    pub completed: Vec<TaskSummary>,
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
    /// 当前生效的加速设备（coreml / metal / gpu / cpu）；未探测到时为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_device: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::ChatMessage;

    fn detail(kind: &str) -> TaskDetail {
        TaskDetail {
            id: "req".to_string(),
            kind: kind.to_string(),
            status: "succeeded".to_string(),
            model: "m".to_string(),
            provider: None,
            started_at_ms: 1_000,
            completed_at_ms: Some(2_500),
            duration_ms: Some(1_500),
            request: TaskRequestDetail::default(),
            result: TaskResultDetail::default(),
            request_truncated: false,
            result_truncated: false,
            error: None,
        }
    }

    #[test]
    fn chat_preview_prefers_last_user_message() {
        let mut d = detail("chat");
        d.request.messages = vec![
            ChatMessage {
                role: "user".into(),
                content: "第一问".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "回答".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "第二问".into(),
            },
        ];
        assert_eq!(d.summary().input_preview, "第二问");

        // 对话里没有 user 消息时回退到最后一条消息。
        d.request.messages = vec![
            ChatMessage {
                role: "system".into(),
                content: "系统提示".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "回复".into(),
            },
        ];
        assert_eq!(d.summary().input_preview, "回复");
    }

    #[test]
    fn stt_and_tts_previews_use_their_own_request_fields() {
        let mut stt = detail("stt");
        stt.request.file_name = Some("meeting.wav".into());
        stt.request.audio_duration_ms = Some(15_700);
        let summary = stt.summary();
        assert_eq!(summary.input_preview, "meeting.wav");
        assert_eq!(summary.audio_duration_ms, Some(15_700));

        let mut tts = detail("tts");
        tts.request.input_text = Some("朗读这段话".into());
        let summary = tts.summary();
        assert_eq!(summary.input_preview, "朗读这段话");
        assert_eq!(summary.audio_duration_ms, None);
    }

    #[test]
    fn unknown_kind_preview_falls_back_to_text_then_file() {
        // 任务列表按字符串透传 kind；未知类型不能丢预览。
        let mut d = detail("rerank");
        d.request.input_text = Some("候选文本".into());
        assert_eq!(d.summary().input_preview, "候选文本");

        d.request.input_text = None;
        d.request.file_name = Some("query.txt".into());
        assert_eq!(d.summary().input_preview, "query.txt");
    }

    #[test]
    fn preview_truncates_by_chars_not_bytes_and_appends_ellipsis() {
        let mut d = detail("tts");
        // 120 个 CJK 字符：按字节截断会切出非法 UTF-8，必须按字符数截断。
        d.request.input_text = Some("汉".repeat(120));
        let preview = d.summary().input_preview;
        let chars: Vec<char> = preview.chars().collect();
        assert_eq!(chars.len(), 118);
        assert!(chars[..117].iter().all(|&c| c == '汉'));
        assert_eq!(chars[117], '…');

        // 恰好 117 字符不追加省略号。
        d.request.input_text = Some("汉".repeat(117));
        assert_eq!(d.summary().input_preview, "汉".repeat(117));
    }

    #[test]
    fn summary_carries_identity_timestamps_and_error() {
        let mut d = detail("chat");
        d.provider = Some("llama.cpp".into());
        d.error = Some("worker crashed".into());
        let summary = d.summary();
        assert_eq!(summary.id, "req");
        assert_eq!(summary.kind, "chat");
        assert_eq!(summary.status, "succeeded");
        assert_eq!(summary.provider.as_deref(), Some("llama.cpp"));
        assert_eq!(summary.started_at_ms, 1_000);
        assert_eq!(summary.completed_at_ms, Some(2_500));
        assert_eq!(summary.duration_ms, Some(1_500));
        assert_eq!(summary.error.as_deref(), Some("worker crashed"));
    }
}
