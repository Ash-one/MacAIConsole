use std::fmt;
use std::path::{Component, Path};

use serde::Deserialize;

use super::RUNNER_PROTOCOL_V1;

pub const RUNNER_SCHEMA_V1: &str = "macai.runner.v1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerManifest {
    pub schema: String,
    pub id: String,
    pub version: String,
    pub protocols: Vec<String>,
    pub capabilities: Vec<String>,
    pub entrypoint: Entrypoint,
    pub runtime: RunnerRuntime,
    pub capacity: Capacity,
    pub timeouts: Timeouts,
    pub security: Security,
    #[serde(default)]
    pub models: Vec<BundledModel>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entrypoint {
    pub command: Vec<String>,
    pub working_directory: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerRuntime {
    #[serde(rename = "type")]
    pub runtime_type: String,
    pub id: String,
    pub project: String,
    pub lock: String,
    pub python: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capacity {
    pub max_instances: u32,
    pub max_concurrency_per_instance: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timeouts {
    pub boot_seconds: u64,
    pub load_seconds: u64,
    pub inference_seconds: u64,
    pub shutdown_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Security {
    pub network_during_install: bool,
    pub network_during_runtime: bool,
    #[serde(default)]
    pub inherit_environment: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundledModel {
    pub profile: String,
    pub adapter: String,
}

#[derive(Debug)]
pub struct ManifestError(String);

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ManifestError {}

impl RunnerManifest {
    pub fn parse(contents: &str) -> Result<Self, ManifestError> {
        let manifest: Self = toml::from_str(contents)
            .map_err(|error| ManifestError(format!("invalid runner.toml: {error}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|error| ManifestError(format!("cannot read {}: {error}", path.display())))?;
        Self::parse(&contents)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema != RUNNER_SCHEMA_V1 {
            return Err(ManifestError(format!(
                "unsupported runner schema '{}'",
                self.schema
            )));
        }
        if !valid_id(&self.id) {
            return Err(ManifestError(
                "runner id must use lowercase ASCII letters, digits, '.' or '-'".to_string(),
            ));
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError(
                "runner version must not be empty".to_string(),
            ));
        }
        if !self
            .protocols
            .iter()
            .any(|protocol| protocol == RUNNER_PROTOCOL_V1)
        {
            return Err(ManifestError(format!(
                "runner must support {RUNNER_PROTOCOL_V1}"
            )));
        }
        if self.capabilities.is_empty()
            || self
                .capabilities
                .iter()
                .any(|capability| !valid_capability(capability))
        {
            return Err(ManifestError(
                "runner must declare versioned capabilities such as tts.v1".to_string(),
            ));
        }
        if self.entrypoint.command.is_empty() {
            return Err(ManifestError(
                "entrypoint command must not be empty".to_string(),
            ));
        }
        if !matches!(
            self.entrypoint.working_directory.as_str(),
            "package" | "runtime"
        ) {
            return Err(ManifestError(
                "entrypoint working_directory must be 'package' or 'runtime'".to_string(),
            ));
        }
        for argument in &self.entrypoint.command {
            validate_command_argument(argument)?;
        }
        if self.runtime.id.trim().is_empty() || self.runtime.runtime_type.trim().is_empty() {
            return Err(ManifestError(
                "runtime type and id must not be empty".to_string(),
            ));
        }
        validate_relative_path("runtime.project", &self.runtime.project)?;
        validate_relative_path("runtime.lock", &self.runtime.lock)?;
        if self.runtime.runtime_type == "python-uv"
            && self
                .runtime
                .python
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(ManifestError(
                "python-uv runtime requires a Python constraint".to_string(),
            ));
        }
        if self.capacity.max_instances == 0 || self.capacity.max_concurrency_per_instance == 0 {
            return Err(ManifestError(
                "runner capacity values must be positive".to_string(),
            ));
        }
        if self.timeouts.boot_seconds == 0
            || self.timeouts.load_seconds == 0
            || self.timeouts.inference_seconds == 0
            || self.timeouts.shutdown_seconds == 0
        {
            return Err(ManifestError(
                "runner timeout values must be positive".to_string(),
            ));
        }
        if self
            .security
            .inherit_environment
            .iter()
            .any(|name| !matches!(name.as_str(), "HTTP_PROXY" | "HTTPS_PROXY" | "NO_PROXY"))
        {
            return Err(ManifestError(
                "runner may inherit only HTTP_PROXY, HTTPS_PROXY and NO_PROXY".to_string(),
            ));
        }
        for model in &self.models {
            validate_relative_path("models.profile", &model.profile)?;
            if model.adapter.trim().is_empty() {
                return Err(ManifestError(
                    "models.adapter must not be empty".to_string(),
                ));
            }
        }
        Ok(())
    }

    pub fn validate_package(&self, package_root: &Path) -> Result<(), ManifestError> {
        let root = package_root.canonicalize().map_err(|error| {
            ManifestError(format!(
                "cannot canonicalize package root {}: {error}",
                package_root.display()
            ))
        })?;
        for relative in [&self.runtime.project, &self.runtime.lock]
            .into_iter()
            .chain(self.models.iter().map(|model| &model.profile))
        {
            let candidate = root.join(relative).canonicalize().map_err(|error| {
                ManifestError(format!("cannot resolve package path '{relative}': {error}"))
            })?;
            if !candidate.starts_with(&root) {
                return Err(ManifestError(format!(
                    "package path '{relative}' escapes its root"
                )));
            }
        }
        Ok(())
    }

    pub fn resolve_command(
        &self,
        package_root: &Path,
        environment_python: &Path,
        runtime_temp_root: &Path,
    ) -> Result<Vec<String>, ManifestError> {
        self.validate_package(package_root)?;
        let root = package_root.canonicalize().map_err(|error| {
            ManifestError(format!(
                "cannot canonicalize package root {}: {error}",
                package_root.display()
            ))
        })?;
        self.entrypoint
            .command
            .iter()
            .enumerate()
            .map(|(index, argument)| {
                resolve_argument(
                    argument,
                    index == 0,
                    &root,
                    environment_python,
                    runtime_temp_root,
                )
            })
            .collect()
    }
}

fn resolve_argument(
    argument: &str,
    is_program: bool,
    package_root: &Path,
    environment_python: &Path,
    runtime_temp_root: &Path,
) -> Result<String, ManifestError> {
    let resolved = match argument {
        "{environment.python}" => environment_python.to_path_buf(),
        "{package.root}" => package_root.to_path_buf(),
        "{runtime.temp_root}" => runtime_temp_root.to_path_buf(),
        literal if is_program && is_safe_relative(literal) => package_root.join(literal),
        literal => return Ok(literal.to_string()),
    };
    Ok(resolved.to_string_lossy().into_owned())
}

fn validate_command_argument(argument: &str) -> Result<(), ManifestError> {
    if argument.trim().is_empty()
        || ["|", ";", "&&", "||", "`", "$(", ">", "<"]
            .iter()
            .any(|forbidden| argument.contains(forbidden))
        || matches!(argument, "sh" | "bash" | "zsh" | "fish")
    {
        return Err(ManifestError(
            "entrypoint command must be argv, not a shell expression".to_string(),
        ));
    }
    Ok(())
}

fn validate_relative_path(field: &str, value: &str) -> Result<(), ManifestError> {
    if !is_safe_relative(value) {
        return Err(ManifestError(format!(
            "{field} must be a safe relative package path"
        )));
    }
    Ok(())
}

fn is_safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.trim().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

fn valid_capability(value: &str) -> bool {
    let mut parts = value.rsplitn(2, '.');
    matches!(parts.next(), Some(version) if version.starts_with('v') && version[1..].chars().all(|c| c.is_ascii_digit()))
        && matches!(parts.next(), Some(name) if !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> &'static str {
        r#"
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
boot_seconds = 15
load_seconds = 300
inference_seconds = 300
shutdown_seconds = 5

[security]
network_during_install = true
network_during_runtime = false
inherit_environment = ["HTTPS_PROXY"]
"#
    }

    #[test]
    fn validates_a_complete_manifest() {
        let manifest = RunnerManifest::parse(manifest()).unwrap();
        assert_eq!(manifest.id, "org.example.fake");
    }

    #[test]
    fn rejects_shell_and_path_escape() {
        assert!(RunnerManifest::parse(&manifest().replace("fake-runner", "sh")).is_err());
        assert!(RunnerManifest::parse(&manifest().replace("uv.lock", "../uv.lock")).is_err());
    }
}
