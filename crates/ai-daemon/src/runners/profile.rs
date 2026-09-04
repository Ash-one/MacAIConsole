use std::fmt;
use std::path::Path;

use semver::VersionReq;
use serde::{Deserialize, Serialize};

pub const MODEL_PROFILE_SCHEMA_V1: &str = "macai.model.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(default)]
    pub defaults: ProfileDefaults,
    #[serde(default)]
    pub resources: ProfileResources,
    pub compatibility: ProfileCompatibility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub repo: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileArtifacts {
    pub directory: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileCompatibility {
    pub runner: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileDefaults {
    pub keep_alive: Option<String>,
    pub voice: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileResources {
    pub memory_estimate_bytes: Option<u64>,
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

    /// 解析 SQLite 中保存的 canonical JSON snapshot。持久快照走与 TOML 输入相同的
    /// 语义校验，避免数据库内容绕过 schema、revision 或路径约束。
    pub fn from_snapshot(contents: &str) -> Result<Self, ProfileError> {
        let profile: Self = serde_json::from_str(contents)
            .map_err(|error| ProfileError(format!("invalid model profile snapshot: {error}")))?;
        profile.validate()?;
        Ok(profile)
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
            ("compatibility.runner", &self.compatibility.runner),
        ] {
            if value.trim().is_empty() {
                return Err(ProfileError(format!("{name} must not be empty")));
            }
        }
        // source.repo / source.revision 的空值语义按 source_type 裁决：
        // huggingface 必须两者俱全（immutable commit 校验见下）；local 来源
        // 必须两者皆空（ad-hoc 绑定，见下）。
        VersionReq::parse(&self.compatibility.runner).map_err(|error| {
            ProfileError(format!(
                "compatibility.runner must be a SemVer range: {error}"
            ))
        })?;
        // `local` 来源只允许 ad-hoc 绑定路径构造（bind_adhoc_model）：
        // 用户本地目录注册的模型没有 HF repo/revision 身份，持久身份由
        // models.db 的注册记录承担。catalog Profile 仍必须是 huggingface
        // + immutable revision。
        if self.source.source_type == "local" {
            if !self.source.repo.is_empty() || !self.source.revision.is_empty() {
                return Err(ProfileError(
                    "local source must not declare repo or revision".to_string(),
                ));
            }
        } else if self.source.source_type != "huggingface" {
            return Err(ProfileError(
                "only huggingface or local profile sources are supported in v1".to_string(),
            ));
        }
        if self.source.source_type == "huggingface" {
            if self.source.repo.trim().is_empty() {
                return Err(ProfileError("source.repo must not be empty".to_string()));
            }
            if !is_immutable_commit(&self.source.revision) {
                return Err(ProfileError(
                    "source.revision must be a 40- or 64-character hexadecimal commit".to_string(),
                ));
            }
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
        if !safe_relative(&self.artifacts.directory) {
            return Err(ProfileError(
                "artifacts require a safe directory and at least one file".to_string(),
            ));
        }
        // ad-hoc 绑定（local 来源）在注册时只有一个目录名，files 清单留空；
        // catalog Profile 必须逐文件声明 artifacts。
        if self.source.source_type == "huggingface"
            && (self.artifacts.files.is_empty()
                || self.artifacts.files.iter().any(|file| !safe_relative(file)))
        {
            return Err(ProfileError(
                "artifact file paths must be relative and must not escape their directory"
                    .to_string(),
            ));
        }
        if self.source.source_type == "local"
            && self.artifacts.files.iter().any(|file| !safe_relative(file))
        {
            return Err(ProfileError(
                "artifact file paths must be relative and must not escape their directory"
                    .to_string(),
            ));
        }
        for (name, value) in [
            ("defaults.keep_alive", &self.defaults.keep_alive),
            ("defaults.voice", &self.defaults.voice),
            ("defaults.format", &self.defaults.format),
        ] {
            if value
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            {
                return Err(ProfileError(format!("{name} must not be empty")));
            }
        }
        if self.resources.memory_estimate_bytes == Some(0) {
            return Err(ProfileError(
                "resources.memory_estimate_bytes must be positive".to_string(),
            ));
        }
        Ok(())
    }

    /// 规范 JSON 快照（结构字段顺序稳定，序列化确定）。Model Profile 的
    /// 持久 snapshot 与 digest 以它为准；变更探测经 digest 判定。
    pub fn canonical_json(&self) -> Result<Vec<u8>, ProfileError> {
        serde_json::to_vec(self)
            .map_err(|error| ProfileError(format!("cannot serialize profile snapshot: {error}")))
    }

    /// sha256(digest of canonical JSON)。与 package digest（Runner 包全量内容）
    /// 分离：profile digest 只指纹 Model Profile 内容本身。
    pub fn digest(&self) -> Result<String, ProfileError> {
        use sha2::{Digest, Sha256};
        let bytes = self.canonical_json()?;
        Ok(format!("{:x}", Sha256::digest(&bytes)))
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
    matches!(parts.next(), Some(version) if version.len() > 1 && version.starts_with('v') && version[1..].chars().all(|c| c.is_ascii_digit()))
        && matches!(parts.next(), Some(name) if !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
}

fn is_immutable_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_deterministic_and_tracks_content_changes() {
        let base = r#"
schema = "macai.model.v1"
id = "kokoro-82m-zh"
name = "Kokoro 82M zh"
capabilities = ["tts.v1"]
runner = "org.macai.kokoro"
adapter = "kokoro-mlx"
format = "directory"
[source]
type = "huggingface"
repo = "1038lab/Kokoro-82M-zh-MLX"
revision = "4bd6c9644da381fee105f37fcd8cb63d038ba7e8"
[artifacts]
directory = "kokoro-82m-zh"
files = ["config.json", "model.safetensors"]
[defaults]
keep_alive = "always"
voice = "zf_001"
format = "wav"
[resources]
memory_estimate_bytes = 400000000
[compatibility]
runner = ">=0.1,<0.2"
"#;
        let profile = ModelProfile::parse(base).unwrap();
        let first = profile.digest().unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(
            profile.digest().unwrap(),
            first,
            "digest must be deterministic"
        );
        let changed = ModelProfile::parse(&base.replace("zf_001", "af_heart")).unwrap();
        assert_ne!(
            changed.digest().unwrap(),
            first,
            "digest must track content"
        );
    }

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

    #[test]
    fn parses_typed_defaults_and_resources() {
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
revision = "0123456789abcdef0123456789abcdef01234567"
[artifacts]
directory = "fake"
files = ["model.bin"]
[defaults]
keep_alive = "always"
voice = "zf_001"
format = "wav"
[resources]
memory_estimate_bytes = 400000000
[compatibility]
runner = ">=0.1,<0.2"
"#;
        assert!(ModelProfile::parse(&profile.replace(">=0.1,<0.2", "not-a-range")).is_err());
        let profile = ModelProfile::parse(profile).unwrap();
        assert_eq!(profile.defaults.voice.as_deref(), Some("zf_001"));
        assert_eq!(profile.resources.memory_estimate_bytes, Some(400_000_000));
    }
}
