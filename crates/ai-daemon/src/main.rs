//! aiworkd — macOS Local AI Runtime daemon。
//!
//! 对外提供 OpenAI-compatible chat API；管理面提供 Provider 状态与模型
//! load/unload。默认只绑定 127.0.0.1:11435。

mod audio;
mod process_memory;
mod providers;
mod pull;
mod registry;
mod runtime;
mod scheduler;
mod tasks;

use std::path::{Path as FilePath, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path as AxumPath, Query, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use ai_core::errors::{AIError, ApiErrorBody};
use ai_core::provider::ProviderError;
use ai_core::request::{ChatRequest, SpeechRequest, TranscriptionRequest};
use ai_core::response::ModelEntry;

use crate::runtime::Runtime;
use crate::tasks::{chat_request_detail, speech_request_detail, transcription_request_detail};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const LOG_LEVEL_INFO: u8 = 0;
const LOG_LEVEL_DEBUG: u8 = 1;

type LogFilterHandle =
    tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>;

#[derive(Clone)]
struct AppState {
    runtime: Arc<Runtime>,
    log_filter: LogFilterHandle,
    log_level: Arc<AtomicU8>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
    model_count: usize,
}

#[derive(Serialize)]
struct LoggingLevelResponse {
    level: String,
}

#[derive(Debug, Deserialize)]
struct SetLoggingLevelRequest {
    level: String,
}

#[derive(Debug, Deserialize)]
struct LoadModelRequest {
    path: String,
    id: Option<String>,
    name: Option<String>,
    /// llm（默认）/ stt / tts。
    model_type: Option<String>,
    /// 显式推理后端。STT 支持 whisper.cpp（默认）、qwen3-asr-mlx 与 sherpa-onnx；
    /// TTS 支持 kokoro-mlx（默认）与 qwen3-tts。
    provider: Option<String>,
    context_length: Option<u64>,
    keep_alive: Option<String>,
}

#[tokio::main]
async fn main() {
    let initial_log_level = configured_log_level();
    let log_filter = init_tracing();

    // 注册表数据库与 GUI 模型仓库同根（~/Library/Application Support/MacAIConsole）。
    let db_path = std::env::var("HOME")
        .map(|home| {
            std::path::PathBuf::from(home)
                .join("Library/Application Support/MacAIConsole/models.db")
        })
        .unwrap_or_else(|_| std::path::PathBuf::from("models.db"));
    let mut runtime = Runtime::with_store(&db_path);
    // Phase 4：装配仓库随附 built-in Runner（无 Runner/模型时静默跳过，
    // 保留纯内置 Provider 路径）。
    bootstrap_runners(&mut runtime).await;
    let runtime = Arc::new(runtime);
    runtime.spawn_idle_reaper();

    let state = AppState {
        runtime: Arc::clone(&runtime),
        log_filter,
        log_level: Arc::new(AtomicU8::new(initial_log_level)),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/audio/transcriptions", post(audio_transcriptions))
        .route("/v1/audio/speech", post(audio_speech))
        .route("/api/runtime", get(runtime_info))
        .route("/api/logging", get(logging_level).post(set_logging_level))
        .route("/api/tasks", get(list_tasks))
        .route("/api/tasks/{id}", get(task_detail))
        .route("/api/providers", get(provider_statuses))
        .route("/api/models/load", post(register_and_load_model))
        .route("/api/models/pull", post(pull_model))
        .route("/api/models/{id}/load", post(load_registered_model))
        .route("/api/models/{id}/unload", post(unload_model))
        .route("/api/models/{id}", delete(unregister_model))
        .route("/api/models/{id}/rename", post(rename_model))
        .route("/api/models/{id}/keep-alive", post(set_model_keep_alive))
        .route("/api/models/{id}/voice", post(set_model_voice))
        .route("/api/models/{id}/voices", get(list_model_voices))
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .with_state(state);

    // 端口默认 11435；测试/并行场景可用 AIWORKD_PORT 覆盖（生产 GUI 拉起时不设置）。
    let port: u16 = std::env::var("AIWORKD_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(11435);
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("bind failed");

    tracing::info!(%addr, "aiworkd listening");
    println!("AI Runtime running at http://{addr}");

    // SIGTERM / SIGINT：先优雅卸载所有 worker，再退出，避免 worker 成为孤儿。
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("install SIGINT handler");

    tokio::select! {
        result = axum::serve(listener, app) => {
            result.expect("server error");
        }
        _ = sigterm.recv() => {
            tracing::info!("SIGTERM received; shutting down all workers");
            runtime.shutdown_all().await;
        }
        _ = sigint.recv() => {
            tracing::info!("SIGINT received; shutting down all workers");
            runtime.shutdown_all().await;
        }
    }
}

fn configured_log_level() -> u8 {
    match std::env::var("RUST_LOG") {
        Ok(level) if level.eq_ignore_ascii_case("debug") => LOG_LEVEL_DEBUG,
        _ => LOG_LEVEL_INFO,
    }
}

fn init_tracing() -> LogFilterHandle {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let (filter_layer, handle) = tracing_subscriber::reload::Layer::new(filter);
    tracing_subscriber::registry()
        .with(filter_layer)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(false),
        )
        .init();
    handle
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: VERSION.to_string(),
        model_count: state.runtime.list_models().await.len(),
    })
}

