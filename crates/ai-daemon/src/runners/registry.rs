use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use semver::{Version, VersionReq};
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
    pub bundled_profiles: Vec<BundledProfile>,
}

#[derive(Debug, Clone)]
pub struct BundledProfile {
    pub adapter: String,
    pub profile: ModelProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerRegistryError {
    InvalidVersionRequirement {
        requirement: String,
        reason: String,
    },
    InvalidRunnerVersion {
        id: String,
        version: String,
    },
    NoTrustedRunner {
        id: String,
        requirement: String,
    },
    AmbiguousTrustedRunner {
        id: String,
        requirement: String,
        versions: Vec<String>,
    },
    ProfileNotFound {
        profile_id: String,
    },
    ProfileRunnerMismatch {
        profile_id: String,
    },
    ProfileCapabilityMismatch {
        profile_id: String,
    },
    PackageDigestChanged {
        root: PathBuf,
    },
    PackageStaging {
        root: PathBuf,
        reason: String,
    },
}

impl fmt::Display for RunnerRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidVersionRequirement {
                requirement,
                reason,
            } => write!(
                formatter,
                "invalid Runner version requirement '{requirement}': {reason}"
            ),
            Self::InvalidRunnerVersion { id, version } => {
                write!(
                    formatter,
                    "trusted Runner {id} has invalid SemVer version '{version}'"
                )
            }
            Self::NoTrustedRunner { id, requirement } => {
                write!(
                    formatter,
                    "no trusted Runner {id} satisfies version requirement '{requirement}'"
                )
            }
            Self::AmbiguousTrustedRunner {
                id,
                requirement,
                versions,
            } => write!(
                formatter,
                "multiple trusted Runner versions for {id} satisfy '{requirement}': {}",
                versions.join(", ")
            ),
            Self::ProfileNotFound { profile_id } => {
                write!(
                    formatter,
                    "no trusted Runner supplies model profile '{profile_id}'"
                )
            }
            Self::ProfileRunnerMismatch { profile_id } => write!(
                formatter,
                "profile '{profile_id}' does not match its Runner or adapter declaration"
            ),
            Self::ProfileCapabilityMismatch { profile_id } => write!(
                formatter,
                "profile '{profile_id}' requires a capability its selected Runner does not declare"
            ),
            Self::PackageDigestChanged { root } => write!(
                formatter,
                "Runner package {} changed after discovery and cannot be executed",
                root.display()
            ),
            Self::PackageStaging { root, reason } => write!(
                formatter,
                "cannot stage Runner package {} for execution: {reason}",
                root.display()
            ),
        }
    }
}

impl std::error::Error for RunnerRegistryError {}

impl RunnerDescriptor {
    /// Copy the discovered package to a daemon-owned directory and verify that the staged bytes
    /// still equal the digest that was explicitly trusted. The supervisor executes only this copy.
    pub fn stage_for_execution(
        &self,
        runtime_temp_root: &Path,
    ) -> Result<PathBuf, RunnerRegistryError> {
        if self.state != RunnerState::Trusted {
            return Err(RunnerRegistryError::NoTrustedRunner {
                id: self
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.id.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                requirement: "trusted descriptor".to_string(),
            });
        }
        let expected_digest =
            self.package_digest
                .as_ref()
                .ok_or_else(|| RunnerRegistryError::PackageStaging {
                    root: self.root.clone(),
                    reason: "trusted descriptor has no package digest".to_string(),
                })?;
        let source_digest =
            package_digest(&self.root).map_err(|reason| RunnerRegistryError::PackageStaging {
                root: self.root.clone(),
                reason,
            })?;
        if source_digest != *expected_digest {
            return Err(RunnerRegistryError::PackageDigestChanged {
                root: self.root.clone(),
            });
        }
        let staging_root = create_staging_root(runtime_temp_root).map_err(|reason| {
            RunnerRegistryError::PackageStaging {
                root: self.root.clone(),
                reason,
            }
        })?;
        let staged = (|| {
            copy_package_contents(&self.root, &staging_root)?;
            let staged_digest = package_digest(&staging_root)?;
            if staged_digest != *expected_digest {
                return Err("staged package digest does not match discovered digest".to_string());
            }
            self.manifest
                .as_ref()
                .ok_or_else(|| "trusted descriptor has no manifest".to_string())?
                .validate_package(&staging_root)
                .map_err(|error| error.to_string())?;
            Ok(())
        })();
        if let Err(reason) = staged {
            let _ = std::fs::remove_dir_all(&staging_root);
            return Err(RunnerRegistryError::PackageStaging {
                root: self.root.clone(),
                reason,
            });
        }
        Ok(staging_root)
    }
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

