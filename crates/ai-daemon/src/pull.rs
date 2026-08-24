//! HF 模型拉取（handoff §20）。
//!
//! `POST /api/models/pull`：下载 HuggingFace 单文件到模型仓库对应类型
//! 文件夹，完成后立即注册加载。支持断点续传（.part 临时文件 + Range）。

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    /// HF 仓库，如 "Qwen/Qwen2.5-0.5B-Instruct-GGUF"
    pub repo: String,
    /// 仓库内文件路径，如 "qwen2.5-0.5b-instruct-q4_k_m.gguf"
    pub filename: String,
    /// llm / stt / tts
    pub model_type: String,
    /// 注册 ID；缺省用文件名去扩展名
    pub id: Option<String>,
}

/// 校验 repo/filename，防止路径穿越：只允许字母数字与 . _ - /
pub fn validate_pull_parts(repo: &str, filename: &str) -> Result<(), String> {
    let ok = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
            && !s.contains("..")
    };
    if !ok(repo) {
        return Err(format!("invalid repo '{repo}'"));
    }
    if !ok(filename) || filename.starts_with('/') {
        return Err(format!("invalid filename '{filename}'"));
    }
    Ok(())
}

/// 模型仓库根目录（与 GUI 的 ModelRepository 一致）。
pub fn models_dir() -> PathBuf {
    std::env::var("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support/MacAIConsole/Models"))
        .unwrap_or_else(|_| PathBuf::from("Models"))
}

/// 目标文件与 .part 临时文件。
pub fn pull_target(model_type: &str, filename: &str) -> (PathBuf, PathBuf) {
    let dir = models_dir().join(model_type);
    let dest = dir.join(filename.rsplit('/').next().unwrap_or(filename));
    let ext = dest
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let part = if ext.is_empty() {
        dest.with_extension("part")
    } else {
        dest.with_extension(format!("{ext}.part"))
    };
    (dest, part)
}

/// HF resolve URL（跟随 redirect 到 CDN）。
pub fn resolve_url(repo: &str, filename: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/main/{filename}")
}

/// 已存在的字节数（用于断点续传）。目标文件已完整存在时返回 None。
pub fn resume_offset(part: &Path, dest: &Path, expected: Option<u64>) -> u64 {
    if dest.exists() {
        if let (Some(actual), Some(expected)) =
            (std::fs::metadata(dest).ok().map(|m| m.len()), expected)
        {
            if actual == expected {
                return 0;
            }
        }
        // 无期望大小可比时以 dest 存在为准（幂等重入）
        if expected.is_none() && dest.exists() {
            return 0;
        }
    }
    std::fs::metadata(part).map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_paths() {
        assert!(validate_pull_parts("Qwen/Qwen2.5-GGUF", "model/q4_k_m.gguf").is_ok());
        assert!(validate_pull_parts("../etc", "passwd").is_err());
        assert!(validate_pull_parts("ok/repo", "../escape.bin").is_err());
        assert!(validate_pull_parts("", "x.gguf").is_err());
        assert!(validate_pull_parts("r", "/abs/path").is_err());
    }

    #[test]
    fn builds_urls_and_targets() {
        assert_eq!(
            resolve_url("a/b", "c.gguf"),
            "https://huggingface.co/a/b/resolve/main/c.gguf"
        );
        let (dest, part) = pull_target("llm", "m.gguf");
        assert!(dest.ends_with("Models/llm/m.gguf"));
        assert!(part.to_string_lossy().ends_with("m.gguf.part"));
    }
}