async fn logging_level(State(state): State<AppState>) -> Json<LoggingLevelResponse> {
    Json(LoggingLevelResponse {
        level: if state.log_level.load(Ordering::Relaxed) == LOG_LEVEL_DEBUG {
            "debug"
        } else {
            "info"
        }
        .to_string(),
    })
}

async fn set_logging_level(
    State(state): State<AppState>,
    Json(request): Json<SetLoggingLevelRequest>,
) -> Response {
    let level = request.level.to_ascii_lowercase();
    let value = match level.as_str() {
        "info" => LOG_LEVEL_INFO,
        "debug" => LOG_LEVEL_DEBUG,
        _ => {
            return api_error(
                AIError::InvalidRequest,
                "log level must be 'info' or 'debug'",
            );
        }
    };

    if let Err(error) = state
        .log_filter
        .reload(tracing_subscriber::EnvFilter::new(level.clone()))
    {
        return api_error(
            AIError::Internal,
            format!("log filter reload failed: {error}"),
        );
    }
    state.log_level.store(value, Ordering::Relaxed);
    tracing::info!(level, "log level changed");
    Json(LoggingLevelResponse { level }).into_response()
}

async fn list_models(State(state): State<AppState>) -> Json<Value> {
    let models: Vec<ModelEntry> = state
        .runtime
        .list_models()
        .await
        .into_iter()
        .map(|entry| ModelEntry {
            id: entry.spec.id,
            object: "model".to_string(),
            created: entry.loaded_at.unwrap_or(0),
            owned_by: format!("aiworkd/{}", entry.spec.provider),
            model_type: entry.spec.model_type,
            path: entry.spec.path,
        })
        .collect();
    Json(json!({"object": "list", "data": models}))
}

async fn runtime_info(State(state): State<AppState>) -> Json<ai_core::response::RuntimeInfo> {
    Json(state.runtime.runtime_info(VERSION).await)
}

#[derive(Debug, Default, Deserialize)]
struct TaskListQuery {
    completed_limit: Option<usize>,
}

async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<TaskListQuery>,
) -> Json<ai_core::response::TaskListResponse> {
    Json(
        state
            .runtime
            .tasks()
            .list(query.completed_limit.unwrap_or(100)),
    )
}

async fn task_detail(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    match state.runtime.tasks().get(&id) {
        Some(detail) => Json(detail).into_response(),
        None => api_error(AIError::TaskNotFound, format!("task '{id}' not found")),
    }
}

async fn provider_statuses(State(state): State<AppState>) -> Json<Value> {
    let providers: Vec<Value> = state
        .runtime
        .provider_statuses()
        .await
        .into_iter()
        .map(|(descriptor, status)| json!({"descriptor": descriptor, "status": status}))
        .collect();
    Json(json!({"data": providers}))
}

/// POST /api/models/pull —— 下载 HF 单文件或目录清单到模型仓库，可选择立即注册加载。
async fn pull_model(
    State(state): State<AppState>,
    Json(request): Json<pull::PullRequest>,
) -> Response {
    if !matches!(request.model_type.as_str(), "llm" | "stt" | "tts") {
        return api_error(
            AIError::InvalidRequest,
            format!("unsupported model_type '{}'", request.model_type),
        );
    }
    let (model_path, targets) = match pull::pull_targets(&request) {
        Ok(value) => value,
        Err(error) => return api_error(AIError::InvalidRequest, error),
    };
    let endpoint = match pull::configured_endpoint() {
        Ok(endpoint) => endpoint,
        Err(error) => return api_error(AIError::InvalidRequest, error),
    };
    tracing::info!(endpoint = %endpoint, repo = %request.repo, "starting model pull");
    for (filename, destination) in targets {
        if let Err(error) =
            pull::download_file(&endpoint, &request.repo, &filename, &destination).await
        {
            return api_error(AIError::Internal, error);
        }
    }
    finish_pull(state, request, model_path).await
}

/// 下载完成后的公共尾部：按类型注册并加载。
async fn finish_pull(state: AppState, request: pull::PullRequest, dest: PathBuf) -> Response {
    let id = request
        .id
        .clone()
        .unwrap_or_else(|| default_model_id(&dest));
    if request.auto_load == Some(false) {
        return Json(json!({
            "id": id,
            "state": "downloaded",
            "path": dest,
        }))
        .into_response();
    }
    let load_request = LoadModelRequest {
        path: dest.to_string_lossy().into_owned(),
        id: Some(id),
        name: None,
        model_type: Some(request.model_type.clone()),
        provider: request.provider.clone(),
        context_length: None,
        keep_alive: None,
    };
    register_and_load_model(State(state), Json(load_request)).await
}

