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
    /// 期望使用的 provider：auto / mlx / llama.cpp / mock ...
    pub provider: String,
    pub source: Option<String>,
    pub path: Option<String>,
    pub format: Option<String>,
    pub size_bytes: Option<u64>,
    pub memory_estimate: Option<u64>,
    /// keep_alive 语义：0 / 5m / 30m / always
    pub keep_alive: Option<String>,
    pub context_length: Option<u64>,
}

impl ModelSpec {
    /// 解析后的 keep_alive 时长（秒）。None 表示 always（不自动卸载）。
    pub fn keep_alive_secs(&self) -> Option<u64> {
        match self.keep_alive.as_deref() {
            None | Some("always") => None,
            Some("0") => Some(0),
            Some(s) => parse_duration(s),
        }
    }
}

/// 解析 "5m" / "30s" / "2h" 形式的时长字符串。
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("ms") {
        return v.parse::<u64>().ok().map(|n| n / 1000);
    }
    if let Some(v) = s.strip_suffix('s') {
        return v.parse::<u64>().ok();
    }
    if let Some(v) = s.strip_suffix('m') {
        return v.parse::<u64>().ok().map(|n| n * 60);
    }
    if let Some(v) = s.strip_suffix('h') {
        return v.parse::<u64>().ok().map(|n| n * 3600);
    }
    s.parse::<u64>().ok()
}

/// 模型生命周期状态（文档 §22）。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_durations() {
        assert_eq!(parse_duration("30s"), Some(30));
        assert_eq!(parse_duration("5m"), Some(300));
        assert_eq!(parse_duration("2h"), Some(7200));
        assert_eq!(parse_duration("1500ms"), Some(1));
        assert_eq!(parse_duration("invalid"), None);
    }

    #[test]
    fn always_keep_alive_has_no_deadline() {
        let spec = ModelSpec {
            id: "mock".to_string(),
            name: "Mock".to_string(),
            model_type: "llm".to_string(),
            provider: "mock".to_string(),
            source: None,
            path: None,
            format: Some("mock".to_string()),
            size_bytes: None,
            memory_estimate: Some(0),
            keep_alive: Some("always".to_string()),
            context_length: Some(4096),
        };

        assert_eq!(spec.keep_alive_secs(), None);
    }
}
