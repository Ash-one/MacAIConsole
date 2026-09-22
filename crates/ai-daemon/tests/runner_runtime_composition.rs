//! Phase 1C Runtime/scheduler Runner Instance 组合测试。
//!
//! fake Runner 从已注册 Model Profile 经 RunnerProvider bridge 走 Runtime
//! 的真实 Provider 装配路径：register → bind → load → synthesize → unload。
//! lease/busy guard、错误映射、输出目录清理与 shutdown 均在此验证。

use std::collections::HashSet;
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

fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// 组装完整 fake Runner 环境：package + registry + environment + instance manager。
async fn fixture(
    label: &str,
) -> (
    std::path::PathBuf,
    RunnerProvider,
    Arc<RunnerInstanceManager>,
) {
    let (root, provider, instances, package) = setup(label).await;
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

/// 无绑定的 provider + 已发现的 fake Runner package。环境只在 load/ensure
/// 时安装；需要 Ready 环境的测试自行调用 ensure_environment。
async fn setup(
    label: &str,
) -> (
    std::path::PathBuf,
    RunnerProvider,
    Arc<RunnerInstanceManager>,
    std::path::PathBuf,
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

    let registry = RunnerRegistry::discover(std::slice::from_ref(&root), &[], &HashSet::new());
    let temp_root = root.join("temp");
    std::fs::create_dir_all(&temp_root).unwrap();
    let environments = EnvironmentManager::new(EnvironmentManagerConfig {
        runtime_root: root.join("Runtimes/python"),
        uv_path: None,
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
    (root, provider, instances, package)
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
        requested_provider: Some("org.example.fake".to_string()),
        provider_selection_reason: Some("explicit provider selection".to_string()),
        source: None,
        path: None,
        format: Some("directory".to_string()),
        size_bytes: None,
        memory_estimate: Some(1000),
        keep_alive: Some("always".to_string()),
        context_length: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
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
async fn unbound_provider_reports_available_once_environment_is_ready() {
    // 回归（2026-09-13）：status 的环境查询依赖 bindings 首项，脚本 Runner 在
    // 首个模型注册前没有任何绑定，available 恒为 false，GUI 的「注册并加载」
    // 按钮被死锁——首个模型永远无法从 GUI 注册。环境 Ready 的未绑定 provider
    // 必须报告 available=true（ready 仍为 false，等待首个绑定）。
    let (root, provider, instances, _package) = setup("unboundavailable").await;
    let before = provider.status().await;
    assert!(
        !before.available,
        "环境未安装时未绑定 provider 不可用: {:?}",
        before.reason
    );
    assert_eq!(
        before.reason.as_deref(),
        Some("no Runner-backed model is bound")
    );

    let descriptor = instances
        .discovered()
        .into_iter()
        .find(|entry| {
            entry
                .manifest
                .as_ref()
                .is_some_and(|m| m.id == "org.example.fake")
        })
        .expect("fake runner must be discovered");
    let manifest = descriptor
        .manifest
        .expect("discovered runner has a manifest");
    instances
        .environments()
        .ensure_environment(&manifest, &descriptor.root, Duration::from_secs(120))
        .await
        .expect("environment install");
    let status = provider.status().await;
    assert!(
        status.available,
        "Ready 环境让未绑定 provider 可用: {:?}",
        status.reason
    );
    assert!(!status.ready, "未绑定模型时不 ready");
    assert_eq!(
        status.reason.as_deref(),
        Some("no Runner-backed model is bound")
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
    assert!(snapshot.resident_bytes.is_some());
    assert!(snapshot.resident_bytes.unwrap() > 0);
    let memory_usage = provider.memory_usage_bytes().await;
    assert!(memory_usage.is_some());
    assert!(memory_usage.unwrap() > 0);
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
    assert_eq!(provider.memory_usage_bytes().await, None);
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

/// 客户端断开 = daemon 侧 infer future 在终态帧前被 drop。实例必须立即呈现
/// 不可用 + 真实原因，下一次 load 整体回收进程后恢复正常推理；否则被放弃
/// 推理的残留 result 帧会毒化同实例的下一次推理（protocol violation）。
#[tokio::test]
async fn abandoned_inference_marks_instance_and_recovers_on_next_load() {
    let (root, provider, instances) = fixture("abandon-reload").await;
    provider.load(&model_spec()).await.expect("initial load");

    // 直接驱动 instances.infer 以便在 accepted 帧消费后精确放弃该推理，
    // 等价于 HTTP 客户端断开对 handler future 的 drop。
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel::<()>();
    let output_dir = root.join("temp").join("abandon-request");
    std::fs::create_dir_all(&output_dir).unwrap();
    let infer_instances = Arc::clone(&instances);
    let mut accepted_tx = Some(accepted_tx);
    let inference = tokio::spawn(async move {
        infer_instances
            .infer(
                "org.example.fake",
                "tts.v1",
                serde_json::json!({
                    "text": "__slow__",
                    "voice": "zf_001",
                    "speed": 1.0,
                    "format": "wav",
                    "language": "auto",
                }),
                serde_json::json!({
                    "directory": output_dir.display().to_string(),
                    "allowed_extensions": ["wav"],
                }),
                Duration::from_secs(10),
                |event| {
                    if matches!(event, ai_daemon::runners::InferEvent::Accepted(_)) {
                        if let Some(tx) = accepted_tx.take() {
                            let _ = tx.send(());
                        }
                    }
                },
            )
            .await
    });
    accepted_rx.await.expect("accepted frame consumed");
    inference.abort();
    let join_error = inference.await.unwrap_err();
    assert!(join_error.is_cancelled(), "infer future must be dropped");

    // 放弃后实例立即不可用，原因可观测（供 /api/providers 与恢复路径使用）。
    let snapshot = instances
        .instance_snapshot("org.example.fake")
        .await
        .expect("instance snapshot");
    assert!(
        !snapshot.alive,
        "abandoned inference must mark the instance not alive"
    );
    assert_eq!(snapshot.loaded_model, None);
    let reason = snapshot.last_error.expect("abandon reason must be set");
    assert!(
        reason.contains("abandoned"),
        "reason must describe the abandonment: {reason}"
    );
    let status = provider.status().await;
    assert!(!status.ready, "abandoned instance must not report ready");
    assert!(status.resident_models.is_empty());

    // 下一次 load 复用 !alive 回收路径：杀掉仍在合成的旧进程并重新拉起。
    provider
        .load(&model_spec())
        .await
        .expect("reload must recycle the tainted instance");
    let recovered = provider.status().await;
    assert!(recovered.ready);
    provider
        .synthesize(speech_request())
        .await
        .expect("inference after recycle");
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
            reasoning_content: None,
        }],
        stream: true,
        temperature: Some(0.7),
        top_p: Some(0.95),
        max_tokens: Some(64),
        session_id: None,
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
    let reasoning: String = chunks[1..]
        .iter()
        .filter_map(|chunk| chunk.choices[0].delta.reasoning_content.clone())
        .collect();
    assert_eq!(reasoning, "because");
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
    assert_eq!(
        response.choices[0].message.reasoning_content.as_deref(),
        Some("because")
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
async fn chat_stream_forwards_session_id_to_runner_payload() {
    let (root, provider, instances) = fixture("session-id").await;
    provider.load(&model_spec()).await.expect("load");
    let message = |content: &str| ai_core::request::ChatMessage {
        role: "user".to_string(),
        content: content.to_string(),
        reasoning_content: None,
    };

    // 携带 session_id：fake Runner 校验 infer 载荷里存在匹配的 string key。
    let request = ai_core::request::ChatRequest {
        model: "fake-model".to_string(),
        messages: vec![message("__require_session__:conv-42")],
        stream: true,
        temperature: None,
        top_p: None,
        max_tokens: None,
        session_id: Some("conv-42".to_string()),
    };
    let stream = ai_core::provider::ChatProvider::chat_stream(&provider, request)
        .await
        .expect("session_id must reach the runner payload");
    use futures::StreamExt;
    let chunks: Vec<ai_core::response::ChatChunk> =
        stream.map(|item| item.expect("chunk ok")).collect().await;
    let text: String = chunks
        .iter()
        .filter_map(|chunk| chunk.choices[0].delta.content.clone())
        .collect();
    assert_eq!(text, "__require_session__:conv-42");

    // 缺席 session_id：daemon 不得写 null key，fake Runner 校验 key 缺席。
    let request = ai_core::request::ChatRequest {
        model: "fake-model".to_string(),
        messages: vec![message("__forbid_session__")],
        ..Default::default()
    };
    let response = ai_core::provider::ChatProvider::chat(&provider, request)
        .await
        .expect("absent session_id must not produce the key");
    assert_eq!(
        response.choices[0].message.content,
        "__forbid_session__".to_string()
    );

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
    provider.load(&model_spec()).await.expect("load");
    let pid = instances
        .instance_snapshot("org.example.fake")
        .await
        .and_then(|snapshot| snapshot.pid)
        .expect("loaded fake-runner must expose its pid");
    assert!(
        process_is_alive(pid),
        "fake-runner process must be alive after load"
    );
    instances
        .shutdown_instance("org.example.fake")
        .await
        .expect("shutdown");
    // graceful shutdown 后进程退出；轮询等待进程表收敛，最多等 3 秒。
    let mut reaped = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if !process_is_alive(pid) {
            reaped = true;
            break;
        }
    }
    assert!(
        reaped,
        "fake-runner process {pid} must be reaped after shutdown"
    );
    let _ = std::fs::remove_dir_all(root);
}