async fn register_and_load_model(
    State(state): State<AppState>,
    Json(request): Json<LoadModelRequest>,
) -> Response {
    let path = match FilePath::new(&request.path).canonicalize() {
        Ok(path) => path,
        Err(error) => {
            return api_error(
                AIError::ModelNotFound,
                format!("cannot open model '{}': {error}", request.path),
            )
        }
    };
    let model_type = match request.model_type.as_deref() {
        Some("llm") | None => "llm",
        Some("stt") => "stt",
        Some("tts") => "tts",
        Some(other) => {
            return api_error(
                AIError::InvalidRequest,
                format!("unsupported model_type '{other}' (expected \"llm\", \"stt\" or \"tts\")"),
            );
        }
    };
    let requested_provider = request.provider.as_deref().map(str::trim);
    let provider = match (model_type, requested_provider) {
        ("llm", None | Some("llama.cpp")) => "llama.cpp",
        ("llm", Some("mlx-lm")) => "mlx-lm",
        ("stt", None | Some("whisper.cpp")) => "whisper.cpp",
        ("stt", Some("qwen3-asr-mlx")) => "qwen3-asr-mlx",
        ("stt", Some("sherpa-onnx")) => "sherpa-onnx",
        ("tts", None | Some("kokoro-mlx")) => "kokoro-mlx",
        ("tts", Some("qwen3-tts")) => "qwen3-tts",
        (other_type, None) => {
            return api_error(
                AIError::InvalidRequest,
                format!("no default provider is defined for model type '{other_type}'"),
            );
        }
        (_, Some(other)) => {
            return api_error(
                AIError::InvalidRequest,
                format!("provider '{other}' does not support model type '{model_type}'"),
            );
        }
    };
    match provider {
        "llama.cpp" if path.extension().and_then(|value| value.to_str()) != Some("gguf") => {
            return api_error(
                AIError::InvalidRequest,
                format!(
                    "expected a .gguf model for provider 'llama.cpp', got '{}'",
                    path.display()
                ),
            );
        }
        "whisper.cpp" if path.extension().and_then(|value| value.to_str()) != Some("bin") => {
            return api_error(
                AIError::InvalidRequest,
                format!(
                    "expected a .bin model for provider 'whisper.cpp', got '{}'",
                    path.display()
                ),
            );
        }
        "kokoro-mlx" if !path.is_dir() => {
            return api_error(
                AIError::InvalidRequest,
                format!(
                    "expected a model directory containing model.safetensors for provider 'kokoro-mlx', got '{}'",
                    path.display()
                ),
            );
        }
        "qwen3-tts" if !path.is_dir() => {
            return api_error(
                AIError::InvalidRequest,
                format!(
                    "expected a model directory for provider 'qwen3-tts', got '{}'",
                    path.display()
                ),
            );
        }
        "mlx-lm" => {
            if let Err(error) = providers::mlx_lm::validate_model_dir(&path) {
                return provider_error(error);
            }
        }
        "qwen3-asr-mlx" => {
            if let Err(error) = providers::qwen3_asr::validate_mlx_8bit_model_dir(&path) {
                return provider_error(error);
            }
        }
        "sherpa-onnx" => {
            if let Err(error) = providers::sherpa_onnx::validate_model_dir(&path) {
                return provider_error(error);
            }
        }
        _ => {}
    }

    let id = request.id.unwrap_or_else(|| default_model_id(&path));
    if !valid_model_id(&id) {
        return api_error(
            AIError::InvalidRequest,
            "model id may contain only letters, numbers, '.', '_' and '-'",
        );
    }
    let size_bytes = match recursive_path_size(&path) {
        Ok(size) => size,
        Err(error) => {
            return api_error(
                AIError::ModelNotFound,
                format!("cannot inspect model '{}': {error}", path.display()),
            )
        }
    };
    let (format, keep_alive_default, memory_estimate) = match provider {
        "llama.cpp" => (Some("gguf"), Some("5m"), Some(size_bytes)),
        // MLX 权重体积即内存占用主体；real RSS 由 worker 上报。
        "mlx-lm" => (Some("mlx"), Some("5m"), Some(size_bytes)),
        "whisper.cpp" => (Some("bin"), Some("always"), Some(size_bytes)),
        // 6 GiB is MacAI's initial scheduling estimate; real RSS is reported from the worker.
        "qwen3-asr-mlx" => (
            Some("qwen3-asr-mlx-8bit"),
            Some("always"),
            Some(2 * 1024 * 1024 * 1024),
        ),
        "qwen3-tts" => (Some("qwen3-tts"), Some("always"), Some(size_bytes)),
        "sherpa-onnx" => (
            Some("sherpa-onnx-zh-int8-2025"),
            Some("always"),
            Some(size_bytes),
        ),
        _ => (None, Some("always"), Some(size_bytes)),
    };
    let spec = ai_core::model::ModelSpec {
        id: id.clone(),
        name: request.name.unwrap_or_else(|| id.clone()),
        model_type: model_type.to_string(),
        provider: provider.to_string(),
        source: None,
        path: Some(path.to_string_lossy().into_owned()),
        format: format.map(String::from),
        size_bytes: Some(size_bytes),
        memory_estimate,
        keep_alive: request
            .keep_alive
            .or_else(|| keep_alive_default.map(String::from)),
        context_length: request.context_length.or(Some(4096)),
        default_voice: None,
    };

    match state.runtime.register_and_load(spec).await {
        Ok(handle) => Json(json!({
            "id": handle.model_id,
            "provider": handle.provider_id,
            "state": "ready"
        }))
        .into_response(),
        Err(error) => provider_error(error),
    }
}

async fn load_registered_model(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    match state.runtime.load_model(&id).await {
        Ok(handle) => Json(json!({
            "id": handle.model_id,
            "provider": handle.provider_id,
            "state": "ready"
        }))
        .into_response(),
        Err(error) => provider_error(error),
    }
}