    /// Select a trusted Runner only when the caller supplies a requirement that identifies one
    /// matching installed version. Discovery order is never a tie-breaker.
    pub fn trusted(
        &self,
        id: &str,
        version_requirement: &str,
    ) -> Result<&RunnerDescriptor, RunnerRegistryError> {
        let requirement = VersionReq::parse(version_requirement).map_err(|error| {
            RunnerRegistryError::InvalidVersionRequirement {
                requirement: version_requirement.to_string(),
                reason: error.to_string(),
            }
        })?;
        let mut matches = Vec::new();
        for entry in &self.entries {
            if entry.state != RunnerState::Trusted {
                continue;
            }
            let Some(manifest) = &entry.manifest else {
                continue;
            };
            if manifest.id != id {
                continue;
            }
            let version = Version::parse(&manifest.version).map_err(|_| {
                RunnerRegistryError::InvalidRunnerVersion {
                    id: manifest.id.clone(),
                    version: manifest.version.clone(),
                }
            })?;
            if requirement.matches(&version) {
                matches.push(entry);
            }
        }
        match matches.len() {
            0 => Err(RunnerRegistryError::NoTrustedRunner {
                id: id.to_string(),
                requirement: version_requirement.to_string(),
            }),
            1 => Ok(matches.remove(0)),
            _ => Err(RunnerRegistryError::AmbiguousTrustedRunner {
                id: id.to_string(),
                requirement: version_requirement.to_string(),
                versions: matches
                    .into_iter()
                    .filter_map(|entry| {
                        entry
                            .manifest
                            .as_ref()
                            .map(|manifest| manifest.version.clone())
                    })
                    .collect(),
            }),
        }
    }

    /// Resolve a bundled profile only through its explicit Runner, adapter, capability contracts,
    /// and SemVer compatibility range.
    pub fn resolve_bundled_profile(
        &self,
        profile_id: &str,
    ) -> Result<ResolvedProfile<'_>, RunnerRegistryError> {
        let mut matches = Vec::new();
        for entry in &self.entries {
            if entry.state != RunnerState::Trusted {
                continue;
            }
            let Some(manifest) = &entry.manifest else {
                continue;
            };
            for bundled in &entry.bundled_profiles {
                let profile = bundled.profile.clone();
                if profile.id != profile_id {
                    continue;
                }
                if profile.runner != manifest.id || profile.adapter != bundled.adapter {
                    return Err(RunnerRegistryError::ProfileRunnerMismatch {
                        profile_id: profile.id,
                    });
                }
                let runner = self.trusted(&profile.runner, &profile.compatibility.runner)?;
                let runner_manifest = runner
                    .manifest
                    .as_ref()
                    .expect("trusted runner descriptor has a manifest");
                if !runner_manifest
                    .models
                    .iter()
                    .any(|model| model.adapter == profile.adapter)
                {
                    return Err(RunnerRegistryError::ProfileRunnerMismatch {
                        profile_id: profile.id,
                    });
                }
                if profile
                    .capabilities
                    .iter()
                    .any(|capability| !runner_manifest.capabilities.contains(capability))
                {
                    return Err(RunnerRegistryError::ProfileCapabilityMismatch {
                        profile_id: profile.id,
                    });
                }
                matches.push((runner, profile));
            }
        }
        match matches.len() {
            0 => Err(RunnerRegistryError::ProfileNotFound {
                profile_id: profile_id.to_string(),
            }),
            1 => {
                let (runner, profile) = matches.remove(0);
                Ok(ResolvedProfile { runner, profile })
            }
            _ => Err(RunnerRegistryError::AmbiguousTrustedRunner {
                id: format!("model profile '{profile_id}'"),
                requirement: "profile identity".to_string(),
                versions: matches
                    .into_iter()
                    .filter_map(|(runner, _)| {
                        runner
                            .manifest
                            .as_ref()
                            .map(|manifest| manifest.version.clone())
                    })
                    .collect(),
            }),
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
                bundled_profiles: Vec::new(),
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
                bundled_profiles: Vec::new(),
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
            bundled_profiles: Vec::new(),
        };
    }
    let bundled_profiles = match manifest
        .models
        .iter()
        .map(|bundled| {
            ModelProfile::load(&root.join(&bundled.profile)).map(|profile| BundledProfile {
                adapter: bundled.adapter.clone(),
                profile,
            })
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(profiles) => profiles,
        Err(error) => {
            return RunnerDescriptor {
                root,
                package_digest: Some(digest),
                state: RunnerState::Unavailable,
                reason: Some(error.to_string()),
                manifest: Some(manifest),
                bundled_profiles: Vec::new(),
            }
        }
    };
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
        bundled_profiles,
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
    collect_package_files(root, &mut files)?;
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

fn collect_package_files(current: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
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
            collect_package_files(&path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn create_staging_root(parent: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create runtime temp root {}: {error}",
            parent.display()
        )
    })?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("cannot create staging nonce: {error}"))?
        .as_nanos();
    for attempt in 0..128_u32 {
        let root = parent.join(format!(
            "runner-package-{}-{nonce}-{attempt}",
            std::process::id()
        ));
        match std::fs::create_dir(&root) {
            Ok(()) => return Ok(root),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create staging directory {}: {error}",
                    root.display()
                ))
            }
        }
    }
    Err("cannot allocate unique Runner package staging directory".to_string())
}

