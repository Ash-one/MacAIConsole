//! macai — aiworkd 的命令行客户端（文档 §4.3、§18）。
//!
//! CLI 只负责输入、输出和 Runtime 管理请求；模型与推理状态统一由 aiworkd 持有。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

const DEFAULT_ADDR: &str = "127.0.0.1:11435";
const LONG_ABOUT: &str = "MacAI 本地 AI Runtime 的命令行客户端。\n\n\
所有命令都连接 aiworkd；CLI 不会另起一套推理 Runtime。查询命令可用于观察模型、\
Provider 和任务状态，管理命令可加载、停止或配置已注册模型。";
const AFTER_HELP: &str = "常用示例：
  macai status
  macai list
  macai load ./model.gguf --id local-model
  macai chat local-model \"用一句话介绍你自己\" --temperature 0.2
  macai run local-model --system \"回答保持简洁\"
  macai tasks --limit 10
  macai --addr 127.0.0.1:11435 providers

使用“macai <命令> --help”查看某个命令的参数。";

#[derive(Parser)]
#[command(
    name = "macai",
    version,
    about = "MacAI 本地 AI Runtime 命令行客户端",
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP,
    arg_required_else_help = true
)]
struct Cli {
    /// aiworkd 地址。
    #[arg(
        short = 'a',
        long,
        global = true,
        default_value = DEFAULT_ADDR,
        value_name = "HOST:PORT"
    )]
    addr: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 查看 daemon、请求和系统内存概况。
    Status,
    /// 列出已注册模型及其加载状态。
    #[command(name = "list", visible_alias = "models")]
    Models,
    /// 发送一次非流式 Chat Completion。
    Chat {
        /// 模型 ID 或本地 GGUF 路径。
        model: String,
        /// 用户消息；传入 - 时从标准输入读取。
        message: String,
        /// 置于对话开头的 system prompt。
        #[arg(long)]
        system: Option<String>,
        /// 采样温度。
        #[arg(long)]
        temperature: Option<f64>,
        /// 最大生成 token 数。
        #[arg(long)]
        max_tokens: Option<u64>,
    },
    /// 注册并加载本地 LLM、STT 或 TTS 模型。
    Load {
        /// 模型文件或目录路径。
        path: String,
        /// 注册 ID；缺省时使用文件名。
        #[arg(long)]
        id: Option<String>,
        /// 展示名称；缺省时与 ID 相同。
        #[arg(long)]
        name: Option<String>,
        /// 模型类型。
        #[arg(long = "type", value_enum, default_value = "llm")]
        model_type: ModelType,
        /// 推理后端，如 qwen3-asr；省略时按模型类型选择默认值。
        #[arg(long)]
        provider: Option<String>,
        /// LLM 上下文长度。
        #[arg(long, default_value_t = 4096)]
        context_length: u64,
        /// 空闲驻留策略，如 0、5m、2h 或 always。
        #[arg(long)]
        keep_alive: Option<String>,
    },
    /// 加载注册表中已有的模型。
    Start { model: String },
    /// 停止模型 worker 并释放内存；注册记录仍保留。
    #[command(visible_alias = "stop")]
    Unload { model: String },
    /// 删除模型注册记录；模型文件仍保留。
    #[command(visible_alias = "rm")]
    Remove { model: String },
    /// 修改模型 ID；运行中的模型会先停止。
    Rename { model: String, new_id: String },
    /// 设置模型空闲驻留策略，如 0、5m、2h 或 always。
    KeepAlive { model: String, value: String },
    /// 从 HuggingFace 下载单文件到模型仓库并注册加载（文档 §20）。
    Pull {
        /// HF 仓库，如 Qwen/Qwen2.5-0.5B-Instruct-GGUF
        repo: String,
        /// 仓库内文件路径，如 qwen2.5-0.5b-instruct-q4_k_m.gguf
        filename: String,
        /// 模型类型。
        #[arg(long = "type", value_enum, default_value = "llm")]
        model_type: ModelType,
        /// 注册 ID；缺省用文件名去扩展名
        #[arg(long)]
        id: Option<String>,
    },
    /// 启动带上下文记忆的交互式聊天。
    Run {
        /// 模型 ID 或本地 GGUF 路径。
        model: String,
        /// 置于对话开头的 system prompt。
        #[arg(long)]
        system: Option<String>,
        /// 采样温度。
        #[arg(long)]
        temperature: Option<f64>,
        /// 每轮最大生成 token 数。
        #[arg(long)]
        max_tokens: Option<u64>,
    },
    /// 使用本地 STT 模型转写 PCM WAV。
    Transcribe {
        file: String,
        #[arg(long, default_value = "whisper-base")]
        model: String,
        #[arg(long)]
        language: Option<String>,
    },
    /// 使用本地 TTS Provider 生成 WAV。
    Speak {
        /// 要合成的文本；传入 - 时从标准输入读取。
        text: String,
        #[arg(short, long, default_value = "speech.wav")]
        output: String,
        #[arg(long, default_value = "")]
        model: String,
        #[arg(long, default_value = "zf_001")]
        voice: String,
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
    },
    /// 列出运行中的模型、内存、设备和驻留策略。
    Ps,
    /// 查看 Provider 可用性、设备和错误原因。
    Providers,
    /// 查看近期任务，或按 ID 输出完整任务详情。
    Tasks {
        /// 可选任务 ID。
        id: Option<String>,
        /// 列表中最多包含的已完成任务数。
        #[arg(short, long, default_value_t = 20, value_name = "N")]
        limit: usize,
    },
    /// 查看或修改 daemon 日志级别。
    Logging {
        /// 省略时只查询当前级别。
        #[arg(value_enum)]
        level: Option<LogLevel>,
    },
    /// 列出、设置或清除 TTS 模型的默认音色。
    Voice {
        model: String,
        /// 新的默认音色；省略时列出可用音色。
        voice: Option<String>,
        /// 清除已设置的默认音色。
        #[arg(long, conflicts_with = "voice")]
        clear: bool,
    },
    /// 检查 API server；未运行时给出启动提示（文档 §18 Server）。
    Serve,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ModelType {
    Llm,
    Stt,
    Tts,
}

