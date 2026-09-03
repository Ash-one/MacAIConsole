use serde_json::{json, Value};
use tokio::io::{stdin, stdout};

use ai_daemon::runners::{read_frame, write_frame, Envelope, DEFAULT_MAX_FRAME_BYTES};

#[tokio::main]
async fn main() {
    let mut input = stdin();
    let mut output = stdout();
    write_frame(
        &mut output,
        &Envelope::new(
            "hello",
            "startup",
            json!({
                "runner_id": "org.example.fake",
                "runner_version": "0.1.0",
                "protocol_versions": ["macai.runner.v1"],
                "capabilities": ["chat.v1", "tts.v1"],
                "pid": std::process::id(),
            }),
        ),
    )
    .await
    .expect("write hello");

    while let Ok(command) = read_frame(&mut input, DEFAULT_MAX_FRAME_BYTES).await {
        match command.message_type.as_str() {
            "infer"
                if command.payload.get("capability").and_then(Value::as_str) == Some("chat.v1") =>
            {
                write_frame(
                    &mut output,
                    &Envelope::new("accepted", command.id.clone(), json!({})),
                )
                .await
                .expect("write accepted");
                let request = command.payload.get("request").cloned().unwrap_or(json!({}));
                let messages = request
                    .get("messages")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if messages.is_empty() {
                    write_frame(
                        &mut output,
                        &Envelope::new(
                            "error",
                            command.id.clone(),
                            json!({
                                "code": "invalid_request",
                                "message": "messages must be a non-empty list",
                                "retryable": false,
                            }),
                        ),
                    )
                    .await
                    .expect("write error");
                    continue;
                }
                // 逐段回显最后一条 user 消息，覆盖 daemon 的 delta→chunk 映射。
                let content = messages
                    .last()
                    .and_then(|message| message.get("content"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                for segment in split_chunks(&content) {
                    write_frame(
                        &mut output,
                        &Envelope::new("delta", command.id.clone(), json!({ "text": segment })),
                    )
                    .await
                    .expect("write delta");
                }
                let prompt_tokens: u64 = messages
                    .iter()
                    .filter_map(|message| {
                        message
                            .get("content")
                            .and_then(Value::as_str)
                            .map(|content| content.split_whitespace().count() as u64)
                    })
                    .sum();
                let completion_tokens = content.split_whitespace().count() as u64;
                write_frame(
                    &mut output,
                    &Envelope::new(
                        "result",
                        command.id.clone(),
                        json!({
                            "text": content,
                            "finish_reason": "stop",
                            "usage": {
                                "prompt_tokens": prompt_tokens,
                                "completion_tokens": completion_tokens,
                                "total_tokens": prompt_tokens + completion_tokens,
                            },
                            "effective_device": "cpu",
                        }),
                    ),
                )
                .await
                .expect("write result");
            }
            "infer" => {
                if command
                    .payload
                    .get("request")
                    .and_then(|request| request.get("text"))
                    .and_then(serde_json::Value::as_str)
                    == Some("__crash__")
                {
                    std::process::exit(86);
                }
                write_frame(
                    &mut output,
                    &Envelope::new("accepted", command.id.clone(), json!({})),
                )
                .await
                .expect("write accepted");
                if command
                    .payload
                    .get("request")
                    .and_then(|request| request.get("text"))
                    .and_then(serde_json::Value::as_str)
                    == Some("__slow__")
                {
                    tokio::time::sleep(std::time::Duration::from_millis(750)).await;
                }
                // tts.v1 结果形状：向 daemon 授权目录写一个 wav 文件并回报
                // 路径与字节数，覆盖 daemon 的路径校验与读取路径。
                let directory = command
                    .payload
                    .get("output")
                    .and_then(|output| output.get("directory"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let path = std::path::Path::new(&directory).join("output.wav");
                let wav = minimal_wav(&[0_i16; 64]);
                let bytes = wav.len() as u64;
                if let Err(error) = std::fs::write(&path, wav) {
                    write_frame(
                        &mut output,
                        &Envelope::new(
                            "error",
                            command.id.clone(),
                            json!({
                                "code": "internal",
                                "message": format!("cannot write output: {error}"),
                                "retryable": false,
                            }),
                        ),
                    )
                    .await
                    .expect("write error");
                    continue;
                }
                write_frame(
                    &mut output,
                    &Envelope::new(
                        "result",
                        command.id.clone(),
                        json!({
                            "content_type": "audio/wav",
                            "path": path.display().to_string(),
                            "bytes": bytes,
                            "duration_ms": 64,
                            "sample_rate": 24000,
                        }),
                    ),
                )
                .await
                .expect("write result");
            }
            other => {
                let reply_type = match other {
                    "initialize" => "initialized",
                    "load" => "loaded",
                    "unload" => "unloaded",
                    "shutdown" => "shutdown_complete",
                    "health" => "healthy",
                    _ => "error",
                };
                write_frame(
                    &mut output,
                    &Envelope::new(
                        reply_type,
                        command.id.clone(),
                        json!({
                            "ok": reply_type != "error",
                            "model_id": command.payload.get("model_id").cloned().unwrap_or(json!(null)),
                            "effective_device": "cpu",
                            "capabilities": ["chat.v1", "tts.v1"],
                            "limits": {"max_concurrency": 1},
                        }),
                    ),
                )
                .await
                .expect("write reply");
                if command.message_type == "shutdown" {
                    break;
                }
            }
        }
    }
}

/// 把文本切成多段 delta（每个词一段），覆盖 daemon 的 delta→chunk 映射。
fn split_chunks(content: &str) -> Vec<String> {
    let mut segments: Vec<String> = content
        .split_inclusive(char::is_whitespace)
        .map(str::to_string)
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.is_empty() && !content.is_empty() {
        segments.push(content.to_string());
    }
    segments
}

/// 最小合法 RIFF/WAVE（16-bit PCM 单声道），用于覆盖 daemon 的音频读取路径。
fn minimal_wav(samples: &[i16]) -> Vec<u8> {
    let sample_rate = 24_000_u32;
    let data_bytes = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1_u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2_u16.to_le_bytes()); // block align
    out.extend_from_slice(&16_u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}
