use serde_json::json;
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
                "capabilities": ["tts.v1"],
                "pid": std::process::id(),
            }),
        ),
    )
    .await
    .expect("write hello");

    while let Ok(command) = read_frame(&mut input, DEFAULT_MAX_FRAME_BYTES).await {
        let reply_type = match command.message_type.as_str() {
            "initialize" => "initialized",
            "load" => "loaded",
            "infer" => {
                write_frame(
                    &mut output,
                    &Envelope::new("accepted", command.id.clone(), json!({})),
                )
                .await
                .expect("write accepted");
                "result"
            }
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
                json!({"ok": reply_type != "error"}),
            ),
        )
        .await
        .expect("write reply");
        if command.message_type == "shutdown" {
            break;
        }
    }
}
