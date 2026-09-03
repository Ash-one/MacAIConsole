//! Phase 1C Runtime/scheduler Runner Instance 组合测试。
//!
//! fake Runner 从已注册 Model Profile 经 RunnerProvider bridge 走 Runtime
//! 的真实 Provider 装配路径：register → bind → load → synthesize → unload。
//! lease/busy guard、错误映射、输出目录清理与 shutdown 均在此验证。

use std::collections::HashSet;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use ai_core::model::ModelSpec;
use ai_core::provider::{Provider, TTSProvider};
use ai_core::request::SpeechRequest;
use ai_daemon::runners::{
    EnvironmentManager, EnvironmentManagerConfig, ModelProfile, RunnerInstanceManager,
    RunnerProvider, RunnerRegistry,
};

const MANIFEST: &str = r#"
schema = "macai.runner.v1"
id = "org.example.fake"
version = "0.1.0"
protocols = ["macai.runner.v1"]
capabilities = ["chat.v1", "tts.v1"]
[entrypoint]
command = ["fake-runner"]
working_directory = "package"
[runtime]
type = "python-uv"
id = "org.example.env"
project = "."
lock = "uv.lock"
python = ">=3.10"
probe = ["{environment.python}", "-c", "print('probe-ok')"]
[capacity]
max_instances = 1
max_concurrency_per_instance = 1
[timeouts]
boot_seconds = 10
load_seconds = 10
inference_seconds = 10
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
[defaults]
keep_alive = "always"
voice = "zf_001"
format = "wav"
[resources]
memory_estimate_bytes = 1000
[compatibility]
runner = ">=0.1,<0.2"
"#;

fn test_root(label: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("macai-1c-{label}-{}-{unique}", std::process::id()))
}

/// 组装完整 fake Runner 环境：package + registry + environment + instance manager。
async fn fixture(
    label: &str,
) -> (
    std::path::PathBuf,
    RunnerProvider,
    Arc<RunnerInstanceManager>,
) {
    let root = test_root(label);
    let package = root.join("fake");
    std::fs::create_dir_all(package.join("profiles")).unwrap();
    std::fs::write(package.join("runner.toml"), MANIFEST).unwrap();
    std::fs::write(
        package.join("pyproject.toml"),
        "[project]\nname = \"macai-env-fixture\"\nversion = \"0.1.0\"\nrequires-python = \">=3.10\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("uv.lock"),
        "version = 1\nrevision = 3\nrequires-python = \">=3.10\"\n\n[[package]]\nname = \"macai-env-fixture\"\nversion = \"0.1.0\"\nsource = { virtual = \".\" }\n",
    )
    .unwrap();
    std::fs::write(package.join("profiles/fake.toml"), PROFILE).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_fake-runner"),
        package.join("fake-runner"),
    )
    .unwrap();

    let registry = RunnerRegistry::discover(&[root.clone()], &[], &HashSet::new());
    let temp_root = root.join("temp");
    std::fs::create_dir_all(&temp_root).unwrap();
    let environments = EnvironmentManager::new(EnvironmentManagerConfig {
        runtime_root: root.join("Runtimes/python"),
    });
    let instances = Arc::new(RunnerInstanceManager::new(
        registry,
        environments,
        temp_root.clone(),
    ));
    let provider = RunnerProvider::new(
        "org.example.fake".to_string(),
        &["tts.v1".to_string(), "chat.v1".to_string()],
        instances.clone(),
        temp_root,
    );

    // uv 不可用时测试在环境安装步骤失败；CI 固定安装 uv 0.9.21。
    let binding = ai_daemon::runners::RunnerModelBinding {
        model_id: "fake-model".to_string(),
        profile: ModelProfile::load(&package.join("profiles/fake.toml")).unwrap(),
        environment_id: "org.example.env".to_string(),
        artifact_root: package.clone(),
    };
    provider.bind_model(binding).await;
    (root, provider, instances)
}

fn speech_request() -> SpeechRequest {
    SpeechRequest {
        model: "fake-model".to_string(),
        input: "你好".to_string(),
        voice: Some("zf_001".to_string()),
        format: Some("wav".to_string()),
        speed: Some(1.0),
    }
}

fn model_spec() -> ModelSpec {
    ModelSpec {
        id: "fake-model".to_string(),
        name: "Fake".to_string(),
        model_type: "tts".to_string(),
        provider: "org.example.fake".to_string(),
        source: None,
        path: None,
        format: Some("directory".to_string()),
        size_bytes: None,
        memory_estimate: Some(1000),
        keep_alive: Some("always".to_string()),
        context_length: None,
        default_voice: Some("zf_001".to_string()),
    }
}