impl ModelType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::Stt => "stt",
            Self::Tts => "tts",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LogLevel {
    Info,
    Debug,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let base = format!("http://{}", cli.addr);
    let result = match cli.command {
        Commands::Status => cmd_status(&base),
        Commands::Models => cmd_models(&base),
        Commands::Chat {
            model,
            message,
            system,
            temperature,
            max_tokens,
        } => read_text_argument(&message).and_then(|message| {
            cmd_chat(
                &base,
                &model,
                &message,
                system.as_deref(),
                temperature,
                max_tokens,
            )
        }),
        Commands::Load {
            path,
            id,
            name,
            model_type,
            provider,
            context_length,
            keep_alive,
        } => cmd_load(
            &base,
            &path,
            LoadOptions {
                id: id.as_deref(),
                name: name.as_deref(),
                model_type,
                provider: provider.as_deref(),
                context_length,
                keep_alive: keep_alive.as_deref(),
            },
        )
        .map(|_| ()),
        Commands::Start { model } => cmd_start(&base, &model),
        Commands::Unload { model } => cmd_unload(&base, &model),
        Commands::Remove { model } => cmd_remove(&base, &model),
        Commands::Rename { model, new_id } => cmd_rename(&base, &model, &new_id),
        Commands::KeepAlive { model, value } => cmd_keep_alive(&base, &model, &value),
        Commands::Pull {
            repo,
            filename,
            model_type,
            id,
        } => cmd_pull(&base, &repo, &filename, model_type, id.as_deref()),
        Commands::Run {
            model,
            system,
            temperature,
            max_tokens,
        } => cmd_run(&base, &model, system.as_deref(), temperature, max_tokens),
        Commands::Transcribe {
            file,
            model,
            language,
        } => cmd_transcribe(&base, &file, &model, language.as_deref()),
        Commands::Speak {
            text,
            output,
            model,
            voice,
            speed,
        } => read_text_argument(&text)
            .and_then(|text| cmd_speak(&base, &text, &output, &model, &voice, speed)),
        Commands::Ps => cmd_ps(&base),
        Commands::Providers => cmd_providers(&base),
        Commands::Tasks { id, limit } => cmd_tasks(&base, id.as_deref(), limit),
        Commands::Logging { level } => cmd_logging(&base, level),
        Commands::Voice {
            model,
            voice,
            clear,
        } => cmd_voice(&base, &model, voice.as_deref(), clear),
        Commands::Serve => cmd_serve(&base),
    };
    if let Err(e) = result {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

// ---------- helpers ----------

/// 最简单的 HTTP 请求：只做 GET 和 POST JSON，返回 (status, body)。
/// 原型用 std TcpStream 手写，避免为 CLI 引入重型 HTTP 依赖；
/// 后续可换 reqwest/ureq。
fn http(method: &str, url: &str, body: Option<&str>) -> Result<(u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported url: {url}"))?;
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = if path.is_empty() { "/" } else { path };

    let mut stream =
        TcpStream::connect(host_port).map_err(|e| format!("cannot connect to {host_port}: {e}"))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(300)))
        .ok();

    let body_str = body.unwrap_or("");
    let content_len = body_str.len();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\nContent-Length: {content_len}\r\nConnection: close\r\n\r\n{body_str}"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| format!("read failed: {e}"))?;

    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line: {status_line:?}"))?;

    // 跳过 headers，读取 body。
    let mut body = String::new();
    for line in reader.by_ref().lines() {
        let line = line.map_err(|e| format!("read header failed: {e}"))?;
        if line.is_empty() {
            break;
        }
    }
    reader
        .read_to_string(&mut body)
        .map_err(|e| format!("read body failed: {e}"))?;

    Ok((status, body))
}