async fn unload_model(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    if state.runtime.get_model(&id).await.is_none() {
        return api_error(AIError::ModelNotFound, format!("model '{id}' not found"));
    }
    match state.runtime.unload_model(&id).await {
        Ok(()) => Json(json!({"id": id, "state": "unloaded"})).into_response(),
        Err(error) => provider_error(error),
    }
}

/// DELETE /api/models/{id} —— 从注册表删除模型（已加载时先卸载 worker）。
/// 只移除注册记录，模型文件保留在磁盘上。
async fn unregister_model(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    match state.runtime.unregister_model(&id).await {
        Ok(()) => Json(json!({"id": id, "deleted": true})).into_response(),
        Err(error) => provider_error(error),
    }
}

#[derive(Debug, Deserialize)]
struct RenameModelRequest {
    new_id: String,
}

/// POST /api/models/{id}/rename —— 重命名模型 ID（设置保留，已加载先卸载）。
async fn rename_model(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<RenameModelRequest>,
) -> Response {
    match state.runtime.rename_model(&id, &req.new_id).await {
        Ok(()) => Json(json!({"id": req.new_id.trim(), "renamed_from": id})).into_response(),
        Err(error) => provider_error(error),
    }
}

#[derive(Debug, Deserialize)]
struct SetKeepAliveRequest {
    /// "always" / "-1" / "0" / "5m" / "30m" / 纯数字秒。缺省视为 always。
    keep_alive: Option<String>,
}

async fn set_model_keep_alive(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<SetKeepAliveRequest>,
) -> Response {
    let keep_alive = req
        .keep_alive
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    if let Some(value) = &keep_alive {
        if value != "always" && value != "-1" && scheduler::parse_keep_alive(Some(value)).is_none()
        {
            return api_error(
                AIError::InvalidRequest,
                format!("invalid keep_alive '{value}': use e.g. 0 / 5m / 30m / 2h / always"),
            );
        }
    }
    if state.runtime.set_keep_alive(&id, keep_alive.clone()).await {
        Json(json!({"id": id, "keep_alive": keep_alive})).into_response()
    } else {
        api_error(AIError::ModelNotFound, format!("model '{id}' not found"))
    }
}

#[derive(Debug, Deserialize)]
struct SetVoiceRequest {
    voice: String,
}

/// 设置 TTS 模型的默认音色。空字符串清除（回到 provider 内置缺省 zf_001）。
async fn set_model_voice(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<SetVoiceRequest>,
) -> Response {
    let voice = req.voice.trim().to_string();
    let voice = (!voice.is_empty()).then_some(voice);
    if state.runtime.set_default_voice(&id, voice.clone()).await {
        Json(json!({"id": id, "default_voice": voice})).into_response()
    } else {
        api_error(AIError::ModelNotFound, format!("model '{id}' not found"))
    }
}

/// 列出 TTS 模型可用的音色：Qwen3-TTS 使用 provider 内置 speaker，其他
/// 目录模型扫描 voices/*.safetensors。
async fn list_model_voices(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let Some(spec) = state.runtime.get_model(&id).await else {
        return api_error(AIError::ModelNotFound, format!("model '{id}' not found"));
    };
    let (voices, default_voice) = if spec.provider == "qwen3-tts" {
        (
            providers::qwen3_tts::BUILTIN_VOICES
                .iter()
                .map(|voice| (*voice).to_string())
                .collect(),
            spec.default_voice
                .clone()
                .filter(|voice| providers::qwen3_tts::BUILTIN_VOICES.contains(&voice.as_str()))
                .or_else(|| Some(providers::qwen3_tts::DEFAULT_VOICE.to_string())),
        )
    } else {
        let mut voices: Vec<String> = Vec::new();
        if let Some(path) = &spec.path {
            let dir = FilePath::new(path).join("voices");
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if let Some(stem) = name.strip_suffix(".safetensors") {
                        voices.push(stem.to_string());
                    }
                }
            }
        }
        voices.sort();
        (voices, spec.default_voice.clone())
    };
    Json(json!({"id": id, "voices": voices, "default_voice": default_voice})).into_response()
}