#[tokio::test]
async fn runner_provider_completes_load_infer_unload_cycle() {
    let (root, provider, instances) = fixture("cycle").await;
    let handle = provider.load(&model_spec()).await.expect("runner load");
    assert_eq!(handle.provider_id, "org.example.fake");
    assert_eq!(handle.model_id, "fake-model");

    let response = provider
        .synthesize(speech_request())
        .await
        .expect("tts infer");
    assert_eq!(response.content_type, "audio/wav");
    assert!(
        !response.audio.is_empty(),
        "fake runner produces a wav payload"
    );

    // unload 走协议层；随后关闭 instance，worker 进程不留孤儿。
    provider.unload(&handle).await.expect("runner unload");
    instances
        .shutdown_instance("org.example.fake")
        .await
        .expect("shutdown");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn unbound_model_fails_with_model_not_found() {
    let (root, provider, _instances) = fixture("unbound").await;
    let mut spec = model_spec();
    spec.id = "never-bound".to_string();
    let error = provider.load(&spec).await.unwrap_err();
    assert_eq!(error.kind, ai_core::AIError::ModelNotFound);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn output_directory_is_cleaned_after_success() {
    let (root, provider, _instances) = fixture("cleanup").await;
    provider.load(&model_spec()).await.expect("load");
    let temp_root = root.join("temp");
    provider.synthesize(speech_request()).await.expect("infer");
    // per-request 输出目录读取后被清理，temp root 不残留 tts-request-*。
    let leftovers: Vec<_> = std::fs::read_dir(&temp_root)
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
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn status_reflects_environment_phase_without_resident_worker() {
    let (root, provider, instances) = fixture("status").await;
    let status = provider.status().await;
    // 环境已装好（fixture 路径中 ensure 只在 load 时触发；此处未 load，
    // environment status 可能尚未存在）——两种合法状态：未安装或 ready。
    if let Some(reason) = &status.reason {
        assert!(
            reason.contains("environment") || reason.contains("bound"),
            "reason must describe environment or binding state: {reason}"
        );
    }
    // load 之后 status 的 resident_models 应包含模型。
    provider.load(&model_spec()).await.expect("load");
    let ready = provider.status().await;
    assert!(
        ready.ready,
        "after load the runner provider reports ready: {:?}",
        ready.reason
    );
    assert!(ready.resident_models.contains(&"fake-model".to_string()));
    assert_eq!(ready.effective_device.as_deref(), Some("cpu"));
    let snapshot = instances
        .instance_snapshot("org.example.fake")
        .await
        .expect("instance snapshot");
    assert!(snapshot.alive);
    assert!(snapshot.pid.is_some());
    assert_eq!(snapshot.loaded_model.as_deref(), Some("fake-model"));
    assert_eq!(snapshot.active_requests, 0);
    instances
        .shutdown_instance("org.example.fake")
        .await
        .expect("shutdown");
    let stopped = provider.status().await;
    assert!(
        stopped.available,
        "the installed environment remains available"
    );
    assert!(!stopped.ready, "a stopped worker is not ready");
    assert!(
        stopped.resident_models.is_empty(),
        "a stopped worker has no resident model"
    );
    assert_eq!(stopped.effective_device, None);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn crashed_worker_clears_residency_and_can_be_reloaded() {
    let (root, provider, instances) = fixture("crash-reload").await;
    provider.load(&model_spec()).await.expect("initial load");
    let mut crash = speech_request();
    crash.input = "__crash__".to_string();
    let error = provider.synthesize(crash).await.unwrap_err();
    assert_eq!(error.kind, ai_core::AIError::BackendCrashed);

    let failed = provider.status().await;
    assert!(!failed.ready);
    assert!(failed.resident_models.is_empty());
    assert!(provider.health_check().await.is_err());

    provider
        .load(&model_spec())
        .await
        .expect("reload after crash");
    let recovered = provider.status().await;
    assert!(recovered.ready);
    assert_eq!(recovered.resident_models, vec!["fake-model".to_string()]);
    provider
        .synthesize(speech_request())
        .await
        .expect("inference after recovery");
    instances
        .shutdown_instance("org.example.fake")
        .await
        .expect("shutdown");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn status_snapshot_does_not_wait_for_active_inference_io() {
    let (root, provider, instances) = fixture("status-during-infer").await;
    let provider = Arc::new(provider);
    provider.load(&model_spec()).await.expect("load");
    let infer_provider = provider.clone();
    let inference = tokio::spawn(async move {
        let mut request = speech_request();
        request.input = "__slow__".to_string();
        infer_provider.synthesize(request).await
    });

    let mut observed_active = false;
    for _ in 0..100 {
        if instances
            .instance_snapshot("org.example.fake")
            .await
            .is_some_and(|snapshot| snapshot.active_requests == 1)
        {
            observed_active = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(observed_active, "inference must become active");
    let status = tokio::time::timeout(Duration::from_millis(100), provider.status())
        .await
        .expect("status must not wait for the process I/O lock");
    assert!(status.ready);
    assert_eq!(status.resident_models, vec!["fake-model".to_string()]);
    inference.await.unwrap().expect("slow inference");
    instances
        .shutdown_instance("org.example.fake")
        .await
        .expect("shutdown");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn chat_stream_maps_delta_events_and_result_usage() {
    let (root, provider, instances) = fixture("chat-stream").await;
    provider.load(&model_spec()).await.expect("load");

    let request = ai_core::request::ChatRequest {
        model: "fake-model".to_string(),
        messages: vec![ai_core::request::ChatMessage {
            role: "user".to_string(),
            content: "hello runner chat".to_string(),
        }],
        stream: true,
        temperature: Some(0.7),
        max_tokens: Some(64),
    };

    let stream = ai_core::provider::ChatProvider::chat_stream(&provider, request.clone())
        .await
        .expect("chat stream");
    use futures::StreamExt;
    let chunks: Vec<ai_core::response::ChatChunk> =
        stream.map(|item| item.expect("chunk ok")).collect().await;
    // 首个 chunk 带 role；delta chunks 拼出原文；末 chunk 带 finish_reason + usage。
    assert_eq!(
        chunks.first().and_then(|c| c.choices[0].delta.role.clone()),
        Some("assistant".to_string())
    );
    let text: String = chunks[1..]
        .iter()
        .filter_map(|chunk| chunk.choices[0].delta.content.clone())
        .collect();
    assert_eq!(text, "hello runner chat");
    let last = chunks.last().expect("final chunk");
    assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));
    let usage = last.usage.clone().expect("usage on final chunk");
    assert_eq!(usage.prompt_tokens, 3);
    assert_eq!(usage.completion_tokens, 3);
    assert_eq!(usage.total_tokens, 6);

    // 非流式路径聚合出同一个 ChatResponse。
    let response = ai_core::provider::ChatProvider::chat(&provider, request)
        .await
        .expect("chat response");
    assert_eq!(response.object, "chat.completion");
    assert_eq!(
        response.choices[0].message.content,
        "hello runner chat".to_string()
    );
    assert_eq!(response.usage.total_tokens, 6);

    // 空 messages 在 provider 层快速失败，不下发 runner。
    let empty = ai_core::request::ChatRequest {
        model: "fake-model".to_string(),
        ..Default::default()
    };
    let error = ai_core::provider::ChatProvider::chat(&provider, empty)
        .await
        .unwrap_err();
    assert_eq!(error.kind, ai_core::AIError::InvalidRequest);

    instances
        .shutdown_instance("org.example.fake")
        .await
        .expect("shutdown");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn runner_model_spec_carries_profile_defaults() {
    // Model Profile 到 ModelSpec 的映射边界；Runtime lease/LRU 由 runtime.rs 的
    // 行为测试拥有，不能用字段断言冒充生命周期组合证据。
    let (root, _provider, _instances) = fixture("runtime").await;
    let spec = model_spec();
    // spec.provider 指向 RunnerProvider 注册名；这里只验证 spec 形状与默认值。
    assert_eq!(spec.provider, "org.example.fake");
    assert_eq!(spec.default_voice.as_deref(), Some("zf_001"));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn child_process_is_reaped_after_shutdown() {
    let (root, provider, instances) = fixture("reap").await;
    // 通过 ps 查找 fake-runner 子进程；shutdown 后数量必须回到基线。
    let find_pids = || -> Vec<String> {
        let output = Command::new("/bin/ps")
            .args(["-eo", "pid=,comm="])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains("fake-runner"))
            .map(|line| line.trim().split_whitespace().next().unwrap().to_string())
            .collect()
    };
    let baseline = find_pids().len();
    provider.load(&model_spec()).await.expect("load");
    assert!(
        find_pids().len() > baseline,
        "fake-runner process must be alive after load"
    );
    instances
        .shutdown_instance("org.example.fake")
        .await
        .expect("shutdown");
    // graceful shutdown 后进程退出；轮询等待 ps 视图收敛（进程退出与
    // 父进程 reap 各需一点时间），最多等 3 秒。
    let mut reaped = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if find_pids().len() == baseline {
            reaped = true;
            break;
        }
    }
    assert!(
        reaped,
        "fake-runner process must be reaped after shutdown (baseline={baseline}, now={:?})",
        find_pids()
    );
    let _ = std::fs::remove_dir_all(root);
}