fn http_binary(
    method: &str,
    url: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(u16, Vec<u8>), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported url: {url}"))?;
    let (host_port, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let mut stream =
        TcpStream::connect(host_port).map_err(|error| format!("cannot connect: {error}"))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(300)))
        .ok();
    let headers = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(headers.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|error| format!("write failed: {error}"))?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|error| format!("read failed: {error}"))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| format!("bad status line: {status_line:?}"))?;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| format!("read header failed: {error}"))?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
    }
    let mut response = Vec::new();
    reader
        .read_to_end(&mut response)
        .map_err(|error| format!("read body failed: {error}"))?;
    Ok((status, response))
}

fn response_error(status: u16, body: &[u8]) -> String {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let message = value["error"]["message"]
        .as_str()
        .unwrap_or("unknown error");
    let kind = value["error"]["type"].as_str().unwrap_or("error");
    format!("{kind} (HTTP {status}): {message}")
}

fn post_json(base: &str, path: &str, body: &Value) -> Result<(u16, Value), String> {
    let (status, text) = http("POST", &format!("{base}{path}"), Some(&body.to_string()))?;
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok((status, parsed))
}

fn get_json(base: &str, path: &str) -> Result<(u16, Value), String> {
    let (status, text) = http("GET", &format!("{base}{path}"), None)?;
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok((status, parsed))
}

fn delete_json(base: &str, path: &str) -> Result<(u16, Value), String> {
    let (status, text) = http("DELETE", &format!("{base}{path}"), None)?;
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok((status, parsed))
}

fn json_response_error(status: u16, response: &Value) -> String {
    let message = response["error"]["message"]
        .as_str()
        .unwrap_or("unknown error");
    let kind = response["error"]["type"].as_str().unwrap_or("error");
    format!("{kind} (HTTP {status}): {message}")
}

fn expect_success(status: u16, response: &Value) -> Result<(), String> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(json_response_error(status, response))
    }
}

fn read_text_argument(value: &str) -> Result<String, String> {
    if value != "-" {
        return Ok(value.to_string());
    }
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read stdin: {error}"))?;
    if text.trim().is_empty() {
        Err("stdin did not contain any text".to_string())
    } else {
        Ok(text)
    }
}

fn print_pretty_json(value: &Value) -> Result<(), String> {
    let output = serde_json::to_string_pretty(value)
        .map_err(|error| format!("cannot format response: {error}"))?;
    println!("{output}");
    Ok(())
}

// ---------- commands ----------

