//! HF 模型拉取（handoff §20）。
//!
//! `POST /api/models/pull`：下载 HuggingFace 单文件或目录模型清单到模型仓库，
//! 可在完成后立即注册加载。支持断点续传（.part 临时文件 + Range）。

use std::path::{Path, PathBuf};

use futures::StreamExt;
use reqwest::{header, StatusCode};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

pub const DEFAULT_ENDPOINT: &str = "https://huggingface.co";
const ENDPOINT_ENV: &str = "AIWORKD_HF_ENDPOINT";
const MAX_DOWNLOAD_ATTEMPTS: usize = 3;

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    /// HF 仓库，如 "Qwen/Qwen2.5-0.5B-Instruct-GGUF"
    pub repo: String,
    /// 仓库内文件路径，如 "qwen2.5-0.5b-instruct-q4_k_m.gguf"
    pub filename: Option<String>,
    /// 目录模型所需文件清单；与 directory 一起使用，并保留仓库内相对路径。
    #[serde(default)]
    pub files: Vec<String>,
    /// 目录模型在 Models/<type>/ 下的目标文件夹名。
    pub directory: Option<String>,
    /// llm / stt / tts
    pub model_type: String,
    /// 注册 ID；缺省用文件名去扩展名
    pub id: Option<String>,
    /// 显式 Provider，例如 org.macai.qwen3-asr（Runner）或 sherpa-onnx。
    pub provider: Option<String>,
    /// 下载后是否立即注册加载；默认保持原有 pull 行为。
    pub auto_load: Option<bool>,
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

/// 单文件与目录清单二选一，且所有相对路径都必须通过穿越校验。
pub fn validate_pull_request(request: &PullRequest) -> Result<(), String> {
    match (
        request.filename.as_deref(),
        request.files.is_empty(),
        request.directory.as_deref(),
    ) {
        (Some(filename), true, None) => validate_pull_parts(&request.repo, filename),
        (None, false, Some(directory)) => {
            validate_pull_parts(&request.repo, directory)?;
            if directory.contains('/') {
                return Err("directory must be a single folder name".to_string());
            }
            for filename in &request.files {
                validate_pull_parts(&request.repo, filename)?;
            }
            let unique = request.files.iter().collect::<std::collections::HashSet<_>>();
            if unique.len() != request.files.len() {
                return Err("files must not contain duplicates".to_string());
            }
            Ok(())
        }
        _ => Err(
            "use either filename for a single-file model, or directory + files for a directory model"
                .to_string(),
        ),
    }
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

/// 返回最终模型路径，以及每个 HF 相对路径对应的本地目标。
pub fn pull_targets(request: &PullRequest) -> Result<(PathBuf, Vec<(String, PathBuf)>), String> {
    validate_pull_request(request)?;
    if let Some(filename) = &request.filename {
        let (dest, _) = pull_target(&request.model_type, filename);
        return Ok((dest.clone(), vec![(filename.clone(), dest)]));
    }

    let root = models_dir()
        .join(&request.model_type)
        .join(request.directory.as_deref().expect("validated directory"));
    let targets = request
        .files
        .iter()
        .map(|filename| (filename.clone(), root.join(filename)))
        .collect();
    Ok((root, targets))
}

fn part_path(dest: &Path) -> PathBuf {
    let ext = dest
        .extension()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    if ext.is_empty() {
        dest.with_extension("part")
    } else {
        dest.with_extension(format!("{ext}.part"))
    }
}

/// 校验并规范化 Hugging Face 兼容源地址。
///
/// 源地址只允许 HTTP(S) 的主机和可选路径，避免把查询参数或用户凭据
/// 带入每一个下载请求与日志。
pub fn normalize_endpoint(endpoint: &str) -> Result<String, String> {
    let mut value = endpoint.trim().to_string();
    if value.is_empty() {
        return Err("download endpoint must not be empty".to_string());
    }
    if !value.contains("://") {
        value = format!("https://{value}");
    }
    let parsed = reqwest::Url::parse(&value)
        .map_err(|error| format!("invalid download endpoint '{endpoint}': {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(format!(
            "download endpoint must use http or https and include a host: '{endpoint}'"
        ));
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "download endpoint must not contain credentials, query parameters or fragments"
                .to_string(),
        );
    }
    let mut normalized = parsed.to_string();
    while normalized.ends_with('/') {
        normalized.pop();
    }
    Ok(normalized)
}

