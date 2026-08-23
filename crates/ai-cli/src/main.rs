//! ai — CLI（文档 §4.3、§18）。
//!
//! CLI 不启动自己的 inference runtime，一切通过 HTTP 调用 aiworkd：
//!   ai status
//!   ai list
//!   ai chat <model> <message>
//!   ai run  <model>          （交互式聊天，文档 §18 Run）
//!   ai ps
//!   ai serve                 （检查 daemon；未运行则提示启动）

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;

use clap::{Parser, Subcommand};
use serde_json::{json, Value};

const DEFAULT_ADDR: &str = "127.0.0.1:11435";

#[derive(Parser)]
#[command(name = "ai", version, about = "macOS Local AI Workbench CLI")]
struct Cli {
    /// daemon 地址（默认 127.0.0.1:11435）。
    #[arg(long, global = true, default_value = DEFAULT_ADDR)]
    addr: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 检查 aiworkd 是否在运行（文档 §48：ai status → "AI Runtime running"）。
    Status,
    /// 列出已注册模型（文档 §18 Models）。
    #[command(name = "list", visible_alias = "models")]
    Models,
    /// 单次聊天（文档 §18 Chat）。
    Chat { model: String, message: String },
    /// 注册并加载本地 GGUF 模型。
    Load {
        path: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value_t = 4096)]
        context_length: u64,
    },
    /// 卸载常驻模型并释放内存。
    Unload { model: String },
    /// 交互式聊天（文档 §18 Run）。
    Run { model: String },
    /// 查看运行中的模型（文档 §18 Runtime）。
    Ps,
    /// 检查 API server；未运行时给出启动提示（文档 §18 Server）。
    Serve,
}

fn main() {
    let cli = Cli::parse();
    let base = format!("http://{}", cli.addr);
    let result = match cli.command {
        Commands::Status => cmd_status(&base),
        Commands::Models => cmd_models(&base),
        Commands::Chat { model, message } => cmd_chat(&base, &model, &message),
        Commands::Load {
            path,
            id,
            context_length,
        } => cmd_load(&base, &path, id.as_deref(), context_length).map(|_| ()),
        Commands::Unload { model } => cmd_unload(&base, &model),
        Commands::Run { model } => cmd_run(&base, &model),
        Commands::Ps => cmd_ps(&base),
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

// ---------- commands ----------

fn cmd_status(base: &str) -> Result<(), String> {
    let (status, body) = get_json(base, "/health")?;
    if status != 200 {
        return Err(format!("daemon unhealthy (HTTP {status})"));
    }
    let version = body["version"].as_str().unwrap_or("?");
    let model_count = body["model_count"].as_u64().unwrap_or(0);
    println!(
        "AI Runtime running (v{}, {} model(s) registered)",
        version, model_count
    );
    println!("{}", base);
    Ok(())
}

fn cmd_models(base: &str) -> Result<(), String> {
    let (status, body) = get_json(base, "/v1/models")?;
    if status != 200 {
        return Err(format!("request failed (HTTP {status}): {}", body));
    }
    println!("{:<12} {:<6} OWNED_BY", "NAME", "TYPE");
    println!("{:<12} {:<6} --------", "----", "----");
    if let Some(data) = body["data"].as_array() {
        for m in data {
            println!(
                "{:<12} {:<6} {}",
                m["id"].as_str().unwrap_or("?"),
                "llm",
                m["owned_by"].as_str().unwrap_or("?")
            );
        }
    }
    Ok(())
}

fn cmd_chat(base: &str, model: &str, message: &str) -> Result<(), String> {
    let model = resolve_model_argument(base, model)?;
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": message}],
        "stream": false,
    });
    let (status, resp) = post_json(base, "/v1/chat/completions", &body)?;
    if status != 200 {
        let msg = resp["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(format!(
            "{}: {msg}",
            resp["error"]["type"].as_str().unwrap_or("error")
        ));
    }
    if let Some(content) = resp["choices"][0]["message"]["content"].as_str() {
        println!("{}", content);
    }
    Ok(())
}

/// 交互式聊天（文档 §18 Run）。
fn cmd_run(base: &str, model: &str) -> Result<(), String> {
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

        let mut messages: Vec<Value> = history
            .iter()
            .flat_map(|(u, a)| {
                vec![
                    json!({"role": "user", "content": u}),
                    json!({"role": "assistant", "content": a}),
                ]
            })
            .collect();
        messages.push(json!({"role": "user", "content": line}));

        let body = json!({"model": model, "messages": messages, "stream": false});
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
        cmd_load(base, model, None, 4096)
    } else {
        Ok(model.to_string())
    }
}

fn cmd_load(
    base: &str,
    path: &str,
    id: Option<&str>,
    context_length: u64,
) -> Result<String, String> {
    let body = json!({
        "path": path,
        "id": id,
        "context_length": context_length,
    });
    let (status, response) = post_json(base, "/api/models/load", &body)?;
    if status != 200 {
        let message = response["error"]["message"]
            .as_str()
            .unwrap_or("unknown error");
        return Err(format!(
            "{}: {message}",
            response["error"]["type"].as_str().unwrap_or("error")
        ));
    }
    let model_id = response["id"]
        .as_str()
        .ok_or_else(|| format!("invalid load response: {response}"))?
        .to_string();
    println!("Loaded {model_id} with llama.cpp");
    Ok(model_id)
}

fn cmd_unload(base: &str, model: &str) -> Result<(), String> {
    let (status, response) = post_json(base, &format!("/api/models/{model}/unload"), &json!({}))?;
    if status != 200 {
        let message = response["error"]["message"]
            .as_str()
            .unwrap_or("unknown error");
        return Err(format!(
            "{}: {message}",
            response["error"]["type"].as_str().unwrap_or("error")
        ));
    }
    println!("Unloaded {model}");
    Ok(())
}

/// 查看运行中的模型（文档 §18 Runtime）。
fn cmd_ps(base: &str) -> Result<(), String> {
    let (status, body) = get_json(base, "/api/runtime")?;
    if status != 200 {
        return Err(format!("request failed (HTTP {status})"));
    }
    println!("MODEL              PROVIDER      STATE");
    println!("-----              --------      -----");
    if let Some(models) = body["loaded_models"].as_array() {
        for m in models {
            println!(
                "{:<18} {:<13} {}",
                m["id"].as_str().unwrap_or("?"),
                m["provider"].as_str().unwrap_or("?"),
                m["state"].as_str().unwrap_or("?")
            );
        }
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