async fn chat_completions(State(state): State<AppState>, Json(req): Json<ChatRequest>) -> Response {
    let task = state
        .runtime
        .start_task("chat", req.model.clone(), chat_request_detail(&req));
    let request_id = task.id().to_string();
    tracing::info!(%request_id, model = %req.model, stream = req.stream, "chat request");

    let model_id = req.model.clone();
    let model_lease = state.runtime.model_lease(&model_id);
    let provider = match state.runtime.chat_provider(&model_id).await {
        Ok(provider) => provider,
        Err(error) => {
            task.fail(error.message.clone());
            return provider_error(error);
        }
    };
    if let Some(spec) = state.runtime.get_model(&model_id).await {
        task.set_provider(Some(spec.provider));
    }
    state.runtime.touch_model(&model_id).await;

    if req.stream {
        match provider.chat_stream(req).await {
            Ok(stream) => {
                let response_stream = async_stream::stream! {
                    let _model_lease = model_lease;
                    futures::pin_mut!(stream);
                    while let Some(result) = stream.next().await {
                        match result {
                            Ok(chunk) => {
                                for choice in &chunk.choices {
                                    if let Some(content) = choice.delta.content.as_deref() {
                                        task.append_output(content);
                                    }
                                    if choice.finish_reason.is_some() {
                                        task.set_finish_reason(choice.finish_reason.clone());
                                    }
                                }
                                if let Some(usage) = &chunk.usage {
                                    task.set_usage(usage.clone());
                                }
                                let event = Event::default().json_data(&chunk).unwrap();
                                yield Ok::<_, std::convert::Infallible>(event);
                            }
                            Err(error) => {
                                task.fail(error.message.clone());
                                let event = Event::default()
                                    .event("error")
                                    .json_data(ApiErrorBody::new(error.kind, error.message))
                                    .unwrap();
                                yield Ok::<_, std::convert::Infallible>(event);
                                return;
                            }
                        }
                    }
                    task.succeed_current();
                    yield Ok::<_, std::convert::Infallible>(Event::default().data("[DONE]"));
                };
                let sse = Sse::new(response_stream)
                    .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)));
                (StatusCode::OK, sse).into_response()
            }
            Err(error) => {
                task.fail(error.message.clone());
                provider_error(error)
            }
        }
    } else {
        match provider.chat(req).await {
            Ok(response) => {
                let choice = response.choices.first();
                task.succeed(ai_core::response::TaskResultDetail {
                    output_text: choice.map(|choice| choice.message.content.clone()),
                    finish_reason: choice.and_then(|choice| choice.finish_reason.clone()),
                    prompt_tokens: Some(response.usage.prompt_tokens),
                    completion_tokens: Some(response.usage.completion_tokens),
                    total_tokens: Some(response.usage.total_tokens),
                    ..ai_core::response::TaskResultDetail::default()
                });
                Json(response).into_response()
            }
            Err(error) => {
                task.fail(error.message.clone());
                provider_error(error)
            }
        }
    }
}

async fn audio_transcriptions(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut upload: Option<UploadedAudio> = None;
    let mut model = "whisper-base".to_string();
    let mut language = None;
    let mut response_format = "json".to_string();

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                return api_error(
                    AIError::InvalidRequest,
                    format!("invalid multipart request: {error}"),
                )
            }
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                let file_name = field.file_name().unwrap_or("audio.wav").to_string();
                let bytes = match field.bytes().await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        return api_error(
                            AIError::InvalidRequest,
                            format!("cannot read uploaded audio: {error}"),
                        )
                    }
                };
                if bytes.is_empty() {
                    return api_error(AIError::InvalidRequest, "uploaded audio is empty");
                }
                let upload_size = bytes.len() as u64;
                let extension = FilePath::new(&file_name)
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(|value| value.to_ascii_lowercase());
                // 解码是 CPU 密集操作；放 blocking 线程，避免拖住 daemon 的事件循环。
                // Bytes → Vec 在引用计数为 1 时零拷贝。
                let bytes: Vec<u8> = bytes.into();
                let normalized = match tokio::task::spawn_blocking(move || {
                    audio::normalize_to_pcm_wav(bytes, extension.as_deref())
                })
                .await
                {
                    Ok(Ok(data)) => data,
                    Ok(Err(audio::NormalizeError(reason))) => {
                        return api_error(AIError::InvalidRequest, reason)
                    }
                    Err(error) => {
                        return api_error(
                            AIError::Internal,
                            format!("audio normalization failed: {error}"),
                        )
                    }
                };
                let audio_duration_ms = audio::wav_duration_ms(&normalized);
                let uploaded = UploadedAudio::new(
                    state.runtime.next_request_id(),
                    file_name,
                    upload_size,
                    audio_duration_ms,
                );
                if let Err(error) = tokio::fs::write(&uploaded.path, &normalized).await {
                    return api_error(
                        AIError::Internal,
                        format!("cannot stage uploaded audio: {error}"),
                    );
                }
                upload = Some(uploaded);
            }
            "model" => match field.text().await {
                Ok(value) if !value.trim().is_empty() => model = value,
                Ok(_) => {}
                Err(error) => {
                    return api_error(
                        AIError::InvalidRequest,
                        format!("cannot read model field: {error}"),
                    )
                }
            },
            "language" => match field.text().await {
                Ok(value) if !value.trim().is_empty() => language = Some(value),
                Ok(_) => {}
                Err(error) => {
                    return api_error(
                        AIError::InvalidRequest,
                        format!("cannot read language field: {error}"),
                    )
                }
            },
            "response_format" => match field.text().await {
                Ok(value) if !value.trim().is_empty() => response_format = value,
                Ok(_) => {}
                Err(error) => {
                    return api_error(
                        AIError::InvalidRequest,
                        format!("cannot read response_format field: {error}"),
                    )
                }
            },
            _ => {}
        }
    }

    let Some(upload) = upload else {
        return api_error(
            AIError::InvalidRequest,
            "multipart field 'file' is required",
        );
    };
    if !matches!(response_format.as_str(), "json" | "text") {
        return api_error(
            AIError::InvalidRequest,
            "response_format must be 'json' or 'text'",
        );
    }
    let request = TranscriptionRequest {
        model,
        file: Some(upload.path.to_string_lossy().into_owned()),
        language,
        response_format: Some(response_format.clone()),
    };
    let task = state.runtime.start_task(
        "stt",
        request.model.clone(),
        transcription_request_detail(
            &request,
            Some(upload.file_name.clone()),
            Some(upload.file_size_bytes),
            upload.audio_duration_ms,
        ),
    );
    match state.runtime.transcribe(request, task).await {
        Ok(response) if response_format == "text" => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            response.text,
        )
            .into_response(),
        Ok(response) => Json(response).into_response(),
        Err(error) => provider_error(error),
    }
}