fn cmd_status(base: &str) -> Result<(), String> {
    let (status, body) = get_json(base, "/health")?;
    if status != 200 {
        return Err(json_response_error(status, &body));
    }
    let version = body["version"].as_str().unwrap_or("?");
    let model_count = body["model_count"].as_u64().unwrap_or(0);
    println!(
        "AI Runtime running (v{}, {} model(s) registered)",
        version, model_count
    );
    println!("{}", base);
    if let Ok((200, runtime)) = get_json(base, "/api/runtime") {
        let pid = runtime["pid"].as_u64().unwrap_or(0);
        let uptime = format_duration_secs(runtime["uptime_secs"].as_u64().unwrap_or(0));
        let active = runtime["active_requests"].as_u64().unwrap_or(0);
        println!("PID {pid} · uptime {uptime} · {active} active request(s)");

        if let (Some(used), Some(total)) = (
            runtime["memory_used"].as_u64(),
            runtime["memory_total"].as_u64(),
        ) {
            let budget = runtime["memory_budget"].as_u64().map(format_bytes);
            let budget = budget
                .map(|value| format!(" · AI budget {value}"))
                .unwrap_or_default();
            println!(
                "System memory {} / {}{}",
                format_bytes(used),
                format_bytes(total),
                budget
            );
        }
    }
    Ok(())
}

fn cmd_models(base: &str) -> Result<(), String> {
    let (status, body) = get_json(base, "/v1/models")?;
    if status != 200 {
        return Err(json_response_error(status, &body));
    }
    let states: Option<HashMap<String, String>> = get_json(base, "/api/runtime")
        .ok()
        .filter(|(status, _)| *status == 200)
        .and_then(|(_, runtime)| runtime["loaded_models"].as_array().cloned())
        .map(|models| {
            models
                .into_iter()
                .filter_map(|model| {
                    Some((
                        model["id"].as_str()?.to_string(),
                        model["state"].as_str().unwrap_or("loaded").to_string(),
                    ))
                })
                .collect()
        });

    println!("{:<32} {:<6} {:<10} PROVIDER", "NAME", "TYPE", "STATUS");
    println!("{:<32} {:<6} {:<10} --------", "----", "----", "------");
    if let Some(data) = body["data"].as_array() {
        for m in data {
            let id = m["id"].as_str().unwrap_or("?");
            let provider = m["owned_by"]
                .as_str()
                .unwrap_or("?")
                .strip_prefix("aiworkd/")
                .unwrap_or_else(|| m["owned_by"].as_str().unwrap_or("?"));
            println!(
                "{:<32} {:<6} {:<10} {}",
                id,
                m["type"].as_str().unwrap_or("?"),
                states
                    .as_ref()
                    .and_then(|states| states.get(id))
                    .map(String::as_str)
                    .unwrap_or(if states.is_some() {
                        "unloaded"
                    } else {
                        "unknown"
                    }),
                provider
            );
        }
    }
    Ok(())
}

fn cmd_chat(
    base: &str,
    model: &str,
    message: &str,
    system: Option<&str>,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
) -> Result<(), String> {
    let model = resolve_model_argument(base, model)?;
    let mut messages = Vec::new();
    if let Some(system) = system {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": message}));
    let body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "temperature": temperature,
        "max_tokens": max_tokens,
    });
    let (status, resp) = post_json(base, "/v1/chat/completions", &body)?;
    if status != 200 {
        return Err(json_response_error(status, &resp));
    }
    if let Some(content) = resp["choices"][0]["message"]["content"].as_str() {
        println!("{}", content);
    }
    Ok(())
}

