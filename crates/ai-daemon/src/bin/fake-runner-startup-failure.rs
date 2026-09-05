use std::path::PathBuf;

use ai_daemon::runners::{write_frame, Envelope};
use serde_json::json;
use tokio::io::{stdout, AsyncWriteExt};

#[tokio::main]
async fn main() {
    let runtime_root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .expect("runtime temp root argument");
    let mode = std::env::args().nth(2).expect("failure mode argument");
    if mode == "descendant" {
        std::fs::write(
            runtime_root.join("startup-failure-descendant.pid"),
            std::process::id().to_string(),
        )
        .expect("write descendant pid");
        tokio::time::sleep(std::time::Duration::from_secs(3_600)).await;
        return;
    }
    std::fs::write(
        runtime_root.join(format!("startup-failure-{mode}.pid")),
        std::process::id().to_string(),
    )
    .expect("write test pid");

    std::process::Command::new(std::env::current_exe().expect("current executable"))
        .arg(&runtime_root)
        .arg("descendant")
        .spawn()
        .expect("spawn test descendant");

    let mut output = stdout();
    match mode.as_str() {
        "identity" => write_frame(
            &mut output,
            &Envelope::new(
                "hello",
                "startup",
                json!({
                    "runner_id": "org.example.wrong",
                    "runner_version": "0.1.0",
                    "protocol_versions": ["macai.runner.v1"],
                    "capabilities": ["tts.v1"],
                }),
            ),
        )
        .await
        .expect("write wrong identity hello"),
        "protocol" => {
            output
                .write_all(&[0, 0, 0, 8, b'n', b'o', b't', b'-', b'j', b's', b'o', b'n'])
                .await
                .expect("write malformed frame");
            output.flush().await.expect("flush malformed frame");
        }
        "timeout" => {}
        "success" | "crash" => write_frame(
            &mut output,
            &Envelope::new(
                "hello",
                "startup",
                json!({
                    "runner_id": "org.example.fake",
                    "runner_version": "0.1.0",
                    "protocol_versions": ["macai.runner.v1"],
                    "capabilities": ["tts.v1"],
                }),
            ),
        )
        .await
        .expect("write valid hello"),
        other => panic!("unknown startup failure mode {other}"),
    }
    if mode == "crash" {
        return;
    }
    tokio::time::sleep(std::time::Duration::from_secs(3_600)).await;
}