fn copy_package_contents(source: &Path, destination: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(source)
        .map_err(|error| format!("cannot scan {}: {error}", source.display()))?
    {
        let entry =
            entry.map_err(|error| format!("cannot read package directory entry: {error}"))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&source_path)
            .map_err(|error| format!("cannot inspect {}: {error}", source_path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Runner package contains forbidden symlink {}",
                source_path.display()
            ));
        }
        if metadata.is_dir() {
            std::fs::create_dir(&destination_path).map_err(|error| {
                format!("cannot create {}: {error}", destination_path.display())
            })?;
            copy_package_contents(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            std::fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "cannot copy {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
            std::fs::set_permissions(&destination_path, metadata.permissions()).map_err(
                |error| {
                    format!(
                        "cannot set permissions on {}: {error}",
                        destination_path.display()
                    )
                },
            )?;
        }
    }
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
project = "."
lock = "uv.lock"
python = ">=3.12,<3.13"
probe = ["{environment.python}", "-c", "print('probe')"]
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

    const PROFILE: &str = "schema='macai.model.v1'\nid='fake'\nname='Fake'\ncapabilities=['tts.v1']\nrunner='org.example.fake'\nadapter='fake'\nformat='directory'\n[source]\ntype='huggingface'\nrepo='org/fake'\nrevision='0123456789abcdef0123456789abcdef01234567'\n[artifacts]\ndirectory='fake'\nfiles=['model']\n[compatibility]\nrunner='>=0.1,<0.2'\n";

    fn write_package(root: &Path, version: &str, profile: &str) {
        std::fs::create_dir_all(root.join("profiles")).unwrap();
        std::fs::write(root.join("runner.toml"), MANIFEST.replace("0.1.0", version)).unwrap();
        std::fs::write(root.join("pyproject.toml"), "[project]\nname = 'fake'\n").unwrap();
        std::fs::write(root.join("uv.lock"), "version = 1\n").unwrap();
        std::fs::write(root.join("profiles/fake.toml"), profile).unwrap();
    }

    #[test]
    fn one_bad_plugin_does_not_hide_a_trusted_runner() {
        let root =
            std::env::temp_dir().join(format!("macai-runner-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let good = root.join("good");
        let bad = root.join("bad");
        write_package(&good, "0.1.0", PROFILE);
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("runner.toml"), "schema = 'wrong'\n").unwrap();
        let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
        assert!(registry.trusted("org.example.fake", "=0.1.0").is_ok());
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

    #[test]
    fn runner_selection_requires_one_compatible_version() {
        let root = std::env::temp_dir().join(format!(
            "macai-runner-registry-versions-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        write_package(&root.join("v010"), "0.1.0", PROFILE);
        write_package(&root.join("v020"), "0.2.0", PROFILE);
        let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
        assert!(matches!(
            registry.trusted("org.example.fake", ">=0.1,<0.3"),
            Err(RunnerRegistryError::AmbiguousTrustedRunner { .. })
        ));
        assert_eq!(
            registry
                .trusted("org.example.fake", "=0.2.0")
                .unwrap()
                .manifest
                .as_ref()
                .unwrap()
                .version,
            "0.2.0"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_rejects_an_unsatisfied_runner_range() {
        let root = std::env::temp_dir().join(format!(
            "macai-runner-registry-incompatible-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        write_package(
            &root.join("runner"),
            "0.1.0",
            &PROFILE.replace(">=0.1,<0.2", ">=99.0"),
        );
        let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
        assert!(matches!(
            registry.resolve_bundled_profile("fake"),
            Err(RunnerRegistryError::NoTrustedRunner { .. })
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn profile_selection_uses_the_discovery_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "macai-runner-registry-profile-snapshot-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let package = root.join("runner");
        write_package(&package, "0.1.0", PROFILE);
        let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
        std::fs::write(
            package.join("profiles/fake.toml"),
            PROFILE.replace("id='fake'", "id='changed-after-discovery'"),
        )
        .unwrap();
        assert_eq!(
            registry.resolve_bundled_profile("fake").unwrap().profile.id,
            "fake"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