fn cmd_run(
    base: &str,
    model: &str,
    system: Option<&str>,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
) -> Result<(), String> {
    let model = resolve_model_argument(base, model)?;
    println!("Interactive chat with '{model}' (Ctrl-D / 'exit' to quit)");
    let stdin = std::io::stdin();
    let mut history: Vec<(String, String)> = Vec::new();
    loop {
        print!("> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let line = line.trim().to_string();
        if line.is_empty() || line == "exit" || line == "quit" {
            break;
        }

        let mut messages: Vec<Value> = system
            .map(|content| vec![json!({"role": "system", "content": content})])
            .unwrap_or_default();
        messages.extend(history.iter().flat_map(|(u, a)| {
            vec![
                json!({"role": "user", "content": u}),
                json!({"role": "assistant", "content": a}),
            ]
        }));
        messages.push(json!({"role": "user", "content": line}));

        let body = json!({
            "model": model,
            "messages": messages,
            "stream": false,
            "temperature": temperature,
            "max_tokens": max_tokens,
        });
        let (status, resp) = post_json(base, "/v1/chat/completions", &body)?;
        if status != 200 {
            let msg = resp["error"]["message"].as_str().unwrap_or("unknown error");
            eprintln!("error: {msg}");
            continue;
        }
        let content = resp["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        println!("{}", content);
        history.push((line, content));
    }
    Ok(())
}

fn resolve_model_argument(base: &str, model: &str) -> Result<String, String> {
    if Path::new(model).is_file() {
        cmd_load(
            base,
            model,
            LoadOptions {
                id: None,
                name: None,
                model_type: ModelType::Llm,
                provider: None,
                context_length: 4096,
                keep_alive: None,
            },
        )
    } else {
        Ok(model.to_string())
    }
}

struct LoadOptions<'a> {
    id: Option<&'a str>,
    name: Option<&'a str>,
    model_type: ModelType,
    provider: Option<&'a str>,
    context_length: u64,
    keep_alive: Option<&'a str>,
}

fn cmd_load(base: &str, path: &str, options: LoadOptions<'_>) -> Result<String, String> {
    let body = json!({
        "path": path,
        "id": options.id,
        "name": options.name,
        "model_type": options.model_type.as_str(),
        "provider": options.provider,
        "context_length": options.context_length,
        "keep_alive": options.keep_alive,
    });
    let (status, response) = post_json(base, "/api/models/load", &body)?;
    if status != 200 {
        return Err(json_response_error(status, &response));
    }
    let model_id = response["id"]
        .as_str()
        .ok_or_else(|| format!("invalid load response: {response}"))?
        .to_string();
    let provider = response["provider"].as_str().unwrap_or("unknown provider");
    println!("Loaded {model_id} with {provider}");
    Ok(model_id)
}

fn cmd_start(base: &str, model: &str) -> Result<(), String> {
    let (status, response) = post_json(base, &format!("/api/models/{model}/load"), &json!({}))?;
    expect_success(status, &response)?;
    let provider = response["provider"].as_str().unwrap_or("unknown provider");
    println!("Started {model} with {provider}");
    Ok(())
}

fn cmd_unload(base: &str, model: &str) -> Result<(), String> {
    let (status, response) = post_json(base, &format!("/api/models/{model}/unload"), &json!({}))?;
    expect_success(status, &response)?;
    println!("Unloaded {model}");
    Ok(())
}

fn cmd_remove(base: &str, model: &str) -> Result<(), String> {
    let (status, response) = delete_json(base, &format!("/api/models/{model}"))?;
    expect_success(status, &response)?;
    println!("Removed registration for {model}; model files were kept");
    Ok(())
}

fn cmd_rename(base: &str, model: &str, new_id: &str) -> Result<(), String> {
    let (status, response) = post_json(
        base,
        &format!("/api/models/{model}/rename"),
        &json!({"new_id": new_id}),
    )?;
    expect_success(status, &response)?;
    println!("Renamed {model} to {new_id}");
    Ok(())
}

fn cmd_keep_alive(base: &str, model: &str, value: &str) -> Result<(), String> {
    let (status, response) = post_json(
        base,
        &format!("/api/models/{model}/keep-alive"),
        &json!({"keep_alive": value}),
    )?;
    expect_success(status, &response)?;
    println!("Set {model} keep-alive to {value}");
    Ok(())
}

/// macai pull：下载可能耗时数分钟。daemon 的 HTTP 读超时是 300s，大文件靠
/// daemon 端流式写盘；CLI 侧提示进度由 daemon 日志承载。
fn cmd_pull(
    base: &str,
    repo: &str,
    filename: &str,
    model_type: ModelType,
    id: Option<&str>,
) -> Result<(), String> {
    let model_type = model_type.as_str();
    println!("Pulling {repo}/{filename} (type={model_type}) …");
    println!("（下载经 daemon 流式落盘，进度见 aiworkd 日志；大文件请耐心等待）");
    let body = json!({
        "repo": repo,
        "filename": filename,
        "model_type": model_type,
        "id": id,
    });
    // 复用 http() 的 300s 读超时；超过则报错，.part 保留可重入续传。
    let (status, response) = post_json(base, "/api/models/pull", &body)?;
    if status != 200 {
        return Err(json_response_error(status, &response));
    }
    let model_id = response["id"]
        .as_str()
        .ok_or_else(|| format!("invalid pull response: {response}"))?;
    println!("Pulled and loaded {model_id}");
    Ok(())
}

fn cmd_transcribe(
    base: &str,
    file: &str,
    model: &str,
    language: Option<&str>,
) -> Result<(), String> {
    let path = Path::new(file)
        .canonicalize()
        .map_err(|error| format!("cannot open audio '{file}': {error}"))?;
    if path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("wav"))
        != Some(true)
    {
        return Err("transcribe currently accepts .wav files".to_string());
    }
    let audio = std::fs::read(&path).map_err(|error| format!("cannot read audio: {error}"))?;
    let boundary = format!(
        "----macai-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    );
    let mut body = Vec::new();
    append_multipart_text(&mut body, &boundary, "model", model);
    if let Some(language) = language {
        append_multipart_text(&mut body, &boundary, "language", language);
    }
    append_multipart_text(&mut body, &boundary, "response_format", "json");
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let (status, response) = http_binary(
        "POST",
        &format!("{base}/v1/audio/transcriptions"),
        &format!("multipart/form-data; boundary={boundary}"),
        &body,
    )?;
    if status != 200 {
        return Err(response_error(status, &response));
    }
    let value: Value = serde_json::from_slice(&response)
        .map_err(|error| format!("invalid transcription response: {error}"))?;
    println!("{}", value["text"].as_str().unwrap_or_default());
    Ok(())
}

fn append_multipart_text(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
    );
}

