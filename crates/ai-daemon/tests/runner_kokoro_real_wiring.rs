//! Phase 2 收尾：真实 Kokoro Runner 经 daemon 侧 RunnerProvider 的组合验证。
//!
//! fake Runner composition（`runner_runtime_composition.rs`）覆盖协议与生命周期；
//! 本测试用真实 `runners/kokoro` 包、真实 uv 受管环境和真实 Kokoro-82M-zh 模型，
//! 走 RunnerRegistry discovery → ensure_environment → spawn → load → synthesize
//! → unload → shutdown 的完整 daemon 路径。
//!
//! 需要真实模型，仅在显式给出模型目录时运行（CI 与常规 `cargo test` 自动跳过）：
//!
//! ```bash
//! MACAI_KOKORO_WIRING_MODEL=/absolute/path/to/kokoro-82m-zh \
//!   cargo test -p ai-daemon --test runner_kokoro_real_wiring
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ai_core::model::ModelSpec;
use ai_core::provider::{Provider, TTSProvider};
use ai_core::request::SpeechRequest;
use ai_daemon::runners::{
    EnvironmentManager, EnvironmentManagerConfig, ModelProfile, RunnerInstanceManager,
    RunnerProvider, RunnerRegistry, RunnerState,
};

const RUNNER_ID: &str = "org.macai.kokoro";
const ENVIRONMENT_ID: &str = "org.macai.kokoro-python";
const MODEL_ID: &str = "kokoro-82m-zh";

/// 复制 Runner 包到隔离目录（daemon discovery 对包内容做全量 digest 且拒绝
/// symlink，因此必须排除 `.venv`、`.pytest_cache`、`__pycache__` 等本地产物）。
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

fn temp_root(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "macai-kokoro-wiring-{label}-{}-{unique}",
        std::process::id()
    ))
}

#[tokio::test]
async fn kokoro_runner_real_composition_load_infer_unload() {
    let Ok(model_root) = std::env::var("MACAI_KOKORO_WIRING_MODEL") else {
        eprintln!(
            "skipped: set MACAI_KOKORO_WIRING_MODEL to the kokoro-82m-zh model directory \
             and `uv sync --project runners/kokoro --locked --no-dev` first"
        );
        return;
    };
    let model_root = PathBuf::from(model_root);
    assert!(
        model_root.join("model.safetensors").is_file(),
        "model root must contain model.safetensors: {}",
        model_root.display()
    );

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let root = temp_root("pkg");
    copy_package(&repo_root.join("runners/kokoro"), &root.join("kokoro")).unwrap();

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
        .unwrap_or_else(|| {
            let found: Vec<_> = registry
                .entries()
                .iter()
                .map(|entry| {
                    format!(
                        "{:?} manifest={:?} reason={:?}",
                        entry.root,
                        entry
                            .manifest
                            .as_ref()
                            .map(|m| (m.id.clone(), m.version.clone())),
                        entry.reason
                    )
                })
                .collect();
            panic!(
                "kokoro runner must be discovered in {} (entries: {found:#?})",
                root.display()
            )
        });
    assert_eq!(
        descriptor.state,
        RunnerState::Trusted,
        "builtin root must trust the kokoro runner: {:?}",
        descriptor.reason
    );

    let temp = root.join("temp");
    std::fs::create_dir_all(&temp).unwrap();
    let environments = EnvironmentManager::new(EnvironmentManagerConfig {
        runtime_root: root.join("Runtimes/python"),
    });
    let instances = Arc::new(RunnerInstanceManager::new(
        registry,
        environments,
        temp.clone(),
    ));
    let provider = RunnerProvider::new(
        RUNNER_ID.to_string(),
        &["tts.v1".to_string()],
        instances.clone(),
        temp.clone(),
    );

    let profile =
        ModelProfile::load(&root.join("kokoro/profiles/kokoro-82m-zh.toml")).expect("profile");
    provider
        .bind_model(ai_daemon::runners::RunnerModelBinding {
            model_id: MODEL_ID.to_string(),
            profile,
            environment_id: ENVIRONMENT_ID.to_string(),
            artifact_root: model_root.clone(),
        })
        .await;

    let spec = ModelSpec {
        id: MODEL_ID.to_string(),
        name: "Kokoro 82M zh".to_string(),
        model_type: "tts".to_string(),
        provider: RUNNER_ID.to_string(),
        source: None,
        path: Some(model_root.display().to_string()),
        format: Some("directory".to_string()),
        size_bytes: None,
        memory_estimate: Some(400_000_000),
        keep_alive: Some("always".to_string()),
        context_length: None,
        default_voice: Some("zf_001".to_string()),
    };

    // load：ensure_environment（真实 uv sync 124 packages）→ spawn → initialize →
    // 真实模型 load（MLX/Metal）。
    let handle = provider.load(&spec).await.expect("kokoro runner load");
    assert_eq!(handle.provider_id, RUNNER_ID);
    assert_eq!(handle.model_id, MODEL_ID);

    let status = provider.status().await;
    assert!(status.ready, "ready after load: {:?}", status.reason);
    assert!(status.resident_models.contains(&MODEL_ID.to_string()));

    // infer：短中文 + 混合中英，经 RunnerProvider 桥接的真实 tts.v1 路径。
    for (label, text) in [
        ("zh", "你好，这是 daemon 侧 RunnerProvider 的中文验证。"),
        ("mixed", "你好 hello，mixed text 接线验证。"),
    ] {
        let response = provider
            .synthesize(SpeechRequest {
                model: MODEL_ID.to_string(),
                input: text.to_string(),
                voice: Some("zf_001".to_string()),
                format: Some("wav".to_string()),
                speed: Some(1.0),
            })
            .await
            .unwrap_or_else(|error| panic!("{label} synthesize failed: {error}"));
        assert_eq!(response.content_type, "audio/wav");
        assert!(
            response.audio.len() > 44_100,
            "{label}: wav payload too small: {} bytes",
            response.audio.len()
        );
        // RIFF/WAVE 头校验。
        assert_eq!(&response.audio[0..4], b"RIFF");
        assert_eq!(&response.audio[8..12], b"WAVE");
        println!("[wiring] {label}: {} bytes", response.audio.len());
    }

    // per-request 输出目录读取后被清理。
    let leftovers: Vec<_> = std::fs::read_dir(&temp)
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("tts-request-")
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "request output dirs must be cleaned: {leftovers:?}"
    );

    provider.unload(&handle).await.expect("unload");
    instances
        .shutdown_instance(RUNNER_ID)
        .await
        .expect("shutdown");
    let _ = std::fs::remove_dir_all(&root);
}
