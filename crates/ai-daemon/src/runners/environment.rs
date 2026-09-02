use std::fmt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// 可复现 Python environment 的最小身份输入。
///
/// fingerprint 是环境目录和升级判定的稳定输入；实际 `uv sync` 的 staging/atomic
/// promotion 仍由后续 daemon environment owner 接入，而不是交给 GUI 或 Runner。
#[derive(Debug, Clone)]
pub struct EnvironmentInput {
    pub id: String,
    pub project: PathBuf,
    pub lock: PathBuf,
    pub python_abi: String,
    pub platform: String,
    pub uv_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentFingerprint(pub String);

impl fmt::Display for EnvironmentFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl EnvironmentInput {
    pub fn fingerprint(&self) -> std::io::Result<EnvironmentFingerprint> {
        let mut hasher = Sha256::new();
        update_field(&mut hasher, b"id", self.id.as_bytes());
        update_field(&mut hasher, b"python_abi", self.python_abi.as_bytes());
        update_field(&mut hasher, b"platform", self.platform.as_bytes());
        update_field(&mut hasher, b"uv_source", self.uv_source.as_bytes());
        update_file(&mut hasher, b"pyproject", &self.project)?;
        update_file(&mut hasher, b"uv_lock", &self.lock)?;
        Ok(EnvironmentFingerprint(format!("{:x}", hasher.finalize())))
    }
}

fn update_field(hasher: &mut Sha256, name: &[u8], value: &[u8]) {
    hasher.update((name.len() as u64).to_be_bytes());
    hasher.update(name);
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn update_file(hasher: &mut Sha256, name: &[u8], path: &Path) -> std::io::Result<()> {
    let contents = std::fs::read(path)?;
    update_field(hasher, name, &contents);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_when_the_lock_changes() {
        let root = std::env::temp_dir().join(format!("macai-environment-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let project = root.join("pyproject.toml");
        let lock = root.join("uv.lock");
        std::fs::write(&project, "[project]\nname = 'fake'\n").unwrap();
        std::fs::write(&lock, "version = 1\n").unwrap();
        let input = EnvironmentInput {
            id: "org.macai.fake-python".to_string(),
            project: project.clone(),
            lock: lock.clone(),
            python_abi: "cpython-3.12".to_string(),
            platform: "darwin-arm64".to_string(),
            uv_source: "uv-0.5".to_string(),
        };
        let before = input.fingerprint().unwrap();
        std::fs::write(&lock, "version = 2\n").unwrap();
        assert_ne!(before, input.fingerprint().unwrap());
        let _ = std::fs::remove_dir_all(root);
    }
}