fn cmd_speak(
    base: &str,
    text: &str,
    output: &str,
    model: &str,
    voice: &str,
    speed: f64,
) -> Result<(), String> {
    if !(0.25..=4.0).contains(&speed) {
        return Err("speech speed must be between 0.25 and 4.0".to_string());
    }
    // 未指定模型时自动选第一个可用的 TTS 模型（管理面已不含测试 provider）。
    let resolved = if model.is_empty() {
        let (status, body) = get_json(base, "/v1/models")?;
        if status != 200 {
            return Err(format!("cannot list models (HTTP {status})"));
        }
        let found = body["data"]
            .as_array()
            .and_then(|models| {
                models
                    .iter()
                    .find(|m| m["type"] == "tts")
                    .and_then(|m| m["id"].as_str())
            })
            .ok_or("no TTS model available; load one with 'macai pull' first")?
            .to_string();
        println!("Using TTS model: {found}");
        found
    } else {
        model.to_string()
    };
    let request = json!({
        "model": resolved,
        "input": text,
        "voice": voice,
        "format": "wav",
        "speed": speed,
    });
    let bytes = request.to_string().into_bytes();
    let (status, response) = http_binary(
        "POST",
        &format!("{base}/v1/audio/speech"),
        "application/json",
        &bytes,
    )?;
    if status != 200 {
        return Err(response_error(status, &response));
    }
    if response.len() < 44 || &response[0..4] != b"RIFF" || &response[8..12] != b"WAVE" {
        return Err("daemon returned invalid WAV data".to_string());
    }
    std::fs::write(output, &response)
        .map_err(|error| format!("cannot write '{output}': {error}"))?;
    println!("Wrote {} bytes to {output}", response.len());
    Ok(())
}

fn cmd_ps(base: &str) -> Result<(), String> {
    let (status, body) = get_json(base, "/api/runtime")?;
    if status != 200 {
        return Err(json_response_error(status, &body));
    }
    println!(
        "{:<30} {:<5} {:<12} {:>9} {:<8} {:<8} KEEP-ALIVE",
        "MODEL", "TYPE", "PROVIDER", "MEMORY", "DEVICE", "STATE"
    );
    println!(
        "{:<30} {:<5} {:<12} {:>9} {:<8} {:<8} ----------",
        "-----", "----", "--------", "------", "------", "-----"
    );
    if let Some(models) = body["loaded_models"].as_array() {
        for m in models {
            let memory = m["memory_usage_bytes"]
                .as_u64()
                .or_else(|| m["memory_estimate"].as_u64())
                .map(format_bytes)
                .unwrap_or_else(|| "-".to_string());
            println!(
                "{:<30} {:<5} {:<12} {:>9} {:<8} {:<8} {}",
                m["id"].as_str().unwrap_or("?"),
                m["model_type"].as_str().unwrap_or("?"),
                m["provider"].as_str().unwrap_or("?"),
                memory,
                m["effective_device"].as_str().unwrap_or("-"),
                m["state"].as_str().unwrap_or("?"),
                m["keep_alive"].as_str().unwrap_or("always")
            );
        }
    }
    Ok(())
}

