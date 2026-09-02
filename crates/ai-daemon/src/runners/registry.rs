use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{ModelProfile, RunnerManifest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerState {
    Trusted,
    Untrusted,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct RunnerDescriptor {
    pub root: PathBuf,
    pub package_digest: Option<String>,
    pub state: RunnerState,
    pub reason: Option<String>,
    pub manifest: Option<RunnerManifest>,
}

#[derive(Debug, Clone, Default)]
pub struct RunnerRegistry {
    entries: Vec<RunnerDescriptor>,
}

impl RunnerRegistry {
    /// Discover bundled and explicitly trusted package directories without ever executing them.
    ///
    /// Built-in roots are trusted by the installed application boundary. Plugin roots become
    /// trusted only when their full package digest is present in `trusted_package_digests`.
    pub fn discover(
        builtin_roots: &[PathBuf],
        plugin_roots: &[PathBuf],
        trusted_package_digests: &HashSet<String>,
    ) -> Self {
        let mut entries = Vec::new();
        for root in builtin_roots {
            entries.extend(discover_in(root, true, trusted_package_digests));
        }
        for root in plugin_roots {
            entries.extend(discover_in(root, false, trusted_package_digests));
        }
        entries.sort_by(|left, right| left.root.cmp(&right.root));
        Self { entries }
    }

    pub fn entries(&self) -> &[RunnerDescriptor] {
        &self.entries
    }

    pub fn trusted(&self, id: &str) -> Option<&RunnerDescriptor> {
        self.entries.iter().find(|entry| {
            entry.state == RunnerState::Trusted
                && entry
                    .manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.id == id)
        })
    }

    /// Resolve a bundled profile only through the explicit Runner and adapter that owns it.
    /// There is deliberately no scan-order fallback.
    pub fn resolve_bundled_profile(&self, profile_id: &str) -> Result<ResolvedProfile<'_>, String> {
        let mut matches = Vec::new();
        for entry in &self.entries {
            if entry.state != RunnerState::Trusted {
                continue;
            }
            let Some(manifest) = &entry.manifest else {
                continue;
            };
            for bundled in &manifest.models {
                let profile_path = entry.root.join(&bundled.profile);
                let profile = match ModelProfile::load(&profile_path) {
                    Ok(profile) => profile,
                    Err(_error) => continue,
                };
                if profile.id == profile_id {
                    matches.push((entry, profile, bundled.adapter.as_str()));
                }
            }
        }
        match matches.len() {
            0 => Err(format!("no trusted Runner supplies model profile '{profile_id}'")),
            1 => {
                let (runner, profile, manifest_adapter) = matches.remove(0);
                if profile.runner
                    != runner
                        .manifest
                        .as_ref()
                        .expect("trusted runner has manifest")
                        .id
                    || profile.adapter != manifest_adapter
                {
                    return Err(format!(
                        "profile '{profile_id}' does not match its Runner or adapter declaration"
                    ));
                }
                Ok(ResolvedProfile { runner, profile })
            }
            _ => Err(format!(
                "multiple trusted Runners supply model profile '{profile_id}'; explicit version selection is required"
            )),
        }
    }
}

pub struct ResolvedProfile<'a> {
    pub runner: &'a RunnerDescriptor,
    pub profile: ModelProfile,
}

fn discover_in(
    root: &Path,
    builtin: bool,
    trusted_package_digests: &HashSet<String>,
) -> Vec<RunnerDescriptor> {
    manifest_paths(root, 3)
        .into_iter()
        .map(|manifest_path| descriptor_for(&manifest_path, builtin, trusted_package_digests))
        .collect()
}

fn descriptor_for(
    manifest_path: &Path,
    builtin: bool,
    trusted_package_digests: &HashSet<String>,
) -> RunnerDescriptor {
    let root = manifest_path
        .parent()
        .unwrap_or(manifest_path)
        .to_path_buf();
    let manifest = match RunnerManifest::load(manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            return RunnerDescriptor {
                root,
                package_digest: None,
                state: RunnerState::Unavailable,
                reason: Some(error.to_string()),
                manifest: None,
            }
        }
    };
    let digest = match package_digest(&root) {
        Ok(digest) => digest,
        Err(error) => {
            return RunnerDescriptor {
                root,
                package_digest: None,
                state: RunnerState::Unavailable,
                reason: Some(error),
                manifest: Some(manifest),
            }
        }
    };
    if let Err(error) = manifest.validate_package(&root) {
        return RunnerDescriptor {
            root,
            package_digest: Some(digest),
            state: RunnerState::Unavailable,
            reason: Some(error.to_string()),
            manifest: Some(manifest),
        };
    }
    let trusted = builtin || trusted_package_digests.contains(&digest);
    RunnerDescriptor {
        root,
        package_digest: Some(digest),
        state: if trusted {
            RunnerState::Trusted
        } else {
            RunnerState::Untrusted
        },
        reason: (!trusted).then(|| "package digest has not been explicitly trusted".to_string()),
        manifest: Some(manifest),
    }
}

