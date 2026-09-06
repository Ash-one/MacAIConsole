use std::fs;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{LocalDetector, RunnerDescriptor, RunnerState};

const MAX_ENTRIES: usize = 4_096;
const MAX_DEPTH: usize = 4;
const MAX_METADATA_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Serialize)]
pub struct DetectorMatch {
    pub runner: String,
    pub adapter: String,
    pub capability: String,
    pub detector_id: String,
    pub reason: String,
    pub manifest_digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    pub canonical_path: String,
    pub size_bytes: u64,
    pub fingerprint: String,
    pub matches: Vec<DetectorMatch>,
    pub diagnostics: Vec<String>,
}

pub fn inspect(root: &Path, descriptors: &[RunnerDescriptor]) -> Result<Inspection, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot open {}: {error}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let (size_bytes, fingerprint) = bounded_walk(&root)?;
    let mut matches = Vec::new();
    let mut diagnostics = Vec::new();
    for descriptor in descriptors {
        if descriptor.state != RunnerState::Trusted {
            continue;
        }
        let (Some(manifest), Some(digest)) = (&descriptor.manifest, &descriptor.package_digest)
        else {
            continue;
        };
        for detector in &manifest.local_detectors {
            match matches_detector(&root, detector) {
                Ok(true) => matches.push(DetectorMatch {
                    runner: manifest.id.clone(),
                    adapter: detector.adapter.clone(),
                    capability: detector.capability.clone(),
                    detector_id: detector.id.clone(),
                    reason: detector.reason.clone(),
                    manifest_digest: digest.clone(),
                }),
                Ok(false) => {}
                Err(error) => diagnostics.push(format!("{}: {error}", detector.id)),
            }
        }
    }
    Ok(Inspection {
        canonical_path: root.to_string_lossy().into_owned(),
        size_bytes,
        fingerprint,
        matches,
        diagnostics,
    })
}

pub fn bounded_walk(root: &Path) -> Result<(u64, String), String> {
    let mut entries = Vec::new();
    collect(root, root, 0, &mut entries)?;
    entries.sort();
    let mut size: u64 = 0;
    let mut hash = Sha256::new();
    for (path, length, modified) in entries {
        size = size
            .checked_add(length)
            .ok_or_else(|| "directory size overflow".to_string())?;
        hash.update(path.as_bytes());
        hash.update(length.to_le_bytes());
        hash.update(modified.to_le_bytes());
    }
    Ok((size, format!("{:x}", hash.finalize())))
}

fn collect(
    root: &Path,
    current: &Path,
    depth: usize,
    entries: &mut Vec<(String, u64, u64)>,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("directory exceeds inspection depth limit".to_string());
    }
    for entry in fs::read_dir(current)
        .map_err(|error| format!("cannot read {}: {error}", current.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!("symlink is not allowed: {}", path.display()));
        }
        if entries.len() >= MAX_ENTRIES {
            return Err("directory exceeds inspection entry limit".to_string());
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "inspection path escaped root".to_string())?
            .to_string_lossy()
            .into_owned();
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_secs())
            .unwrap_or(0);
        if metadata.is_dir() {
            entries.push((format!("{relative}/"), 0, modified));
            collect(root, &path, depth + 1, entries)?;
        } else if metadata.is_file() {
            entries.push((relative, metadata.len(), modified));
        }
    }
    Ok(())
}

fn matches_detector(root: &Path, detector: &LocalDetector) -> Result<bool, String> {
    if detector
        .required_files
        .iter()
        .any(|path| !regular_file(&root.join(path)))
        || detector
            .required_directories
            .iter()
            .any(|path| !regular_directory(&root.join(path)))
        || detector
            .required_absent
            .iter()
            .any(|path| root.join(path).exists())
    {
        return Ok(false);
    }
    for required in &detector.required_globs {
        if glob_count(root, &required.pattern)? < required.min_matches {
            return Ok(false);
        }
    }
    for predicate in &detector.json_predicates {
        let path = root.join(&predicate.file);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| format!("missing metadata file {}", predicate.file))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > MAX_METADATA_BYTES
        {
            return Ok(false);
        }
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        let json: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid {}: {error}", predicate.file))?;
        if json.pointer(&predicate.pointer) != Some(&predicate.equals) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}
fn regular_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}
fn glob_count(root: &Path, pattern: &str) -> Result<usize, String> {
    let mut paths = Vec::new();
    collect(root, root, 0, &mut paths)?;
    Ok(paths
        .into_iter()
        .filter(|(path, _, _)| {
            !path.ends_with('/') && glob_matches(pattern.as_bytes(), path.as_bytes())
        })
        .count())
}
fn glob_matches(pattern: &[u8], value: &[u8]) -> bool {
    let (mut p, mut v, mut star, mut retry) = (0, 0, None, 0);
    while v < value.len() {
        if p < pattern.len() && pattern[p] == value[v] {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            retry = v;
        } else if let Some(index) = star {
            if retry == value.len() || value[retry] == b'/' {
                return false;
            }
            p = index + 1;
            retry += 1;
            v = retry;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::{JsonPredicate, RequiredGlob};

    #[test]
    fn glob_matches_safetensors_at_root_only() {
        assert!(glob_matches(b"*.safetensors", b"model.safetensors"));
        assert!(!glob_matches(b"*.safetensors", b"voices/a.safetensors"));
    }

    #[test]
    fn detector_requires_a_regular_signed_layout_and_rejects_symlinks() {
        let root = std::env::temp_dir().join(format!("macai-inspection-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("config.json"), r#"{"model_type":"qwen3_asr"}"#).unwrap();
        fs::write(root.join("model.safetensors"), "weights").unwrap();
        let detector = LocalDetector {
            id: "qwen3-asr".to_string(),
            capability: "stt.v1".to_string(),
            adapter: "asr".to_string(),
            required_files: vec!["config.json".to_string()],
            required_directories: Vec::new(),
            required_globs: vec![RequiredGlob {
                pattern: "*.safetensors".to_string(),
                min_matches: 1,
            }],
            required_absent: Vec::new(),
            json_predicates: vec![JsonPredicate {
                file: "config.json".to_string(),
                pointer: "/model_type".to_string(),
                equals: serde_json::json!("qwen3_asr"),
            }],
            reason: "test".to_string(),
        };
        assert!(matches_detector(&root, &detector).unwrap());
        #[cfg(unix)]
        std::os::unix::fs::symlink("model.safetensors", root.join("escaped.safetensors")).unwrap();
        #[cfg(unix)]
        assert!(bounded_walk(&root).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
