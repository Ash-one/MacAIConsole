use std::fmt;
use std::path::Path;

use serde::Deserialize;

pub const MODEL_PROFILE_SCHEMA_V1: &str = "macai.model.v1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    pub schema: String,
    pub id: String,
    pub name: String,
    pub capabilities: Vec<String>,
    pub runner: String,
    pub adapter: String,
    pub format: String,
    pub source: ProfileSource,
    pub artifacts: ProfileArtifacts,
    pub compatibility: ProfileCompatibility,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub repo: String,
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileArtifacts {
    pub directory: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileCompatibility {
    pub runner: String,
}

#[derive(Debug)]
pub struct ProfileError(String);

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ProfileError {}

impl ModelProfile {
    pub fn parse(contents: &str) -> Result<Self, ProfileError> {
        let profile: Self = toml::from_str(contents)
            .map_err(|error| ProfileError(format!("invalid model profile: {error}")))?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn load(path: &Path) -> Result<Self, ProfileError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|error| ProfileError(format!("cannot read {}: {error}", path.display())))?;
        Self::parse(&contents)
    }

    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.schema != MODEL_PROFILE_SCHEMA_V1 {
            return Err(ProfileError(format!(
                "unsupported model profile schema '{}'",
                self.schema
            )));
        }
        for (name, value) in [
            ("id", &self.id),
            ("name", &self.name),
            ("runner", &self.runner),
            ("adapter", &self.adapter),
            ("format", &self.format),
            ("source.repo", &self.source.repo),
            ("source.revision", &self.source.revision),
            ("compatibility.runner", &self.compatibility.runner),
        ] {
            if value.trim().is_empty() {
                return Err(ProfileError(format!("{name} must not be empty")));
            }
        }
        if self.source.source_type != "huggingface" {
            return Err(ProfileError(
                "only huggingface profile sources are supported in v1".to_string(),
            ));
        }
        if !is_immutable_commit(&self.source.revision) {
            return Err(ProfileError(
                "source.revision must be a 40- or 64-character hexadecimal commit".to_string(),
            ));
        }
        if self.capabilities.is_empty()
            || self
                .capabilities
                .iter()
                .any(|capability| !valid_capability(capability))
        {
            return Err(ProfileError(
                "profile capabilities must be versioned capability contracts".to_string(),
            ));
        }
        if !safe_relative(&self.artifacts.directory) || self.artifacts.files.is_empty() {
            return Err(ProfileError(
                "artifacts require a safe directory and at least one file".to_string(),
            ));
        }
        if self.artifacts.files.iter().any(|file| !safe_relative(file)) {
            return Err(ProfileError(
                "artifact file paths must be relative and must not escape their directory"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.trim().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn valid_capability(value: &str) -> bool {
    let mut parts = value.rsplitn(2, '.');
    matches!(parts.next(), Some(version) if version.starts_with('v') && version[1..].chars().all(|c| c.is_ascii_digit()))
        && matches!(parts.next(), Some(name) if !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
}

fn is_immutable_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_mutable_or_escaping_profile() {
        let profile = r#"
schema = "macai.model.v1"
id = "fake"
name = "Fake"
capabilities = ["tts.v1"]
runner = "org.example.fake"
adapter = "fake"
format = "directory"
[source]
type = "huggingface"
repo = "org/fake"
revision = "main"
[artifacts]
directory = "../escape"
files = ["model.bin"]
[compatibility]
runner = ">=0.1"
"#;
        assert!(ModelProfile::parse(profile).is_err());
    }
}
