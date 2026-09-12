//! 请求类型（OpenAI-compatible + 内部扩展，文档 §14–§16）。

use serde::{Deserialize, Serialize};

/// Chat 消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// OpenAI-compatible 推理扩展；输入兼容部分框架使用的 `reasoning` 名称。
    #[serde(default, alias = "reasoning", skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
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
    pub top_p: Option<f64>,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// OpenAI 客户端常省略可选字段；容器级 `#[serde(default)]` 保证
    /// 缺字段（甚至空对象）的请求仍可解码，这里是兼容性契约。
    #[test]
    fn partial_json_decodes_with_defaults() {
        let chat: ChatRequest = serde_json::from_str(r#"{"model": "qwen3"}"#).unwrap();
        assert_eq!(chat.model, "qwen3");
        assert!(chat.messages.is_empty());
        assert!(!chat.stream);
        assert!(chat.temperature.is_none());
        assert!(chat.top_p.is_none());
        assert!(chat.max_tokens.is_none());

        let speech: SpeechRequest =
            serde_json::from_str(r#"{"model": "kokoro", "input": "你好"}"#).unwrap();
        assert_eq!(speech.input, "你好");
        assert!(speech.voice.is_none());
        assert!(speech.format.is_none());
        assert!(speech.speed.is_none());

        let transcription: TranscriptionRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(transcription.model, "");
        assert!(transcription.file.is_none());
    }

    #[test]
    fn chat_message_accepts_reasoning_alias_but_serializes_canonical_name() {
        let message: ChatMessage =
            serde_json::from_str(r#"{"role":"assistant","content":"answer","reasoning":"why"}"#)
                .unwrap();
        assert_eq!(message.reasoning_content.as_deref(), Some("why"));
        let encoded = serde_json::to_value(message).unwrap();
        assert_eq!(encoded["reasoning_content"], "why");
        assert!(encoded.get("reasoning").is_none());
    }
}