fn manifest_paths(root: &Path, remaining_depth: usize) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => metadata,
        _ => return paths,
    };
    let _ = metadata;
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return paths,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_symlink() => metadata,
            _ => continue,
        };
        if metadata.is_file() && path.file_name().is_some_and(|name| name == "runner.toml") {
            paths.push(path);
        } else if metadata.is_dir() && remaining_depth > 0 {
            paths.extend(manifest_paths(&path, remaining_depth - 1));
        }
    }
    paths
}

fn package_digest(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    collect_package_files(root, root, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for path in files {
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("invalid package path: {error}"))?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(
            std::fs::read(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
        );
        hasher.update([0]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_package_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(current)
        .map_err(|error| format!("cannot scan {}: {error}", current.display()))?
    {
        let entry =
            entry.map_err(|error| format!("cannot read package directory entry: {error}"))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Runner package contains forbidden symlink {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_package_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    let _ = root;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
schema = "macai.runner.v1"
id = "org.example.fake"
version = "0.1.0"
protocols = ["macai.runner.v1"]
capabilities = ["tts.v1"]
[entrypoint]
command = ["fake-runner"]
working_directory = "package"
[runtime]
type = "python-uv"
id = "org.example.fake-python"
project = "pyproject.toml"
lock = "uv.lock"
python = ">=3.12,<3.13"
[capacity]
max_instances = 1
max_concurrency_per_instance = 1
[timeouts]
boot_seconds = 1
load_seconds = 1
inference_seconds = 1
shutdown_seconds = 1
[security]
network_during_install = false
network_during_runtime = false
[[models]]
profile = "profiles/fake.toml"
adapter = "fake"
"#;

    #[test]
    fn one_bad_plugin_does_not_hide_a_trusted_runner() {
        let root =
            std::env::temp_dir().join(format!("macai-runner-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let good = root.join("good");
        let bad = root.join("bad");
        std::fs::create_dir_all(good.join("profiles")).unwrap();
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(good.join("runner.toml"), MANIFEST).unwrap();
        std::fs::write(good.join("pyproject.toml"), "[project]\nname = 'fake'\n").unwrap();
        std::fs::write(good.join("uv.lock"), "version = 1\n").unwrap();
        std::fs::write(
            good.join("profiles/fake.toml"),
            "schema='macai.model.v1'\nid='fake'\nname='Fake'\ncapabilities=['tts.v1']\nrunner='org.example.fake'\nadapter='fake'\nformat='directory'\n[source]\ntype='huggingface'\nrepo='org/fake'\nrevision='0123456789abcdef0123456789abcdef01234567'\n[artifacts]\ndirectory='fake'\nfiles=['model']\n[compatibility]\nrunner='>=0.1'\n",
        )
        .unwrap();
        std::fs::write(bad.join("runner.toml"), "schema = 'wrong'\n").unwrap();
        let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
        assert!(registry.trusted("org.example.fake").is_some());
        assert!(registry
            .entries()
            .iter()
            .any(|entry| entry.state == RunnerState::Unavailable));
        assert_eq!(
            registry.resolve_bundled_profile("fake").unwrap().profile.id,
            "fake"
        );
        let untrusted = RunnerRegistry::discover(&[], &[good.clone()], &HashSet::new());
        let digest = untrusted.entries()[0].package_digest.clone().unwrap();
        assert_eq!(untrusted.entries()[0].state, RunnerState::Untrusted);
        let trusted = RunnerRegistry::discover(&[], &[good], &HashSet::from([digest]));
        assert_eq!(trusted.entries()[0].state, RunnerState::Trusted);
        let _ = std::fs::remove_dir_all(root);
    }
}