/// 返回 daemon 当前配置的下载源。未设置环境变量时使用官方源。
pub fn configured_endpoint() -> Result<String, String> {
    match std::env::var(ENDPOINT_ENV) {
        Ok(value) if !value.trim().is_empty() => normalize_endpoint(&value),
        _ => Ok(DEFAULT_ENDPOINT.to_string()),
    }
}

/// 组装下载 URL。huggingface.co（含 hf-mirror 等）与 modelscope.cn 的
/// resolve 分支不同：HF 用 `resolve/main`，ModelScope 用 `resolve/master`。
/// ModelScope 作为镜像源时，endpoint 填 `https://modelscope.cn/models`。
pub fn resolve_url_with_endpoint(endpoint: &str, repo: &str, filename: &str) -> String {
    let branch = if endpoint.contains("modelscope.cn") {
        "resolve/master"
    } else {
        "resolve/main"
    };
    format!(
        "{}/{repo}/{branch}/{filename}",
        endpoint.trim_end_matches('/')
    )
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

#[derive(Debug)]
struct DownloadError {
    message: String,
    retryable: bool,
}

impl DownloadError {
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }
}

fn content_range_total(value: &header::HeaderValue) -> Option<u64> {
    let value = value.to_str().ok()?;
    let total = value.rsplit_once('/')?.1;
    (total != "*").then(|| total.parse().ok()).flatten()
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        // Large HF files have previously failed with an HTTP/2 response-body
        // decoding error. Keep this transfer path on HTTP/1.1, then retry from
        // the durable .part offset if the body still ends unexpectedly.
        .http1_only()
        // 稳定 UA：ModelScope LFS CDN 拒绝空 UA（denied by UA ACL = blacklist）。
        .user_agent("MacAI/0.1 aiworkd")
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(120))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| format!("http client: {error}"))
}

/// 流式下载一个 HF 文件。目标旁保留 `.part`，网络中断后可续传；响应体
/// 出错时自动重试，并从最新的 `.part` 大小继续。
pub async fn download_file(
    endpoint: &str,
    repo: &str,
    filename: &str,
    dest: &Path,
) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("cannot create model dir {}: {error}", parent.display()))?;
    }
    let endpoint = normalize_endpoint(endpoint)?;
    let client = build_client()?;
    let mut last_error = None;
    for attempt in 1..=MAX_DOWNLOAD_ATTEMPTS {
        match download_file_once(&client, &endpoint, repo, filename, dest).await {
            Ok(()) => return Ok(()),
            Err(error) if error.retryable && attempt < MAX_DOWNLOAD_ATTEMPTS => {
                tracing::warn!(
                    filename,
                    attempt,
                    max_attempts = MAX_DOWNLOAD_ATTEMPTS,
                    error = %error.message,
                    "download interrupted; retrying from .part"
                );
                last_error = Some(error.message);
                tokio::time::sleep(std::time::Duration::from_secs(attempt as u64 * 2)).await;
            }
            Err(error) => {
                let suffix = if error.retryable {
                    format!(" after {MAX_DOWNLOAD_ATTEMPTS} attempts")
                } else {
                    String::new()
                };
                return Err(format!("{}{suffix}", error.message));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "download failed".to_string()))
}

