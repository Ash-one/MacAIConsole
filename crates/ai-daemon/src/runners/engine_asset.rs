//! 引擎预编译产物安装（cpp 引擎 Runner 化）。
//!
//! manifest `[engine]` 声明的产物由 daemon 在 Runner install 流程中下载：
//! sha256 强制校验 → 系统 `tar -xzf` 解压到 staging → 提交产物内二进制存在
//! → 原子 rename 到受管 Engines 目录。GUI「运行环境」区块与 CLI 均经
//! `POST /api/runners/{id}/install` 触达；适配器经
//! `~/Library/Application Support/MacAIConsole/Engines/<runner-id>/` 定位。
//!
//! 本地产物存在即幂等跳过（引擎升级 = 改 manifest 的 `[engine]` 表，daemon
//! 按 URL+sha256 指纹重建目录）。

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// 受管引擎产物根：`<app_support>/Engines/<runner-id>/`。
pub fn engines_root(app_support: &Path) -> PathBuf {
    app_support.join("Engines")
}

/// 引擎产物就绪目录（幂等判断 + 适配器定位约定）。
pub fn engine_dir(app_support: &Path, runner_id: &str) -> PathBuf {
    engines_root(app_support).join(runner_id)
}

pub fn engine_binary_path(app_support: &Path, runner_id: &str, binary: &str) -> PathBuf {
    engine_dir(app_support, runner_id).join(binary)
}

#[derive(Debug)]
pub enum EngineInstallError {
    /// 下载或校验失败；保留底层错误链供 UI 与日志诊断。
    Download(String),
    Checksum {
        expected: String,
        actual: String,
    },
    Io {
        context: String,
        source: std::io::Error,
    },
    Extract(String),
    BinaryMissing {
        path: PathBuf,
    },
}

impl std::fmt::Display for EngineInstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Download(message) => write!(formatter, "engine asset download failed: {message}"),
            Self::Checksum { expected, actual } => write!(
                formatter,
                "engine asset checksum mismatch: expected {expected}, got {actual}"
            ),
            Self::Io { context, source } => write!(formatter, "{context}: {source}"),
            Self::Extract(message) => {
                write!(formatter, "engine asset extraction failed: {message}")
            }
            Self::BinaryMissing { path } => {
                write!(
                    formatter,
                    "engine archive lacks binary at '{}'",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for EngineInstallError {}

/// 确保引擎产物就绪，返回最终二进制路径。已就绪（checksum 指纹目录存在）
/// 时幂等跳过。
pub async fn ensure_engine_asset(
    app_support: &Path,
    runner_id: &str,
    asset: &crate::runners::EngineAsset,
    client: &reqwest::Client,
) -> Result<PathBuf, EngineInstallError> {
    let dir = engine_dir(app_support, runner_id);
    let fingerprint_file = dir.join(".macai-engine.json");
    if let Ok(existing) = std::fs::read_to_string(&fingerprint_file) {
        if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&existing) {
            if metadata.get("sha256").and_then(serde_json::Value::as_str)
                == Some(asset.sha256.as_str())
                && engine_binary_path(app_support, runner_id, &asset.binary).is_file()
            {
                return Ok(engine_binary_path(app_support, runner_id, &asset.binary));
            }
        }
    }

    let parent = dir.parent().ok_or_else(|| EngineInstallError::Io {
        context: "engine dir has no parent".to_string(),
        source: std::io::Error::new(std::io::ErrorKind::Other, "no parent"),
    })?;
    std::fs::create_dir_all(parent).map_err(|error| EngineInstallError::Io {
        context: format!("cannot create {}", parent.display()),
        source: error,
    })?;
    let staging = parent.join(format!(".{runner_id}.installing"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|error| EngineInstallError::Io {
        context: format!("cannot create staging {}", staging.display()),
        source: error,
    })?;

    let result = install_into_staging(app_support, runner_id, asset, client, &staging).await;
    match result {
        Ok(binary) => {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::rename(&staging, &dir).map_err(|error| EngineInstallError::Io {
                context: format!("cannot promote {} to {}", staging.display(), dir.display()),
                source: error,
            })?;
            Ok(binary)
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(error)
        }
    }
}

async fn install_into_staging(
    app_support: &Path,
    runner_id: &str,
    asset: &crate::runners::EngineAsset,
    client: &reqwest::Client,
    staging: &Path,
) -> Result<PathBuf, EngineInstallError> {
    let archive_path = staging.join("engine-archive.bin");
    download_to(client, &asset.download_url, &archive_path).await?;
    verify_checksum(&archive_path, &asset.sha256)?;

    let output = Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(staging)
        .output()
        .map_err(|error| EngineInstallError::Io {
            context: "cannot run /usr/bin/tar".to_string(),
            source: error,
        })?;
    if !output.status.success() {
        return Err(EngineInstallError::Extract(format!(
            "tar exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let _ = std::fs::remove_file(&archive_path);

    let binary = engine_binary_path(app_support, runner_id, &asset.binary);
    // staging 期间的 binary 路径要以 staging 为根计算。
    let relative = asset.binary.as_str();
    let staged_binary = staging.join(relative);
    if !staged_binary.is_file() {
        return Err(EngineInstallError::BinaryMissing {
            path: staged_binary,
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged_binary, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::write(
        staging.join(".macai-engine.json"),
        serde_json::json!({
            "runner_id": runner_id,
            "sha256": asset.sha256,
            "binary": relative,
        })
        .to_string(),
    )
    .map_err(|error| EngineInstallError::Io {
        context: "cannot write engine fingerprint".to_string(),
        source: error,
    })?;
    Ok(binary)
}

async fn download_to(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
) -> Result<(), EngineInstallError> {
    let response = client
        .get(url)
        .header("User-Agent", "MacAI/0.1 aiworkd")
        .send()
        .await
        .map_err(|error| EngineInstallError::Download(error.to_string()))?;
    if !response.status().is_success() {
        return Err(EngineInstallError::Download(format!(
            "{} returned {}",
            url,
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| EngineInstallError::Download(error.to_string()))?;
    std::fs::write(destination, &bytes).map_err(|error| EngineInstallError::Io {
        context: format!("cannot write {}", destination.display()),
        source: error,
    })?;
    Ok(())
}

fn verify_checksum(path: &Path, expected: &str) -> Result<(), EngineInstallError> {
    let bytes = std::fs::read(path).map_err(|error| EngineInstallError::Io {
        context: format!("cannot read {}", path.display()),
        source: error,
    })?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(EngineInstallError::Checksum {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}