fn cmd_providers(base: &str) -> Result<(), String> {
    let (status, body) = get_json(base, "/api/providers")?;
    if status != 200 {
        return Err(json_response_error(status, &body));
    }
    println!(
        "{:<16} {:<18} {:<6} {:<6} {:<8} DETAIL",
        "PROVIDER", "CAPABILITY", "AVAIL", "READY", "DEVICE"
    );
    println!(
        "{:<16} {:<18} {:<6} {:<6} {:<8} ------",
        "--------", "----------", "-----", "-----", "------"
    );
    if let Some(entries) = body["data"].as_array() {
        for entry in entries {
            let descriptor = &entry["descriptor"];
            let state = &entry["status"];
            let capabilities = descriptor["capabilities"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "-".to_string());
            let detail = state["reason"]
                .as_str()
                .or_else(|| state["install_hint"].as_str())
                .unwrap_or("-");
            println!(
                "{:<16} {:<18} {:<6} {:<6} {:<8} {}",
                descriptor["id"].as_str().unwrap_or("?"),
                capabilities,
                yes_no(state["available"].as_bool().unwrap_or(false)),
                yes_no(state["ready"].as_bool().unwrap_or(false)),
                state["effective_device"].as_str().unwrap_or("-"),
                detail
            );
        }
    }
    Ok(())
}