async fn download_file_once(
    client: &reqwest::Client,
    endpoint: &str,
    repo: &str,
    filename: &str,
    dest: &Path,
) -> Result<(), DownloadError> {
    let part = part_path(dest);
    let url = resolve_url_with_endpoint(endpoint, repo, filename);
    let expected_len =
        match tokio::time::timeout(std::time::Duration::from_secs(30), client.head(&url).send())
            .await
        {
            Ok(Ok(response)) if response.status().is_success() => response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok()),
            _ => None,
        };

    if dest.exists()
        && expected_len.is_some_and(|expected| {
            std::fs::metadata(dest).map(|value| value.len()).ok() == Some(expected)
        })
    {
        tracing::info!(dest = %dest.display(), "pull target already complete");
        return Ok(());
    }

    let mut offset = resume_offset(&part, dest, expected_len);
    if expected_len.is_some_and(|expected| offset > expected) {
        tokio::fs::remove_file(&part).await.map_err(|error| {
            DownloadError::permanent(format!("cannot reset '{}': {error}", part.display()))
        })?;
        offset = 0;
    }
    if offset > 0 && expected_len == Some(offset) {
        tokio::fs::rename(&part, dest).await.map_err(|error| {
            DownloadError::permanent(format!("rename failed for '{}': {error}", dest.display()))
        })?;
        tracing::info!(filename, dest = %dest.display(), bytes = offset, "pull complete");
        return Ok(());
    }
    let mut get = client.get(&url);
    if offset > 0 {
        get = get.header(header::RANGE, format!("bytes={offset}-"));
        tracing::info!(filename, offset, "resuming pull");
    }

    let response = get.send().await.map_err(|error| {
        DownloadError::retryable(format!("download failed for '{filename}': {error}"))
    })?;
    if !response.status().is_success() {
        let retryable = matches!(
            response.status(),
            StatusCode::REQUEST_TIMEOUT
                | StatusCode::TOO_MANY_REQUESTS
                | StatusCode::INTERNAL_SERVER_ERROR
                | StatusCode::BAD_GATEWAY
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::GATEWAY_TIMEOUT
        );
        return Err(if retryable {
            DownloadError::retryable(format!("HF returned {} for {url}", response.status()))
        } else {
            DownloadError::permanent(format!("HF returned {} for {url}", response.status()))
        });
    }
    let append = offset > 0 && response.status() == StatusCode::PARTIAL_CONTENT;
    if !append {
        offset = 0;
    }
    let total = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(content_range_total)
        .or_else(|| {
            response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(|length| length + if append { offset } else { 0 })
        })
        .or(expected_len);
    let mut file = if append {
        tokio::fs::OpenOptions::new().append(true).open(&part).await
    } else {
        let _ = tokio::fs::remove_file(&part).await;
        tokio::fs::File::create(&part).await
    }
    .map_err(|error| {
        DownloadError::permanent(format!("cannot open '{}': {error}", part.display()))
    })?;

    let mut stream = response.bytes_stream();
    let mut downloaded = offset;
    let mut last_report = downloaded;
    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = file.flush().await;
                return Err(DownloadError::retryable(format!(
                    "download interrupted for '{filename}' at {downloaded} bytes: {error}"
                )));
            }
        };
        file.write_all(&bytes).await.map_err(|error| {
            DownloadError::permanent(format!("write failed for '{}': {error}", part.display()))
        })?;
        downloaded += bytes.len() as u64;
        if downloaded - last_report >= 16 * 1024 * 1024 {
            tracing::info!(
                filename,
                downloaded,
                total = total.unwrap_or(0),
                "pull progress"
            );
            last_report = downloaded;
        }
    }
    file.flush().await.map_err(|error| {
        DownloadError::permanent(format!("flush failed for '{}': {error}", part.display()))
    })?;
    if let Some(total) = total {
        if downloaded != total {
            return Err(DownloadError::retryable(format!(
                "size mismatch for '{filename}': got {downloaded}, expected {total}"
            )));
        }
    }
    tokio::fs::rename(&part, dest).await.map_err(|error| {
        DownloadError::permanent(format!("rename failed for '{}': {error}", dest.display()))
    })?;
    tracing::info!(filename, dest = %dest.display(), bytes = downloaded, "pull complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
            resolve_url_with_endpoint(DEFAULT_ENDPOINT, "a/b", "c.gguf"),
            "https://huggingface.co/a/b/resolve/main/c.gguf"
        );
        assert_eq!(
            resolve_url_with_endpoint("https://hf-mirror.com/", "a/b", "c.gguf"),
            "https://hf-mirror.com/a/b/resolve/main/c.gguf"
        );
        let (dest, part) = pull_target("llm", "m.gguf");
        assert!(dest.ends_with("Models/llm/m.gguf"));
        assert!(part.to_string_lossy().ends_with("m.gguf.part"));
    }

    #[test]
    fn validates_and_normalizes_download_endpoints() {
        assert_eq!(
            normalize_endpoint(" hf-mirror.com/ ").unwrap(),
            "https://hf-mirror.com"
        );
        assert_eq!(
            normalize_endpoint("https://mirror.example/hf/").unwrap(),
            "https://mirror.example/hf"
        );
        assert!(normalize_endpoint("ftp://mirror.example").is_err());
        assert!(normalize_endpoint("https://user:pass@mirror.example").is_err());
        assert!(normalize_endpoint("https://mirror.example?token=secret").is_err());
    }

    #[test]
    fn builds_directory_model_targets_and_rejects_mixed_shapes() {
        let request = PullRequest {
            repo: "mlx-community/Qwen3-ASR-0.6B-8bit".to_string(),
            filename: None,
            files: vec![
                "config.json".to_string(),
                "voices/zf_001.safetensors".to_string(),
            ],
            directory: Some("qwen3-asr-mlx-8bit".to_string()),
            model_type: "stt".to_string(),
            id: Some("qwen3-asr-mlx-8bit".to_string()),
            provider: Some("qwen3-asr-mlx".to_string()),
            auto_load: Some(true),
        };
        let (root, targets) = pull_targets(&request).unwrap();
        assert!(root.ends_with("Models/stt/qwen3-asr-mlx-8bit"));
        assert!(targets[1]
            .1
            .ends_with("qwen3-asr-mlx-8bit/voices/zf_001.safetensors"));

        let mut invalid = request;
        invalid.filename = Some("model.bin".to_string());
        assert!(validate_pull_request(&invalid).is_err());
    }

    fn directory_request(files: Vec<&str>, directory: &str) -> PullRequest {
        PullRequest {
            repo: "r".to_string(),
            filename: None,
            files: files.into_iter().map(str::to_string).collect(),
            directory: Some(directory.to_string()),
            model_type: "llm".to_string(),
            id: None,
            provider: None,
            auto_load: None,
        }
    }

    #[test]
    fn rejects_ambiguous_or_unsafe_request_shapes() {
        // 清单重复会触发重复下载与相互覆盖。
        let duplicate = directory_request(vec!["a.bin", "a.bin"], "dir");
        assert!(validate_pull_request(&duplicate).is_err());

        // 目录名必须是一层文件夹名，携带子路径会让目标目录逃出仓库根。
        let nested = directory_request(vec!["a.bin"], "a/b");
        assert!(validate_pull_request(&nested).is_err());

        // 只有 directory 而没有文件清单同样无效。
        let no_files = directory_request(vec![], "dir");
        assert!(validate_pull_request(&no_files).is_err());

        // 目录清单内的单个文件也不能穿越。
        let mut traversal = directory_request(vec!["../escape.bin"], "dir");
        assert!(validate_pull_request(&traversal).is_err());
        traversal.files = vec!["ok.bin".to_string()];
        traversal.repo = "../repo".to_string();
        assert!(validate_pull_request(&traversal).is_err());
    }

    #[test]
    fn pull_target_part_naming_handles_extensionless_and_multi_dot_files() {
        let (dest, part) = pull_target("llm", "LICENSE");
        assert!(dest.ends_with("Models/llm/LICENSE"));
        assert!(part.to_string_lossy().ends_with("LICENSE.part"));

        let (dest, part) = pull_target("llm", "nested/model.tar.gz");
        assert!(dest.ends_with("Models/llm/model.tar.gz"));
        assert!(part.to_string_lossy().ends_with("model.tar.gz.part"));
    }

    #[tokio::test]
    async fn download_file_resumes_after_interrupted_response_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut interrupted = false;
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                loop {
                    let read = socket.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                if request.starts_with("HEAD ") {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .unwrap();
                    continue;
                }
                assert!(request.starts_with("GET "));
                if !interrupted {
                    interrupted = true;
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n12345",
                        )
                        .await
                        .unwrap();
                    continue;
                }
                assert!(request.to_ascii_lowercase().contains("range: bytes=5-"));
                socket
                    .write_all(
                        b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\nContent-Range: bytes 5-9/10\r\nConnection: close\r\n\r\n67890",
                    )
                    .await
                    .unwrap();
                return;
            }
        });

        let root = std::env::temp_dir().join(format!(
            "macai-pull-resume-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&root);
        let dest = root.join("model.bin");
        let result = download_file(&endpoint, "repo", "model.bin", &dest).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"1234567890");
        server.await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resume_offset_skips_complete_dest_and_else_resumes_from_part() {
        let dir = std::env::temp_dir().join(format!("macai-pull-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("dest.bin");
        let part = dir.join("dest.bin.part");

        // 没有 .part 时从头下载。
        assert_eq!(resume_offset(&part, &dest, Some(100)), 0);

        // .part 已有 40 字节 → 续传偏移 40。
        std::fs::write(&part, vec![0u8; 40]).unwrap();
        assert_eq!(resume_offset(&part, &dest, Some(100)), 40);

        // dest 已完整存在（大小匹配期望）→ 幂等重入，不续传。
        std::fs::write(&dest, vec![0u8; 100]).unwrap();
        assert_eq!(resume_offset(&part, &dest, Some(100)), 0);

        // 无期望大小时以 dest 存在为准。
        assert_eq!(resume_offset(&part, &dest, None), 0);

        // dest 大小与期望不符（异常状态）→ 以 .part 为准继续续传。
        std::fs::write(&dest, vec![0u8; 10]).unwrap();
        assert_eq!(resume_offset(&part, &dest, Some(100)), 40);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
