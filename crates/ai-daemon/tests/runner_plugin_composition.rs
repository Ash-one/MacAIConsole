use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ai_daemon::runners::{RunnerProcess, RunnerRegistry, RunnerState};
use serde_json::json;

const MANIFEST: &str = r#"
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
boot_seconds = 5
load_seconds = 5
inference_seconds = 5
shutdown_seconds = 5
[security]
network_during_install = false
network_during_runtime = false
[[models]]
profile = "profiles/fake.toml"
adapter = "fake"
"#;

const PROFILE: &str = r#"
schema = "macai.model.v1"
id = "fake-tts"
name = "Fake TTS"
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
files = ["model.safetensors"]
[compatibility]
runner = ">=0.1"
"#;

#[tokio::test]
async fn trusted_fake_runner_completes_discover_handshake_load_infer_and_unload() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "macai-runner-composition-{}-{unique}",
        std::process::id()
    ));
    let package = root.join("fake");
    std::fs::create_dir_all(package.join("profiles")).unwrap();
    std::fs::write(package.join("runner.toml"), MANIFEST).unwrap();
    std::fs::write(package.join("pyproject.toml"), "[project]\nname='fake'\n").unwrap();
    std::fs::write(package.join("uv.lock"), "version = 1\n").unwrap();
    std::fs::write(package.join("profiles/fake.toml"), PROFILE).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_fake-runner"),
        package.join("fake-runner"),
    )
    .unwrap();

    let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
    let runner = registry
        .trusted("org.example.fake")
        .expect("built-in Runner is trusted");
    assert_eq!(runner.state, RunnerState::Trusted);
    assert_eq!(
        registry
            .resolve_bundled_profile("fake-tts")
            .unwrap()
            .profile
            .adapter,
        "fake"
    );

    let manifest = runner.manifest.as_ref().unwrap();
    let mut process = RunnerProcess::spawn(manifest, &package, &package.join("fake-runner"), &root)
        .await
        .expect("fake Runner handshake");
    process.initialize(&manifest.id, &root).await.unwrap();
    process
        .request(
            "load",
            "load-1",
            json!({"model_id": "fake-tts"}),
            "loaded",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    process
        .request(
            "infer",
            "infer-1",
            json!({"capability": "tts.v1"}),
            "accepted",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let result = process.receive(Duration::from_secs(5)).await.unwrap();
    assert_eq!(result.message_type, "result");
    process
        .request(
            "unload",
            "unload-1",
            json!({}),
            "unloaded",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(process.shutdown().await.unwrap(), "");
    let _ = std::fs::remove_dir_all(root);
}
