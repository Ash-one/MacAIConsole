use std::fmt;
use std::path::{Component, Path};

use semver::Version;
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
    /// 可选的原生引擎产物（cpp 引擎 Runner 化）。声明后 install 流程
    /// 在环境同步之外还要下载并校验该产物；`engine.build` 存在时从固定
    /// source archive 构建目标二进制。
    #[serde(default)]
    pub engine: Option<EngineAsset>,
    pub capacity: Capacity,
    pub timeouts: Timeouts,
    pub security: Security,
    #[serde(default)]
    pub models: Vec<BundledModel>,
    #[serde(default)]
    pub local_detectors: Vec<LocalDetector>,
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
    /// 离线只读环境探针。`python-uv` runtime 必须声明；daemon 在锁定环境同步
    /// 完成后、提升为最终目录前执行，见 runner-manifest-v1 的 probe 契约。
    #[serde(default)]
    pub probe: Vec<String>,
    /// 无 `[[models]]` 的引擎（ad-hoc 绑定型，如 llama.cpp）声明的默认
    /// adapter 名。注册任意模型时构造 ad-hoc Model Profile 使用它。
    #[serde(default)]
    pub default_adapter: Option<String>,
}

/// 引擎产物声明（可选）：install 时由 daemon 下载并解压到受管 Engines
/// 目录。checksum 强制——产物内容变更即拒绝。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineAsset {
    pub download_url: String,
    pub sha256: String,
    /// 压缩包内要执行的可执行文件相对路径。
    pub binary: String,
    /// 官方没有目标平台预编译产物时，从固定 source archive 构建。
    #[serde(default)]
    pub build: Option<CmakeBuild>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CmakeBuild {
    pub source_directory: String,
    pub target: String,
    /// 不含 `-D` 前缀的 CMake cache definitions。
    #[serde(default)]
    pub definitions: Vec<String>,
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

/// 只读本地目录识别规则。规则是 manifest 数据，不允许携带可执行逻辑。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDetector {
    pub id: String,
    pub capability: String,
    pub adapter: String,
    #[serde(default)]
    pub required_files: Vec<String>,
    #[serde(default)]
    pub required_directories: Vec<String>,
    #[serde(default)]
    pub required_globs: Vec<RequiredGlob>,
    #[serde(default)]
    pub required_absent: Vec<String>,
    #[serde(default)]
    pub json_predicates: Vec<JsonPredicate>,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredGlob {
    pub pattern: String,
    #[serde(default = "one")]
    pub min_matches: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonPredicate {
    pub file: String,
    pub pointer: String,
    pub equals: serde_json::Value,
}

fn one() -> usize {
    1
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
        if Version::parse(&self.version).is_err() {
            return Err(ManifestError(
                "runner version must be valid SemVer".to_string(),
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
        for (index, argument) in self.entrypoint.command.iter().enumerate() {
            validate_command_argument(argument, index == 0)?;
        }
        if self.runtime.id.trim().is_empty() || self.runtime.runtime_type.trim().is_empty() {
            return Err(ManifestError(
                "runtime type and id must not be empty".to_string(),
            ));
        }
        if !valid_id(&self.runtime.id) {
            return Err(ManifestError(
                "runtime id must use lowercase ASCII letters, digits, '.' or '-'".to_string(),
            ));
        }
        validate_runtime_project(&self.runtime.project)?;
        validate_relative_path("runtime.lock", &self.runtime.lock)?;
        if self.runtime.runtime_type == "python-uv" {
            if self
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
            if self.runtime.probe.is_empty() {
                return Err(ManifestError(
                    "python-uv runtime requires a probe command".to_string(),
                ));
            }
        }
        for (index, argument) in self.runtime.probe.iter().enumerate() {
            validate_command_argument(argument, index == 0)?;
        }
        if self.capacity.max_instances == 0 || self.capacity.max_concurrency_per_instance == 0 {
            return Err(ManifestError(
                "runner capacity values must be positive".to_string(),
            ));
        }
        if self.capacity.max_instances != 1 || self.capacity.max_concurrency_per_instance != 1 {
            return Err(ManifestError(
                "macai.runner.v1 currently supports exactly one instance and one active inference"
                    .to_string(),
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
        // 环境继承白名单：代理变量（install/runtime 联网）+ HOME（受管
        // 引擎产物定位）+ 两个显式 engine override（开发/诊断）。
        // HOME 只读不改写，泄露面可控；其余变量仍拒绝。
        if self.security.inherit_environment.iter().any(|name| {
            !matches!(
                name.as_str(),
                "HOME"
                    | "HTTP_PROXY"
                    | "HTTPS_PROXY"
                    | "NO_PROXY"
                    | "MACAI_LLAMA_SERVER"
                    | "MACAI_WHISPER_SERVER"
            )
        }) {
            return Err(ManifestError(
                "runner may inherit only HOME, proxy variables, and approved engine overrides"
                    .to_string(),
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
        for detector in &self.local_detectors {
            if !valid_id(&detector.id)
                || !valid_capability(&detector.capability)
                || !self.capabilities.contains(&detector.capability)
                || detector.adapter.trim().is_empty()
                || detector.reason.trim().is_empty()
            {
                return Err(ManifestError(
                    "local detector must have an id, a declared capability, adapter and reason"
                        .to_string(),
                ));
            }
            if !self
                .models
                .iter()
                .any(|model| model.adapter == detector.adapter)
            {
                return Err(ManifestError(
                    "local detector adapter must be declared by this Runner".to_string(),
                ));
            }
            for path in detector
                .required_files
                .iter()
                .chain(&detector.required_directories)
                .chain(&detector.required_absent)
            {
                validate_relative_path("local detector path", path)?;
            }
            for required in &detector.required_globs {
                if required.min_matches == 0 || !valid_glob(&required.pattern) {
                    return Err(ManifestError("local detector glob must be a safe relative path with at most '*' wildcards".to_string()));
                }
            }
            for predicate in &detector.json_predicates {
                validate_relative_path("local detector JSON file", &predicate.file)?;
                if !predicate.pointer.starts_with('/') || !is_scalar_json(&predicate.equals) {
                    return Err(ManifestError(
                        "local detector JSON predicate requires a JSON Pointer and scalar value"
                            .to_string(),
                    ));
                }
            }
        }
        if let Some(engine) = &self.engine {
            if !(engine.download_url.starts_with("https://")
                && engine.sha256.len() == 64
                && engine.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()))
            {
                return Err(ManifestError(
                    "engine requires an HTTPS URL and a 64-character sha256".to_string(),
                ));
            }
            validate_relative_path("engine.binary", &engine.binary)?;
            if let Some(build) = &engine.build {
                validate_relative_path("engine.build.source_directory", &build.source_directory)?;
                if !valid_build_token(&build.target)
                    || build
                        .definitions
                        .iter()
                        .any(|value| !value.contains('=') || !valid_build_definition(value))
                {
                    return Err(ManifestError(
                        "engine CMake target or definitions contain unsafe characters".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// 校验 Runner 包根目录的实际内容。`[[models]]` 指向的 profile 文件必须
    /// 存在；无 `[[models]]` 的引擎（如 llama.cpp：任意 GGUF 注册时构造
    /// ad-hoc 绑定，无 bundled catalog）跳过 profile 检查。
    pub fn validate_package(&self, package_root: &Path) -> Result<(), ManifestError> {
        let root = package_root.canonicalize().map_err(|error| {
            ManifestError(format!(
                "cannot canonicalize package root {}: {error}",
                package_root.display()
            ))
        })?;
        let project = resolve_package_path(&root, &self.runtime.project)?;
        if !project.is_dir() {
            return Err(ManifestError(
                "runtime.project must resolve to a directory".to_string(),
            ));
        }
        let lock = resolve_package_path(&root, &self.runtime.lock)?;
        if !lock.is_file() {
            return Err(ManifestError(
                "runtime.lock must resolve to a file".to_string(),
            ));
        }
        for relative in self.models.iter().map(|model| &model.profile) {
            let profile = resolve_package_path(&root, relative)?;
            if !profile.is_file() {
                return Err(ManifestError(format!(
                    "models.profile '{relative}' must resolve to a file"
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
        self.resolve_argv(
            &self.entrypoint.command,
            package_root,
            environment_python,
            runtime_temp_root,
        )
    }

    /// Resolve the runtime probe argv against the staged package and the synced
    /// venv interpreter（`{environment.python}` 展开为环境运行解释器，
    /// 即 `<env_root>/.venv/bin/python`，而非 uv `--python` 输入的基础解释器）。
    pub fn resolve_probe(
        &self,
        package_root: &Path,
        environment_python: &Path,
        runtime_temp_root: &Path,
    ) -> Result<Vec<String>, ManifestError> {
        self.resolve_argv(
            &self.runtime.probe,
            package_root,
            environment_python,
            runtime_temp_root,
        )
    }

    fn resolve_argv(
        &self,
        command: &[String],
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
        command
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

fn valid_glob(value: &str) -> bool {
    !value.contains("**")
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b'*')
        })
}

fn is_scalar_json(value: &serde_json::Value) -> bool {
    matches!(
        value,
        serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
    )
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

fn validate_command_argument(argument: &str, is_program: bool) -> Result<(), ManifestError> {
    if argument.trim().is_empty()
        || ["|", ";", "&&", "||", "`", "$(", ">", "<"]
            .iter()
            .any(|forbidden| argument.contains(forbidden))
        || matches!(argument, "sh" | "bash" | "zsh" | "fish")
        || (is_program
            && !matches!(
                argument,
                "{environment.python}" | "{package.root}" | "{runtime.temp_root}"
            )
            && !is_safe_relative(argument))
    {
        return Err(ManifestError(
            "entrypoint command must use an approved executable template or package-relative argv"
                .to_string(),
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

fn validate_runtime_project(value: &str) -> Result<(), ManifestError> {
    if value != "." && !is_safe_relative(value) {
        return Err(ManifestError(
            "runtime.project must be '.' or a safe relative package directory".to_string(),
        ));
    }
    Ok(())
}

fn resolve_package_path(root: &Path, relative: &str) -> Result<std::path::PathBuf, ManifestError> {
    let candidate = root.join(relative).canonicalize().map_err(|error| {
        ManifestError(format!("cannot resolve package path '{relative}': {error}"))
    })?;
    if !candidate.starts_with(root) {
        return Err(ManifestError(format!(
            "package path '{relative}' escapes its root"
        )));
    }
    Ok(candidate)
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
    matches!(parts.next(), Some(version) if version.len() > 1 && version.starts_with('v') && version[1..].chars().all(|c| c.is_ascii_digit()))
        && matches!(parts.next(), Some(name) if !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
}

fn valid_build_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_build_definition(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'/' | b'=')
        })
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
project = "."
lock = "uv.lock"
python = ">=3.12,<3.13"
probe = ["{environment.python}", "-c", "print('probe')"]

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
        assert!(RunnerManifest::parse(&manifest().replace("fake-runner", "/bin/sh")).is_err());
        assert!(RunnerManifest::parse(&manifest().replace("uv.lock", "../uv.lock")).is_err());
        assert!(RunnerManifest::parse(&manifest().replace("tts.v1", "tts.v")).is_err());
    }

    #[test]
    fn accepts_package_root_as_the_runtime_project() {
        let manifest = RunnerManifest::parse(manifest()).unwrap();
        assert_eq!(manifest.runtime.project, ".");
    }

    #[test]
    fn python_uv_requires_probe_and_rejects_unsafe_probe_argv() {
        assert!(RunnerManifest::parse(&manifest().replace(
            "probe = [\"{environment.python}\", \"-c\", \"print('probe')\"]\n",
            ""
        ))
        .is_err());
        assert!(RunnerManifest::parse(&manifest().replace(
            "probe = [\"{environment.python}\", \"-c\", \"print('probe')\"]",
            "probe = [\"/bin/echo\"]"
        ))
        .is_err());
        assert!(RunnerManifest::parse(&manifest().replace(
            "probe = [\"{environment.python}\", \"-c\", \"print('probe')\"]",
            "probe = [\"{environment.python}\", \";\", \"rm\"]"
        ))
        .is_err());
    }

    #[test]
    fn invalid_runtime_id_is_rejected() {
        assert!(RunnerManifest::parse(
            &manifest().replace("id = \"org.example.fake-python\"", "id = \"Bad_Runtime\"")
        )
        .is_err());
    }

    #[test]
    fn v1_rejects_capacity_the_instance_manager_cannot_honor() {
        let error = RunnerManifest::parse(&manifest().replace(
            "max_concurrency_per_instance = 1",
            "max_concurrency_per_instance = 2",
        ))
        .unwrap_err();
        assert!(error.to_string().contains("exactly one instance"));
    }

    #[test]
    fn resolve_probe_expands_template_variables() {
        let manifest = RunnerManifest::parse(manifest()).unwrap();
        let package =
            std::env::temp_dir().join(format!("macai-manifest-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&package);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("pyproject.toml"), "[project]\nname='fake'\n").unwrap();
        std::fs::write(package.join("uv.lock"), "version = 1\n").unwrap();
        let resolved = manifest
            .resolve_probe(
                &package,
                Path::new("/managed/env/.venv/bin/python"),
                Path::new("/managed/temp"),
            )
            .unwrap();
        assert_eq!(
            resolved,
            vec![
                "/managed/env/.venv/bin/python".to_string(),
                "-c".to_string(),
                "print('probe')".to_string()
            ]
        );
        let _ = std::fs::remove_dir_all(package);
    }
}
