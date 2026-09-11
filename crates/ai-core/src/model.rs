//! 模型描述与生命周期状态（文档 §19、§22）。

use serde::{Deserialize, Serialize};

/// 模型规格。来自 model.toml manifest（文档 §21）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    pub id: String,
    pub name: String,
    /// llm / stt / tts / embedding ...
    #[serde(rename = "type")]
    pub model_type: String,
    /// daemon 裁决后的 provider ID。
    pub provider: String,
    /// 调用者注册时请求的 provider；`auto` 表示使用 daemon 缺省裁决。
    /// None 仅用于读取早于选择审计字段的外部序列化数据。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_provider: Option<String>,
    /// daemon 为何选择 `provider` 的稳定、可审计说明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_selection_reason: Option<String>,
    pub source: Option<String>,
    pub path: Option<String>,
    pub format: Option<String>,
    pub size_bytes: Option<u64>,
    pub memory_estimate: Option<u64>,
    /// keep_alive 语义：0 / 5m / 30m / always
    pub keep_alive: Option<String>,
    pub context_length: Option<u64>,
    /// LLM 请求未显式指定时使用的采样温度。
    #[serde(default)]
    pub temperature: Option<f64>,
    /// LLM 请求未显式指定时使用的 nucleus sampling 阈值。
    #[serde(default)]
    pub top_p: Option<f64>,
    /// TTS 默认音色（如 zf_001）。None 时 provider 用自己的内置缺省。
    #[serde(default)]
    pub default_voice: Option<String>,
}

/// 模型生命周期状态（文档 §22）。keep_alive 字符串解析的唯一 owner 是
/// ai-daemon::scheduler::parse_keep_alive（handoff §71）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelState {
    Unloaded,
    Loading,
    Ready,
    Busy,
    Idle,
    Unloading,
    Failed,
}

impl ModelState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelState::Unloaded => "unloaded",
            ModelState::Loading => "loading",
            ModelState::Ready => "ready",
            ModelState::Busy => "busy",
            ModelState::Idle => "idle",
            ModelState::Unloading => "unloading",
            ModelState::Failed => "failed",
        }
    }
}
