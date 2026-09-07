//! Whisper.cpp Runner 的真实 daemon 组合验证。
//!
//! 常规测试自动跳过；目标机验证显式提供官方 `whisper-server`、GGML `.bin`
//! 模型和一段含语音的 PCM WAV：
//!
//! ```bash
//! MACAI_WHISPER_SERVER=/absolute/path/to/whisper-server \
//! MACAI_WHISPER_WIRING_MODEL=/absolute/path/to/ggml-base.bin \
//! MACAI_WHISPER_WIRING_AUDIO=/absolute/path/to/speech.wav \
//!   cargo test -p ai-daemon --test runner_whisper_real_wiring -- --ignored --nocapture
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ai_core::model::ModelSpec;
use ai_core::provider::{Provider, STTProvider};
use ai_core::request::TranscriptionRequest;
use ai_daemon::runners::{
    EnvironmentManager, EnvironmentManagerConfig, RunnerInstanceManager, RunnerProvider,
    RunnerRegistry, RunnerState,
};

const RUNNER_ID: &str = "org.macai.whisper.cpp";
const MODEL_ID: &str = "whisper-real-wiring";

fn copy_package(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), ".venv" | ".pytest_cache" | "__pycache__") {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_package(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn temp_root() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "macai-whisper-wiring-{}-{unique}",
        std::process::id()
    ))
}

#[tokio::test]
#[ignore = "requires a local whisper-server, model, and PCM WAV fixture"]
async fn whisper_runner_real_composition_load_infer_unload() {
    let server = std::env::var("MACAI_WHISPER_SERVER")
        .expect("set MACAI_WHISPER_SERVER for the real Whisper Runner wiring test");
    let model = std::env::var("MACAI_WHISPER_WIRING_MODEL")
        .expect("set MACAI_WHISPER_WIRING_MODEL for the real Whisper Runner wiring test");
    let audio = std::env::var("MACAI_WHISPER_WIRING_AUDIO")
        .expect("set MACAI_WHISPER_WIRING_AUDIO for the real Whisper Runner wiring test");
    let server = PathBuf::from(server);
    let model = PathBuf::from(model);
    let audio = PathBuf::from(audio);
    assert!(server.is_file(), "server is missing: {}", server.display());
    assert_eq!(
        model.extension().and_then(|value| value.to_str()),
        Some("bin")
    );
    assert!(model.is_file(), "model is missing: {}", model.display());
    assert!(audio.is_file(), "audio is missing: {}", audio.display());

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let root = temp_root();
    copy_package(
        &repo_root.join("runners/whisper.cpp"),
        &root.join("whisper.cpp"),
    )
    .unwrap();

    let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
    let descriptor = registry
        .entries()
        .iter()
        .find(|entry| {
            entry
                .manifest
                .as_ref()
                .is_some_and(|manifest| manifest.id == RUNNER_ID)
        })
        .expect("whisper Runner must be discovered");
    assert_eq!(
        descriptor.state,
        RunnerState::Trusted,
        "{:?}",
        descriptor.reason
    );

    let temp = root.join("temp");
    std::fs::create_dir_all(&temp).unwrap();
    let environments = EnvironmentManager::new(EnvironmentManagerConfig {
        runtime_root: root.join("Runtimes/python"),
        uv_path: None,
    });
    let instances = Arc::new(RunnerInstanceManager::new(
        registry,
        environments,
        temp.clone(),
    ));
    let provider = RunnerProvider::new(
        RUNNER_ID.to_string(),
        &["stt.v1".to_string()],
        instances.clone(),
        temp,
    );
    provider
        .bind_adhoc_model(MODEL_ID, &model, "whisper-cpp-server", "bin")
        .await
        .expect("bind ad-hoc Whisper model");

    let spec = ModelSpec {
        id: MODEL_ID.to_string(),
        name: "Whisper real wiring".to_string(),
        model_type: "stt".to_string(),
        provider: RUNNER_ID.to_string(),
        requested_provider: Some(RUNNER_ID.to_string()),
        provider_selection_reason: Some("real wiring test".to_string()),
        source: None,
        path: Some(model.display().to_string()),
        format: Some("bin".to_string()),
        size_bytes: Some(model.metadata().unwrap().len()),
        memory_estimate: Some(model.metadata().unwrap().len()),
        keep_alive: Some("always".to_string()),
        context_length: None,
        default_voice: None,
    };

    let handle = provider.load(&spec).await.expect("whisper Runner load");
    let response = provider
        .transcribe(TranscriptionRequest {
            model: MODEL_ID.to_string(),
            file: Some(audio.display().to_string()),
            language: None,
            response_format: Some("json".to_string()),
        })
        .await
        .expect("whisper Runner transcription");
    assert!(
        !response.text.trim().is_empty(),
        "transcript must not be empty"
    );
    println!(
        "[whisper-wiring] language={:?} text={}",
        response.language, response.text
    );

    let status = provider.status().await;
    assert!(status.ready, "ready after load: {:?}", status.reason);
    assert!(status.effective_device.is_some(), "device must be reported");

    provider.unload(&handle).await.expect("unload");
    instances
        .shutdown_instance(RUNNER_ID)
        .await
        .expect("shutdown");
    let _ = std::fs::remove_dir_all(root);
}