async fn audio_speech(
    State(state): State<AppState>,
    Json(mut request): Json<SpeechRequest>,
) -> Response {
    if request.model.trim().is_empty() {
        request.model = "kokoro-mlx".to_string();
    }
    let task = state.runtime.start_task(
        "tts",
        request.model.clone(),
        speech_request_detail(&request),
    );
    match state.runtime.synthesize(request, task).await {
        Ok(speech) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, speech.content_type)
            .header(header::CONTENT_LENGTH, speech.bytes)
            .body(Body::from(speech.audio))
            .unwrap(),
        Err(error) => provider_error(error),
    }
}

struct UploadedAudio {
    path: PathBuf,
    file_name: String,
    file_size_bytes: u64,
    audio_duration_ms: Option<u64>,
}

impl UploadedAudio {
    fn new(
        request_id: String,
        file_name: String,
        file_size_bytes: u64,
        audio_duration_ms: Option<u64>,
    ) -> Self {
        Self {
            path: std::env::temp_dir().join(format!("macai-upload-{request_id}.wav")),
            file_name,
            file_size_bytes,
            audio_duration_ms,
        }
    }
}

impl Drop for UploadedAudio {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn default_model_id(path: &FilePath) -> String {
    let value = if path.is_dir() {
        path.file_name()
    } else {
        path.file_stem()
    };
    value
        .and_then(|value| value.to_str())
        .unwrap_or("model")
        .to_string()
}

fn recursive_path_size(path: &FilePath) -> std::io::Result<u64> {
    let metadata = path.metadata()?;
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        total = total.saturating_add(recursive_path_size(&entry.path())?);
    }
    Ok(total)
}

fn valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn provider_error(error: ProviderError) -> Response {
    api_error(error.kind, error.message)
}

fn api_error(error: AIError, message: impl Into<String>) -> Response {
    let status =
        StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ApiErrorBody::new(error, message))).into_response()
}