fn cmd_tasks(base: &str, id: Option<&str>, limit: usize) -> Result<(), String> {
    if let Some(id) = id {
        let (status, body) = get_json(base, &format!("/api/tasks/{id}"))?;
        expect_success(status, &body)?;
        return print_pretty_json(&body);
    }
    if !(1..=100).contains(&limit) {
        return Err("task limit must be between 1 and 100".to_string());
    }
    let (status, body) = get_json(base, &format!("/api/tasks?completed_limit={limit}"))?;
    if status != 200 {
        return Err(json_response_error(status, &body));
    }
    let running = body["running"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let completed = body["completed"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if running.is_empty() && completed.is_empty() {
        println!("No tasks in the current daemon session");
        return Ok(());
    }
    print_task_group("RUNNING", running);
    print_task_group("RECENT", completed);
    Ok(())
}

fn print_task_group(title: &str, tasks: &[Value]) {
    if tasks.is_empty() {
        return;
    }
    println!("{title}");
    println!(
        "{:<24} {:<5} {:<10} {:<24} {:>9} INPUT",
        "ID", "KIND", "STATUS", "MODEL", "DURATION"
    );
    for task in tasks {
        let duration = task["duration_ms"]
            .as_u64()
            .map(format_duration_ms)
            .unwrap_or_else(|| "-".to_string());
        let preview = task["input_preview"]
            .as_str()
            .unwrap_or("")
            .replace(['\r', '\n'], " ");
        println!(
            "{:<24} {:<5} {:<10} {:<24} {:>9} {}",
            task["id"].as_str().unwrap_or("?"),
            task["kind"].as_str().unwrap_or("?"),
            task["status"].as_str().unwrap_or("?"),
            task["model"].as_str().unwrap_or("?"),
            duration,
            shorten(&preview, 48)
        );
    }
}

fn cmd_logging(base: &str, level: Option<LogLevel>) -> Result<(), String> {
    let (status, body) = match level {
        Some(level) => post_json(base, "/api/logging", &json!({"level": level.as_str()}))?,
        None => get_json(base, "/api/logging")?,
    };
    expect_success(status, &body)?;
    println!(
        "Daemon log level: {}",
        body["level"].as_str().unwrap_or("?")
    );
    Ok(())
}

fn cmd_voice(base: &str, model: &str, voice: Option<&str>, clear: bool) -> Result<(), String> {
    if clear || voice.is_some() {
        let voice = if clear { "" } else { voice.unwrap_or_default() };
        let (status, body) = post_json(
            base,
            &format!("/api/models/{model}/voice"),
            &json!({"voice": voice}),
        )?;
        expect_success(status, &body)?;
        if clear {
            println!("Cleared the default voice for {model}");
        } else {
            println!("Set {model} default voice to {voice}");
        }
        return Ok(());
    }

    let (status, body) = get_json(base, &format!("/api/models/{model}/voices"))?;
    expect_success(status, &body)?;
    let default_voice = body["default_voice"].as_str();
    let voices = body["voices"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if voices.is_empty() {
        println!("No voices found for {model}");
        return Ok(());
    }
    println!(
        "VOICE{}",
        if default_voice.is_some() {
            " (default marked *)"
        } else {
            ""
        }
    );
    for voice in voices.iter().filter_map(Value::as_str) {
        let marker = if Some(voice) == default_voice {
            "*"
        } else {
            " "
        };
        println!("{marker} {voice}");
    }
    Ok(())
}

/// 检查 daemon；未运行则提示如何启动（文档 §18 Server）。
fn cmd_serve(base: &str) -> Result<(), String> {
    match get_json(base, "/health") {
        Ok((200, _)) => {
            println!("Server already running");
            println!("{}", base);
            Ok(())
        }
        _ => {
            println!("Server not running");
            println!("Start it with: aiworkd");
            Ok(())
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn shorten(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        let mut shortened: String = prefix.chars().take(max_chars.saturating_sub(1)).collect();
        shortened.push('…');
        shortened
    } else {
        prefix
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn format_duration_secs(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn format_duration_ms(milliseconds: u64) -> String {
    if milliseconds < 1_000 {
        format!("{milliseconds}ms")
    } else {
        format!("{:.1}s", milliseconds as f64 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn top_level_help_exposes_management_and_observability_commands() {
        let help = Cli::command().render_long_help().to_string();
        for command in [
            "start",
            "remove",
            "keep-alive",
            "providers",
            "tasks",
            "logging",
            "voice",
        ] {
            assert!(help.contains(command), "help omitted {command}");
        }
        assert!(help.contains("macai <命令> --help"));
    }

    #[test]
    fn help_flag_exits_successfully_without_contacting_daemon() {
        let error = match Cli::try_parse_from(["macai", "--help"]) {
            Ok(_) => panic!("--help should stop after rendering help"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(error.to_string().contains("常用示例"));
    }

    #[test]
    fn chat_accepts_generation_options() {
        let cli = Cli::try_parse_from([
            "macai",
            "chat",
            "local-model",
            "hello",
            "--system",
            "be concise",
            "--temperature",
            "0.2",
            "--max-tokens",
            "64",
        ])
        .expect("chat options should parse");

        match cli.command {
            Commands::Chat {
                system,
                temperature,
                max_tokens,
                ..
            } => {
                assert_eq!(system.as_deref(), Some("be concise"));
                assert_eq!(temperature, Some(0.2));
                assert_eq!(max_tokens, Some(64));
            }
            _ => panic!("expected chat command"),
        }
    }

    #[test]
    fn remove_uses_delete_and_preserves_files_contract() {
        let (base, request) = one_shot_json_server(r#"{"id":"demo","deleted":true}"#);
        cmd_remove(&base, "demo").expect("remove request should succeed");

        let request = request.join().expect("mock server should finish");
        assert!(request.starts_with("DELETE /api/models/demo HTTP/1.1\r\n"));
    }

    #[test]
    fn rename_sends_the_new_model_id() {
        let (base, request) = one_shot_json_server(r#"{"id":"renamed","renamed_from":"demo"}"#);
        cmd_rename(&base, "demo", "renamed").expect("rename request should succeed");

        let request = request.join().expect("mock server should finish");
        assert!(request.starts_with("POST /api/models/demo/rename HTTP/1.1\r\n"));
        let body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .expect("request should have a body");
        let body: Value = serde_json::from_str(body).expect("request body should be JSON");
        assert_eq!(body["new_id"], "renamed");
    }

    fn one_shot_json_server(body: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let address = listener.local_addr().expect("read mock address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept CLI request");
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut request = String::new();
            let mut content_length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("read request header");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().expect("valid content length");
                }
                request.push_str(&line);
            }
            request.push_str("\r\n");
            let mut request_body = vec![0; content_length];
            reader
                .read_exact(&mut request_body)
                .expect("read request body");
            request.push_str(&String::from_utf8(request_body).expect("UTF-8 request body"));

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write mock response");
            request
        });
        (format!("http://{address}"), handle)
    }
}
