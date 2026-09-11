use std::fs;
use std::path::Path;

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::RunnerManifest;

const SCRIPT_SCHEMA: &str = "macai.script-runner.v1";
const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const HOST_SOURCE: &str = include_str!("script_host.py");

#[derive(Debug, Clone, Serialize)]
pub struct ScriptRunnerPreview {
    pub id: String,
    pub version: String,
    pub capability: String,
    pub adapter: String,
    pub model_format: String,
    pub requires_python: String,
    pub dependencies: Vec<String>,
    pub network_during_runtime: bool,
    pub source_digest: String,
    pub local_detector_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScriptDependencyPreset {
    pub id: &'static str,
    pub title: &'static str,
    pub requirements: &'static [&'static str],
}

const DEPENDENCY_PRESETS: &[ScriptDependencyPreset] = &[
    ScriptDependencyPreset {
        id: "stdlib",
        title: "仅 Python 标准库",
        requirements: &[],
    },
    ScriptDependencyPreset {
        id: "transformers",
        title: "Transformers + PyTorch",
        requirements: &["transformers", "torch"],
    },
    ScriptDependencyPreset {
        id: "mlx-lm",
        title: "MLX-LM",
        requirements: &["mlx-lm"],
    },
    ScriptDependencyPreset {
        id: "mlx-audio",
        title: "MLX-Audio",
        requirements: &["mlx-audio"],
    },
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptMetadata {
    #[serde(rename = "requires-python")]
    requires_python: String,
    dependencies: Vec<String>,
    tool: ScriptTools,
}

#[derive(Debug, Clone, Deserialize)]
struct ScriptTools {
    macai: ScriptRunner,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptRunner {
    schema: String,
    id: String,
    version: String,
    capability: String,
    adapter: String,
    model_format: String,
    #[serde(default)]
    network_during_runtime: bool,
    #[serde(default)]
    timeouts: ScriptTimeouts,
    #[serde(default)]
    local_detectors: Vec<ScriptDetector>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ScriptTimeouts {
    #[serde(default = "default_boot_seconds")]
    boot_seconds: u64,
    #[serde(default = "default_load_seconds")]
    load_seconds: u64,
    #[serde(default = "default_inference_seconds")]
    inference_seconds: u64,
    #[serde(default = "default_shutdown_seconds")]
    shutdown_seconds: u64,
}

impl Default for ScriptTimeouts {
    fn default() -> Self {
        Self {
            boot_seconds: default_boot_seconds(),
            load_seconds: default_load_seconds(),
            inference_seconds: default_inference_seconds(),
            shutdown_seconds: default_shutdown_seconds(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ScriptDetector {
    id: String,
    reason: String,
    #[serde(default)]
    required_files: Vec<String>,
    #[serde(default)]
    required_directories: Vec<String>,
    #[serde(default)]
    required_globs: Vec<ScriptGlob>,
    #[serde(default)]
    required_absent: Vec<String>,
    #[serde(default)]
    json_predicates: Vec<ScriptJsonPredicate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ScriptGlob {
    pattern: String,
    #[serde(default = "one")]
    min_matches: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ScriptJsonPredicate {
    file: String,
    pointer: String,
    equals: Value,
}

#[derive(Serialize)]
struct GeneratedManifest<'a> {
    schema: &'static str,
    id: &'a str,
    version: &'a str,
    protocols: [&'static str; 1],
    capabilities: [&'a str; 1],
    entrypoint: GeneratedEntrypoint,
    runtime: GeneratedRuntime<'a>,
    capacity: GeneratedCapacity,
    timeouts: &'a ScriptTimeouts,
    security: GeneratedSecurity,
    local_detectors: Vec<GeneratedDetector<'a>>,
}

#[derive(Serialize)]
struct GeneratedEntrypoint {
    command: [&'static str; 4],
    working_directory: &'static str,
}

#[derive(Serialize)]
struct GeneratedRuntime<'a> {
    #[serde(rename = "type")]
    runtime_type: &'static str,
    id: String,
    project: &'static str,
    lock: &'static str,
    python: &'a str,
    probe: [&'static str; 5],
    default_adapter: &'a str,
}

#[derive(Serialize)]
struct GeneratedCapacity {
    max_instances: u32,
    max_concurrency_per_instance: u32,
}

#[derive(Serialize)]
struct GeneratedSecurity {
    network_during_install: bool,
    network_during_runtime: bool,
    inherit_environment: Vec<String>,
}

#[derive(Serialize)]
struct GeneratedDetector<'a> {
    id: &'a str,
    capability: &'a str,
    adapter: &'a str,
    reason: &'a str,
    required_files: &'a [String],
    required_directories: &'a [String],
    required_globs: &'a [ScriptGlob],
    required_absent: &'a [String],
    json_predicates: &'a [ScriptJsonPredicate],
}

#[derive(Serialize)]
struct GeneratedProject<'a> {
    project: GeneratedProjectMetadata<'a>,
    tool: GeneratedProjectTools,
}

#[derive(Serialize)]
struct GeneratedProjectMetadata<'a> {
    name: String,
    version: &'static str,
    #[serde(rename = "requires-python")]
    requires_python: &'a str,
    dependencies: &'a [String],
}

#[derive(Serialize)]
struct GeneratedProjectTools {
    uv: GeneratedUv,
}

#[derive(Serialize)]
struct GeneratedUv {
    package: bool,
}

fn default_boot_seconds() -> u64 {
    30
}
fn default_load_seconds() -> u64 {
    300
}
fn default_inference_seconds() -> u64 {
    300
}
fn default_shutdown_seconds() -> u64 {
    10
}
fn one() -> usize {
    1
}

pub fn inspect_script_runner(source: &str) -> Result<ScriptRunnerPreview, String> {
    let metadata = parse(source)?;
    validate(&metadata)?;
    let manifest = generated_manifest(&metadata)?;
    RunnerManifest::parse(&manifest).map_err(|error| error.to_string())?;
    Ok(ScriptRunnerPreview {
        id: metadata.tool.macai.id.clone(),
        version: metadata.tool.macai.version.clone(),
        capability: metadata.tool.macai.capability.clone(),
        adapter: metadata.tool.macai.adapter.clone(),
        model_format: metadata.tool.macai.model_format.clone(),
        requires_python: metadata.requires_python.clone(),
        dependencies: metadata.dependencies.clone(),
        network_during_runtime: metadata.tool.macai.network_during_runtime,
        source_digest: source_digest(source),
        local_detector_ids: metadata
            .tool
            .macai
            .local_detectors
            .iter()
            .map(|detector| detector.id.clone())
            .collect(),
    })
}

pub fn materialize_script_runner(
    source: &str,
    destination: &Path,
) -> Result<ScriptRunnerPreview, String> {
    let preview = inspect_script_runner(source)?;
    let metadata = parse(source)?;
    fs::create_dir_all(destination)
        .map_err(|error| format!("cannot create {}: {error}", destination.display()))?;
    write(destination.join("runner.py"), source)?;
    write(destination.join("script_host.py"), HOST_SOURCE)?;
    write(
        destination.join("runner.toml"),
        &generated_manifest(&metadata)?,
    )?;
    let config = serde_json::json!({
        "id": preview.id,
        "version": preview.version,
        "capability": preview.capability,
    });
    write(
        destination.join("script_config.json"),
        &serde_json::to_string_pretty(&config).map_err(|error| error.to_string())?,
    )?;
    let project = GeneratedProject {
        project: GeneratedProjectMetadata {
            name: format!("macai-script-{}", preview.id.replace('.', "-")),
            version: "0.0.0",
            requires_python: &metadata.requires_python,
            dependencies: &metadata.dependencies,
        },
        tool: GeneratedProjectTools {
            uv: GeneratedUv { package: false },
        },
    };
    write(
        destination.join("pyproject.toml"),
        &toml::to_string_pretty(&project).map_err(|error| error.to_string())?,
    )?;
    Ok(preview)
}

pub fn script_runner_template(kind: &str) -> Option<&'static str> {
    match kind {
        "chat" => Some(CHAT_TEMPLATE),
        "stt" => Some(STT_TEMPLATE),
        "tts" => Some(TTS_TEMPLATE),
        _ => None,
    }
}

pub fn script_dependency_presets() -> &'static [ScriptDependencyPreset] {
    DEPENDENCY_PRESETS
}

pub fn normalize_script_dependencies(preset: &str, input: &str) -> Result<Vec<String>, String> {
    let preset = DEPENDENCY_PRESETS
        .iter()
        .find(|item| item.id == preset)
        .ok_or_else(|| format!("unknown dependency preset '{preset}'"))?;
    let mut requirements: Vec<String> = preset
        .requirements
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    let mut words = split_install_input(input)?;
    let command_words = match words.as_slice() {
        [python, flag, pip, install, ..]
            if python
                .rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with("python"))
                && flag == "-m"
                && pip == "pip"
                && install == "install" =>
        {
            4
        }
        [pip, install, ..] if matches!(pip.as_str(), "pip" | "pip3") && install == "install" => 2,
        [uv, add, ..] if uv == "uv" && add == "add" => 2,
        [uv, pip, install, ..] if uv == "uv" && pip == "pip" && install == "install" => 3,
        [] => 0,
        _ => 0,
    };
    words.drain(..command_words);
    for word in words {
        if matches!(word.as_str(), "-U" | "--upgrade") {
            continue;
        }
        if word.starts_with('-') {
            return Err(format!(
                "install option '{word}' is not supported; keep only package names"
            ));
        }
        if matches!(word.as_str(), "&&" | "||" | ";" | "|" | ">" | "<") {
            return Err("only one install command is allowed".to_string());
        }
        if !word.trim().is_empty() && !requirements.contains(&word) {
            requirements.push(word);
        }
    }
    if requirements.len() > 32 {
        return Err("at most 32 direct dependencies are allowed".to_string());
    }
    Ok(requirements)
}

fn split_install_input(input: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in input.chars() {
        if escaped {
            word.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if let Some(expected) = quote {
            if character == expected {
                quote = None;
            } else {
                word.push(character);
            }
        } else if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(character);
        }
    }
    if escaped || quote.is_some() {
        return Err("install command has invalid quotes or escaping".to_string());
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}

fn parse(source: &str) -> Result<ScriptMetadata, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err("script source exceeds 1 MiB".to_string());
    }
    let mut blocks = Vec::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if line != "# /// script" {
            continue;
        }
        let mut content = String::new();
        let mut closed = false;
        for line in lines.by_ref() {
            if line == "# ///" {
                closed = true;
                break;
            }
            let value = line
                .strip_prefix("# ")
                .or_else(|| line.strip_prefix('#'))
                .ok_or_else(|| "PEP 723 metadata lines must be comments".to_string())?;
            content.push_str(value);
            content.push('\n');
        }
        if !closed {
            return Err("unclosed PEP 723 script metadata block".to_string());
        }
        blocks.push(content);
    }
    if blocks.len() != 1 {
        return Err("script must contain exactly one PEP 723 script metadata block".to_string());
    }
    toml::from_str(&blocks[0]).map_err(|error| format!("invalid script metadata: {error}"))
}

fn validate(metadata: &ScriptMetadata) -> Result<(), String> {
    let runner = &metadata.tool.macai;
    if runner.schema != SCRIPT_SCHEMA {
        return Err(format!(
            "unsupported script runner schema '{}'",
            runner.schema
        ));
    }
    if Version::parse(&runner.version).is_err() {
        return Err("runner version must be valid SemVer".to_string());
    }
    if !matches!(runner.capability.as_str(), "chat.v1" | "stt.v1" | "tts.v1") {
        return Err("capability must be chat.v1, stt.v1 or tts.v1".to_string());
    }
    if runner.model_format != "directory" {
        return Err("single-file Runner v1 supports model_format = 'directory'".to_string());
    }
    if metadata.requires_python.trim().is_empty() || metadata.dependencies.len() > 128 {
        return Err(
            "requires-python is required and at most 128 dependencies are allowed".to_string(),
        );
    }
    if metadata
        .dependencies
        .iter()
        .any(|dependency| dependency.trim().is_empty())
    {
        return Err("dependencies must not contain empty entries".to_string());
    }
    if runner.adapter.trim().is_empty() {
        return Err("adapter must not be empty".to_string());
    }
    Ok(())
}

fn generated_manifest(metadata: &ScriptMetadata) -> Result<String, String> {
    let runner = &metadata.tool.macai;
    let manifest = GeneratedManifest {
        schema: "macai.runner.v1",
        id: &runner.id,
        version: &runner.version,
        protocols: ["macai.runner.v1"],
        capabilities: [&runner.capability],
        entrypoint: GeneratedEntrypoint {
            command: [
                "{environment.python}",
                "script_host.py",
                "runner.py",
                "script_config.json",
            ],
            working_directory: "package",
        },
        runtime: GeneratedRuntime {
            runtime_type: "python-uv",
            id: format!("{}-python", runner.id),
            project: ".",
            lock: "uv.lock",
            python: &metadata.requires_python,
            probe: [
                "{environment.python}",
                "script_host.py",
                "runner.py",
                "script_config.json",
                "--probe",
            ],
            default_adapter: &runner.adapter,
        },
        capacity: GeneratedCapacity {
            max_instances: 1,
            max_concurrency_per_instance: 1,
        },
        timeouts: &runner.timeouts,
        security: GeneratedSecurity {
            network_during_install: true,
            network_during_runtime: runner.network_during_runtime,
            inherit_environment: Vec::new(),
        },
        local_detectors: runner
            .local_detectors
            .iter()
            .map(|detector| GeneratedDetector {
                id: &detector.id,
                capability: &runner.capability,
                adapter: &runner.adapter,
                reason: &detector.reason,
                required_files: &detector.required_files,
                required_directories: &detector.required_directories,
                required_globs: &detector.required_globs,
                required_absent: &detector.required_absent,
                json_predicates: &detector.json_predicates,
            })
            .collect(),
    };
    toml::to_string_pretty(&manifest).map_err(|error| error.to_string())
}

fn source_digest(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.as_bytes()))
}

fn write(path: impl AsRef<Path>, contents: &str) -> Result<(), String> {
    let path = path.as_ref();
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

const CHAT_TEMPLATE: &str = r#"# MacAI 单文件 Chat Runner 完整示例
#
# 使用方法：
# 1. 修改下方 id，确保它在本机唯一。
# 2. 在 GUI 的“推理库”中选择预设或粘贴官方 pip install 命令；uv 会补全依赖树。
# 3. 用实际模型加载和推理代码替换 load()/chat()，保留函数签名。
#    如需报告设备或特殊清理，可再实现 describe(model) / unload(model)。
# 4. 创建一个模型目录，并放入 macai-example-chat.json：
#      {"prefix": "Echo: "}
#    管理页会用 local_detectors 识别该目录并路由到此 Runner。
#
# inspect 只读取这段 metadata，不会导入或执行本文件。用户点击“信任并添加”后，
# 代码才会在受管 Python worker 中以 aiworkd 当前用户权限运行。
# /// script
# requires-python = ">=3.12,<3.13"
# # 直接依赖使用标准 PEP 508 写法，例如：
# # dependencies = ["transformers>=4.56,<5", "safetensors>=0.6,<1"]
# dependencies = []
#
# [tool.macai]
# schema = "macai.script-runner.v1"
# # 全局唯一、稳定的 Runner ID；同 ID 不允许覆盖不同内容。
# id = "org.example.echo"
# version = "0.1.0"
# # v1 每个文件只声明一种能力：chat.v1 / stt.v1 / tts.v1。
# capability = "chat.v1"
# # adapter 用于把检测到的模型目录绑定到这个 Runner。
# adapter = "echo-chat"
# model_format = "directory"
# # 只有推理确实需要联网时才设为 true；安装依赖可能联网。
# network_during_runtime = false
# # 可选超时，单位为秒；不填写时使用 MacAI 默认值。
# # [tool.macai.timeouts]
# # load_seconds = 300
# # inference_seconds = 300
#
# [[tool.macai.local_detectors]]
# id = "example-echo-directory"
# reason = "包含 MacAI Chat 示例模型标记"
# required_files = ["macai-example-chat.json"]
# ///

import json
from pathlib import Path


def load(model_path, profile):
    """加载一次并返回模型对象；host 会持有它直到 unload。"""
    root = Path(model_path)
    marker = root / "macai-example-chat.json"
    if not marker.is_file():
        raise FileNotFoundError(f"missing {marker.name}")
    settings = json.loads(marker.read_text(encoding="utf-8"))
    return {"root": root, "prefix": settings.get("prefix", "Echo: ")}


def chat(model, messages, options):
    """返回字符串，或像这里一样逐段 yield 字符串以产生流式输出。"""
    prompt = next(
        (item.get("content", "") for item in reversed(messages) if item.get("role") == "user"),
        "",
    )
    yield model["prefix"]
    yield prompt
"#;

const STT_TEMPLATE: &str = r#"# MacAI 单文件 STT Runner 完整示例
#
# 使用方法：
# 1. 修改唯一 id，在 GUI 选择推理库预设或粘贴官方 pip install 命令。
# 2. 创建模型目录，在其中放入 macai-example-stt.json：
#      {"language": "zh"}
# 3. 示例会读取 daemon 提供的 PCM WAV 并返回音频信息；接入真实模型时只需替换
#    load()/transcribe() 的函数体。daemon 负责音频格式归一化、worker 与生命周期。
#    如需报告设备或特殊清理，可再实现 describe(model) / unload(model)。
# /// script
# requires-python = ">=3.12,<3.13"
# # 示例：dependencies = ["mlx-whisper>=0.4,<1"]
# dependencies = []
#
# [tool.macai]
# schema = "macai.script-runner.v1"
# # 全局唯一、稳定的 Runner ID。
# id = "org.example.transcriber"
# version = "0.1.0"
# capability = "stt.v1"
# adapter = "example-stt"
# model_format = "directory"
# network_during_runtime = false
# # 可选超时，单位为秒；长音频模型可提高 inference_seconds。
# # [tool.macai.timeouts]
# # load_seconds = 300
# # inference_seconds = 600
#
# [[tool.macai.local_detectors]]
# id = "example-stt-directory"
# reason = "包含 MacAI STT 示例模型标记"
# required_files = ["macai-example-stt.json"]
# ///

import json
import wave
from pathlib import Path


def load(model_path, profile):
    """返回模型对象。profile 包含 model_id 和 adapter。"""
    marker = Path(model_path) / "macai-example-stt.json"
    if not marker.is_file():
        raise FileNotFoundError(f"missing {marker.name}")
    return json.loads(marker.read_text(encoding="utf-8"))


def transcribe(model, wav_path, options):
    """接收 PCM WAV 路径并返回文字；options 含 language 等请求字段。"""
    with wave.open(wav_path, "rb") as audio:
        frames = audio.getnframes()
        sample_rate = audio.getframerate()
        channels = audio.getnchannels()
    duration = frames / sample_rate if sample_rate else 0
    language = options.get("language") or model.get("language", "auto")
    return f"[示例 STT] {duration:.2f}s, {sample_rate}Hz, {channels}ch, language={language}"
"#;

const TTS_TEMPLATE: &str = r#"# MacAI 单文件 TTS Runner 完整示例
#
# 使用方法：
# 1. 修改唯一 id，在 GUI 选择推理库预设或粘贴官方 pip install 命令。
# 2. 创建模型目录，在其中放入 macai-example-tts.json：
#      {"frequency_hz": 440}
# 3. 示例会生成可播放的 PCM WAV 提示音；接入真实模型时只需替换
#    load()/synthesize() 的函数体，并继续把 WAV 写到 output_path。
#    如需报告设备或特殊清理，可再实现 describe(model) / unload(model)。
# /// script
# requires-python = ">=3.12,<3.13"
# # 示例：dependencies = ["numpy>=2,<3", "soundfile>=0.13,<1"]
# dependencies = []
#
# [tool.macai]
# schema = "macai.script-runner.v1"
# # 全局唯一、稳定的 Runner ID。
# id = "org.example.synthesizer"
# version = "0.1.0"
# capability = "tts.v1"
# adapter = "example-tts"
# model_format = "directory"
# network_during_runtime = false
# # 可选超时，单位为秒；长文本合成可提高 inference_seconds。
# # [tool.macai.timeouts]
# # load_seconds = 300
# # inference_seconds = 600
#
# [[tool.macai.local_detectors]]
# id = "example-tts-directory"
# reason = "包含 MacAI TTS 示例模型标记"
# required_files = ["macai-example-tts.json"]
# ///

import json
import math
import struct
import wave
from pathlib import Path


def load(model_path, profile):
    """加载并返回模型或音色资源。"""
    marker = Path(model_path) / "macai-example-tts.json"
    if not marker.is_file():
        raise FileNotFoundError(f"missing {marker.name}")
    return json.loads(marker.read_text(encoding="utf-8"))


def synthesize(model, text, output_path, options):
    """把 WAV 写入 daemon 管理的 output_path；可返回该路径或 None。"""
    sample_rate = 24_000
    speed = max(0.25, min(float(options.get("speed") or 1.0), 4.0))
    duration = max(0.15, min(0.8, 0.35 / speed))
    frequency = float(model.get("frequency_hz", 440))
    frame_count = int(sample_rate * duration)
    with wave.open(output_path, "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(sample_rate)
        for index in range(frame_count):
            fade = min(1.0, index / 240, (frame_count - index) / 240)
            sample = int(8_000 * fade * math.sin(2 * math.pi * frequency * index / sample_rate))
            audio.writeframesraw(struct.pack("<h", sample))
    return output_path
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::{read_frame, write_frame, Envelope, DEFAULT_MAX_FRAME_BYTES};
    use serde_json::json;
    use std::process::Stdio;

    #[test]
    fn chat_template_materializes_as_a_standard_runner_package() {
        let root = std::env::temp_dir().join(format!("macai-script-runner-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let preview = materialize_script_runner(CHAT_TEMPLATE, &root).unwrap();
        fs::write(
            root.join("uv.lock"),
            "version = 1\nrevision = 3\nrequires-python = '>=3.12,<3.13'\n",
        )
        .unwrap();
        let manifest = RunnerManifest::load(&root.join("runner.toml")).unwrap();
        manifest.validate_package(&root).unwrap();
        assert_eq!(preview.id, "org.example.echo");
        assert_eq!(
            manifest.runtime.default_adapter.as_deref(),
            Some("echo-chat")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parser_rejects_duplicate_metadata_blocks() {
        let source = format!("{CHAT_TEMPLATE}\n{CHAT_TEMPLATE}");
        assert!(inspect_script_runner(&source)
            .unwrap_err()
            .contains("exactly one"));
    }

    #[test]
    fn dependency_helper_keeps_only_direct_requirements() {
        assert_eq!(
            normalize_script_dependencies(
                "transformers",
                "python3 -m pip install -U sentencepiece transformers"
            )
            .unwrap(),
            ["transformers", "torch", "sentencepiece"]
        );
        assert!(
            normalize_script_dependencies("stdlib", "pip install --index-url example.org x")
                .unwrap_err()
                .contains("not supported")
        );
    }

    #[tokio::test]
    async fn generated_host_runs_all_template_examples() {
        for kind in ["chat", "stt", "tts"] {
            exercise_template(kind).await;
        }
    }

    async fn exercise_template(kind: &str) {
        let root = std::env::temp_dir().join(format!(
            "macai-script-host-{kind}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        materialize_script_runner(script_runner_template(kind).unwrap(), &root).unwrap();
        let marker = match kind {
            "chat" => ("macai-example-chat.json", r#"{"prefix":"Echo: "}"#),
            "stt" => ("macai-example-stt.json", r#"{"language":"zh"}"#),
            "tts" => ("macai-example-tts.json", r#"{"frequency_hz":440}"#),
            _ => unreachable!(),
        };
        fs::write(root.join(marker.0), marker.1).unwrap();
        let audio_path = root.join("input.wav");
        if kind == "stt" {
            write_silent_wav(&audio_path);
        }
        let output_dir = root.join("output");
        fs::create_dir_all(&output_dir).unwrap();
        let mut child = tokio::process::Command::new("python3")
            .args(["script_host.py", "runner.py", "script_config.json"])
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();

        let hello = read_frame(&mut stdout, DEFAULT_MAX_FRAME_BYTES)
            .await
            .unwrap();
        assert_eq!(hello.message_type, "hello");
        write_frame(&mut stdin, &Envelope::new("initialize", "init", json!({})))
            .await
            .unwrap();
        assert_eq!(
            read_frame(&mut stdout, DEFAULT_MAX_FRAME_BYTES)
                .await
                .unwrap()
                .message_type,
            "initialized"
        );
        write_frame(
            &mut stdin,
            &Envelope::new(
                "load",
                "load",
                json!({"model_id":"example", "model_root":root.display().to_string()}),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut stdout, DEFAULT_MAX_FRAME_BYTES)
                .await
                .unwrap()
                .message_type,
            "loaded"
        );
        let request = match kind {
            "chat" => json!({"messages":[{"role":"user","content":"hello"}]}),
            "stt" => json!({"audio":audio_path.display().to_string(), "language":"zh"}),
            "tts" => json!({"text":"hello", "speed":1.0}),
            _ => unreachable!(),
        };
        let capability = match kind {
            "chat" => "chat.v1",
            "stt" => "stt.v1",
            "tts" => "tts.v1",
            _ => unreachable!(),
        };
        write_frame(
            &mut stdin,
            &Envelope::new(
                "infer",
                "infer",
                json!({
                    "capability":capability,
                    "request":request,
                    "output":{"directory":output_dir.display().to_string()}
                }),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut stdout, DEFAULT_MAX_FRAME_BYTES)
                .await
                .unwrap()
                .message_type,
            "accepted"
        );
        let mut result = read_frame(&mut stdout, DEFAULT_MAX_FRAME_BYTES)
            .await
            .unwrap();
        if kind == "chat" {
            assert_eq!(result.message_type, "delta");
            assert_eq!(result.payload["text"], "Echo: ");
            result = read_frame(&mut stdout, DEFAULT_MAX_FRAME_BYTES)
                .await
                .unwrap();
            assert_eq!(result.message_type, "delta");
            assert_eq!(result.payload["text"], "hello");
            result = read_frame(&mut stdout, DEFAULT_MAX_FRAME_BYTES)
                .await
                .unwrap();
        }
        assert_eq!(result.message_type, "result");
        match kind {
            "chat" => assert_eq!(result.payload["text"], "Echo: hello"),
            "stt" => assert!(result.payload["text"]
                .as_str()
                .unwrap()
                .contains("[示例 STT]")),
            "tts" => assert!(fs::metadata(output_dir.join("output.wav")).unwrap().len() > 44),
            _ => unreachable!(),
        }
        write_frame(&mut stdin, &Envelope::new("unload", "unload", json!({})))
            .await
            .unwrap();
        assert_eq!(
            read_frame(&mut stdout, DEFAULT_MAX_FRAME_BYTES)
                .await
                .unwrap()
                .message_type,
            "unloaded"
        );
        write_frame(
            &mut stdin,
            &Envelope::new("shutdown", "shutdown", json!({})),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut stdout, DEFAULT_MAX_FRAME_BYTES)
                .await
                .unwrap()
                .message_type,
            "shutdown_complete"
        );
        assert!(child.wait().await.unwrap().success());
        let _ = fs::remove_dir_all(root);
    }

    fn write_silent_wav(path: &Path) {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&38u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&16_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&2u32.to_le_bytes());
        wav.extend_from_slice(&0i16.to_le_bytes());
        fs::write(path, wav).unwrap();
    }
}