/// Phase 4：装配仓库随附 built-in Runner。
///
/// 流程：定位 Runner 目录（`MACAI_RUNNERS_DIR` → cwd `runners`/`../runners`）
/// → discovery（built-in root 直接信任）→ 逐 manifest 读取 bundled Model
/// Profile → 持久化 profile snapshot（models.db）→ 对已有模型 artifact 的
/// profile 建立 RunnerProvider 绑定并 `attach_runner` 进 Runtime。
///
/// 找不到 Runner 目录、无 python-uv Runner 或模型目录缺失时静默跳过，保留纯
/// 内置 Provider 路径；模型目录缺失表示用户尚未下载 artifact，不假装 ready。
/// Plugins 目录与显式信任留到安装 slice。
async fn bootstrap_runners(runtime: &mut Runtime) {
    use std::collections::HashMap;
    use std::collections::HashSet;

    use ai_daemon::runners::{
        EnvironmentManager, EnvironmentManagerConfig, ModelProfile, RunnerInstanceManager,
        RunnerModelBinding, RunnerProvider, RunnerRegistry, RunnerState,
    };

    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let app_support = PathBuf::from(&home).join("Library/Application Support/MacAIConsole");
    let runner_root = match std::env::var_os("MACAI_RUNNERS_DIR") {
        Some(dir) => Some(PathBuf::from(dir)),
        None => match std::env::current_dir() {
            Ok(cwd) => [cwd.join("runners"), cwd.join("../runners")]
                .into_iter()
                .find(|candidate| candidate.is_dir()),
            Err(_) => None,
        },
    };
    let Some(runner_root) = runner_root else {
        tracing::info!("no built-in runners dir; Runner assembly skipped");
        return;
    };
    if !runner_root.is_dir() {
        tracing::info!(path = %runner_root.display(), "built-in runners dir unavailable");
        return;
    }

    let registry = RunnerRegistry::discover(&[runner_root.clone()], &[], &HashSet::new());
    let entries: Vec<_> = registry
        .entries()
        .iter()
        .cloned()
        .filter(|entry| {
            entry.manifest.is_some()
                && matches!(entry.state, RunnerState::Trusted)
                && entry
                    .manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.runtime.runtime_type == "python-uv")
        })
        .collect();
    if entries.is_empty() {
        tracing::info!(path = %runner_root.display(), "no trusted python-uv Runner discovered");
        return;
    }

    let environments = EnvironmentManager::new(EnvironmentManagerConfig::for_app_support());
    let temp_root = app_support.join("Runtimes/tmp");
    if let Err(error) = std::fs::create_dir_all(&temp_root) {
        tracing::warn!(%error, path = %temp_root.display(), "cannot create runner temp root");
        return;
    }
    let instances = Arc::new(RunnerInstanceManager::new(
        registry,
        environments,
        temp_root.clone(),
    ));
    let models_root = app_support.join("Models");
    let mut providers: HashMap<String, Arc<RunnerProvider>> = HashMap::new();

    for entry in &entries {
        let manifest = entry.manifest.as_ref().expect("filtered above");
        let runner_id = manifest.id.clone();
        for model in &manifest.models {
            let profile = match ModelProfile::load(&entry.root.join(&model.profile)) {
                Ok(profile) => profile,
                Err(error) => {
                    tracing::warn!(
                        profile = %model.profile,
                        runner = %runner_id,
                        %error,
                        "skip invalid bundled Model Profile"
                    );
                    continue;
                }
            };
            let kind = if profile.capabilities.iter().any(|cap| cap == "stt.v1") {
                "stt"
            } else if profile.capabilities.iter().any(|cap| cap == "tts.v1") {
                "tts"
            } else {
                "llm"
            };
            let model_dir = models_root.join(kind).join(&profile.artifacts.directory);
            if !model_dir.is_dir() {
                tracing::info!(
                    profile = %profile.id,
                    path = %model_dir.display(),
                    "model artifact missing; Runner binding skipped (not pretending ready)"
                );
                continue;
            }
            match runtime.register_runner_profile(&profile) {
                Ok(Some(digest)) => {
                    tracing::info!(profile = %profile.id, digest = %digest, "runner profile persisted")
                }
                Ok(None) => {
                    tracing::info!(profile = %profile.id, "runner profile registered (memory mode)")
                }
                Err(error) => {
                    tracing::warn!(profile = %profile.id, %error, "profile persist failed")
                }
            }
            let provider = match providers.get(&runner_id).cloned() {
                Some(provider) => provider,
                None => {
                    let provider = Arc::new(RunnerProvider::new(
                        runner_id.clone(),
                        instances.clone(),
                        temp_root.clone(),
                    ));
                    providers.insert(runner_id.clone(), provider.clone());
                    provider
                }
            };
            provider
                .bind_model(RunnerModelBinding {
                    model_id: profile.id.clone(),
                    profile: profile.clone(),
                    environment_id: manifest.runtime.id.clone(),
                    artifact_root: model_dir.clone(),
                })
                .await;
            tracing::info!(
                runner = %runner_id,
                profile = %profile.id,
                artifact = %model_dir.display(),
                "runner model bound"
            );
        }
    }

    for (runner_id, provider) in providers {
        runtime.attach_runner(provider, instances.clone());
        tracing::info!(runner = %runner_id, "built-in Runner attached");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_model_aliases() {
        assert!(valid_model_id("SmolLM2-135M.Q4_K_M"));
        assert!(!valid_model_id("../model"));
        assert!(!valid_model_id("model name"));
    }

    #[test]
    fn preserves_directory_model_id_and_sums_snapshot_size() {
        let path = std::env::temp_dir().join(format!("Qwen3-ASR-0.6B-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("tokenizer")).unwrap();
        std::fs::write(path.join("model.safetensors"), b"1234").unwrap();
        std::fs::write(path.join("tokenizer/config.json"), b"12").unwrap();

        assert_eq!(
            default_model_id(&path),
            path.file_name().unwrap().to_string_lossy()
        );
        assert_eq!(recursive_path_size(&path).unwrap(), 6);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn default_model_id_removes_only_the_last_file_extension() {
        let path = std::env::temp_dir().join("whisper.large-v3.q5_0.bin");
        std::fs::write(&path, b"").unwrap();
        assert_eq!(default_model_id(&path), "whisper.large-v3.q5_0");
        std::fs::remove_file(path).unwrap();
    }

    fn mock_model() -> ai_core::model::ModelSpec {
        ai_core::model::ModelSpec {
            id: "mock-task".to_string(),
            name: "Mock task model".to_string(),
            model_type: "llm".to_string(),
            provider: "mock".to_string(),
            source: None,
            path: None,
            format: Some("mock".to_string()),
            size_bytes: Some(0),
            memory_estimate: Some(0),
            keep_alive: Some("always".to_string()),
            context_length: Some(4096),
            default_voice: None,
        }
    }

    fn app_state(runtime: Arc<Runtime>) -> AppState {
        let (_, log_filter) =
            tracing_subscriber::reload::Layer::new(tracing_subscriber::EnvFilter::new("info"));
        AppState {
            runtime,
            log_filter,
            log_level: Arc::new(AtomicU8::new(LOG_LEVEL_INFO)),
        }
    }

    fn qwen_tts_model() -> ai_core::model::ModelSpec {
        ai_core::model::ModelSpec {
            id: "qwen-tts".to_string(),
            name: "Qwen3-TTS".to_string(),
            model_type: "tts".to_string(),
            provider: "qwen3-tts".to_string(),
            source: None,
            path: None,
            format: Some("qwen3-tts".to_string()),
            size_bytes: None,
            memory_estimate: None,
            keep_alive: Some("always".to_string()),
            context_length: None,
            default_voice: None,
        }
    }

    #[tokio::test]
    async fn qwen3_tts_voice_endpoint_exposes_builtin_voices() {
        let runtime = Arc::new(Runtime::new());
        runtime.register(qwen_tts_model()).await;

        let response =
            list_model_voices(State(app_state(runtime)), AxumPath("qwen-tts".to_string())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        let voices = payload["voices"].as_array().unwrap();
        assert_eq!(voices.len(), 9);
        assert_eq!(voices[0], "Vivian");
        assert_eq!(payload["default_voice"], "Vivian");
        assert!(voices.iter().any(|voice| voice == "Ono_Anna"));
    }

    #[tokio::test]
    async fn non_stream_chat_records_result_and_usage() {
        let runtime = Arc::new(Runtime::new());
        runtime.register(mock_model()).await;
        let request = ChatRequest {
            model: "mock-task".to_string(),
            messages: vec![ai_core::request::ChatMessage {
                role: "user".to_string(),
                content: "hello task".to_string(),
            }],
            stream: false,
            temperature: Some(0.2),
            max_tokens: Some(32),
        };

        let response = chat_completions(State(app_state(runtime.clone())), Json(request)).await;

        assert_eq!(response.status(), StatusCode::OK);
        let list = runtime.tasks().list(100);
        assert_eq!(list.running.len(), 0);
        assert_eq!(list.completed.len(), 1);
        assert_eq!(list.completed[0].status, "succeeded");
        let detail = runtime.tasks().get(&list.completed[0].id).unwrap();
        assert_eq!(detail.result.output_text.as_deref(), Some("hello task"));
        assert_eq!(detail.result.total_tokens, Some(20));
        assert_eq!(detail.request.temperature, Some(0.2));
        assert_eq!(detail.request.max_tokens, Some(32));
    }

    #[tokio::test]
    async fn stream_chat_finishes_task_and_releases_active_count() {
        let runtime = Arc::new(Runtime::new());
        runtime.register(mock_model()).await;
        let response = chat_completions(
            State(app_state(runtime.clone())),
            Json(ChatRequest {
                model: "mock-task".to_string(),
                messages: vec![ai_core::request::ChatMessage {
                    role: "user".to_string(),
                    content: "stream me".to_string(),
                }],
                stream: true,
                temperature: None,
                max_tokens: None,
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(runtime.tasks().running_count(), 1);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("[DONE]"));
        assert_eq!(runtime.tasks().running_count(), 0);
        let list = runtime.tasks().list(100);
        assert_eq!(list.completed[0].status, "succeeded");
        let detail = runtime.tasks().get(&list.completed[0].id).unwrap();
        assert_eq!(detail.result.output_text.as_deref(), Some("stream me"));
        // 流式终帧的 usage 必须落进任务统计，并派生生成吞吐。
        assert_eq!(detail.result.prompt_tokens, Some(9));
        assert_eq!(detail.result.completion_tokens, Some(9));
        assert_eq!(detail.result.total_tokens, Some(18));
        let speed = detail.result.tokens_per_second.expect("tokens/s recorded");
        assert!(speed > 0.0);
    }

    #[tokio::test]
    async fn dropping_stream_response_marks_task_cancelled() {
        let runtime = Arc::new(Runtime::new());
        runtime.register(mock_model()).await;
        let response = chat_completions(
            State(app_state(runtime.clone())),
            Json(ChatRequest {
                model: "mock-task".to_string(),
                messages: vec![ai_core::request::ChatMessage {
                    role: "user".to_string(),
                    content: "cancel me".to_string(),
                }],
                stream: true,
                temperature: None,
                max_tokens: None,
            }),
        )
        .await;

        assert_eq!(runtime.tasks().running_count(), 1);
        drop(response);
        tokio::task::yield_now().await;
        assert_eq!(runtime.tasks().running_count(), 0);
        let list = runtime.tasks().list(100);
        assert_eq!(list.completed[0].status, "cancelled");
        assert_eq!(list.completed[0].error.as_deref(), Some("客户端连接已中断"));
    }

    #[tokio::test]
    async fn valid_chat_for_missing_model_records_failed_task() {
        let runtime = Arc::new(Runtime::new());
        let response = chat_completions(
            State(app_state(runtime.clone())),
            Json(ChatRequest {
                model: "missing-task-model".to_string(),
                messages: vec![ai_core::request::ChatMessage {
                    role: "user".to_string(),
                    content: "will fail".to_string(),
                }],
                stream: false,
                temperature: None,
                max_tokens: None,
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(runtime.tasks().running_count(), 0);
        let list = runtime.tasks().list(100);
        assert_eq!(list.completed.len(), 1);
        assert_eq!(list.completed[0].status, "failed");
        assert!(list.completed[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("not found"));
    }

    #[tokio::test]
    async fn task_detail_for_unknown_id_is_standard_not_found() {
        let response = task_detail(
            State(app_state(Arc::new(Runtime::new()))),
            AxumPath("missing-task".to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// 回归：卸载已加载模型时，unregister / rename 曾在 registry 读守卫内
    /// 触发 set_state 写锁互等（tokio RwLock 写优先）导致请求永久挂起。
    /// 用超时兜底，回归时快速失败而不是卡死测试套件。
    #[tokio::test]
    async fn unregister_loaded_model_completes_without_deadlock() {
        let runtime = Arc::new(Runtime::new());
        runtime.register(mock_model()).await;
        runtime.load_model("mock-task").await.unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            unregister_model(
                State(app_state(runtime.clone())),
                AxumPath("mock-task".to_string()),
            ),
        )
        .await
        .expect("unregister of a loaded model must not deadlock");
        assert_eq!(result.status(), StatusCode::OK);
        assert!(runtime.get_model("mock-task").await.is_none());
    }
}
