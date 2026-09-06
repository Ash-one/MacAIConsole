//! aiworkd — macOS Local AI Runtime daemon。
//!
//! 对外提供 OpenAI-compatible chat API；管理面提供 Provider 状态与模型
//! load/unload。默认只绑定 127.0.0.1:11435。

mod audio;
mod providers;
mod pull;
mod registry;
mod runtime;
mod scheduler;
mod tasks;

use std::collections::HashMap;
use std::path::{Path as FilePath, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
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
use sha2::Digest;
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
    routing_tokens: Arc<StdMutex<HashMap<String, RoutingToken>>>,
}

#[derive(Clone)]
struct RoutingToken {
    path: String,
    fingerprint: String,
    matched: ai_daemon::runners::DetectorMatch,
    expires_at: std::time::Instant,
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
struct PullModelProfileRequest {
    #[serde(default = "default_true")]
    auto_load: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct LoadModelRequest {
    path: String,
    id: Option<String>,
    name: Option<String>,
    /// llm（默认）/ stt / tts。
    model_type: Option<String>,
    /// 显式推理后端。STT 默认使用 org.macai.whisper.cpp；
    /// TTS 支持 qwen3-tts；Python 引擎（Kokoro / Qwen3-ASR）由 Runner 装配，
    /// 显式指定其 runner provider id（org.macai.*）走 descriptor 能力判定。
    provider: Option<String>,
    routing_token: Option<String>,
    context_length: Option<u64>,
    keep_alive: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InspectModelsRequest {
    paths: Vec<String>,
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
    // 装配仓库随附 built-in Runner（无 Runner/模型时静默跳过；生产 Provider
    // 全部从此边界进入，测试 provider 只由 Runtime::new 注入）。
    bootstrap_runners(&mut runtime).await;
    let runtime = Arc::new(runtime);
    runtime.spawn_idle_reaper();

    let state = AppState {
        runtime: Arc::clone(&runtime),
        log_filter,
        log_level: Arc::new(AtomicU8::new(initial_log_level)),
        routing_tokens: Arc::new(StdMutex::new(HashMap::new())),
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
        .route("/api/runners", get(runner_statuses))
        .route("/api/models/inspect", post(inspect_models))
        .route("/api/model-profiles", get(model_profiles))
        .route("/api/model-profiles/{id}/pull", post(pull_model_profile))
        .route(
            "/api/runners/{runner}/install",
            post(install_runner_environment),
        )
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
            requested_provider: entry.spec.requested_provider,
            provider_selection_reason: entry.spec.provider_selection_reason,
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

fn profile_model_type(profile: &ai_daemon::runners::ModelProfile) -> Result<&'static str, String> {
    match profile.capabilities.as_slice() {
        [capability] if capability == "chat.v1" => Ok("llm"),
        [capability] if capability == "stt.v1" => Ok("stt"),
        [capability] if capability == "tts.v1" => Ok("tts"),
        capabilities => Err(format!(
            "profile '{}' must declare exactly one GUI model capability, got {:?}",
            profile.id, capabilities
        )),
    }
}

fn model_profile_json(profile: &ai_daemon::runners::ModelProfile) -> Result<Value, String> {
    let directory = (profile.format == "directory").then_some(&profile.artifacts.directory);
    Ok(json!({
        "id": profile.id,
        "name": profile.name,
        "model_type": profile_model_type(profile)?,
        "runner": profile.runner,
        "source_repo": profile.source.repo,
        "directory": directory,
        "files": profile.artifacts.files,
        "memory_estimate_bytes": profile.resources.memory_estimate_bytes,
    }))
}

async fn model_profiles(State(state): State<AppState>) -> Response {
    let mut data = Vec::new();
    for profile in state.runtime.runner_profiles() {
        match model_profile_json(&profile) {
            Ok(value) => data.push(value),
            Err(error) => return api_error(AIError::InvalidRequest, error),
        }
    }
    Json(json!({ "data": data })).into_response()
}

fn pull_request_for_profile(
    profile: &ai_daemon::runners::ModelProfile,
    auto_load: bool,
) -> Result<pull::PullRequest, String> {
    let model_type = profile_model_type(profile)?.to_string();
    let (filename, files, directory) = if profile.format == "directory" {
        (
            None,
            profile.artifacts.files.clone(),
            Some(profile.artifacts.directory.clone()),
        )
    } else if profile.artifacts.files.len() == 1 {
        (Some(profile.artifacts.files[0].clone()), Vec::new(), None)
    } else {
        return Err(format!(
            "file profile '{}' must declare exactly one artifact",
            profile.id
        ));
    };
    let request = pull::PullRequest {
        repo: profile.source.repo.clone(),
        filename,
        files,
        directory,
        model_type,
        id: Some(profile.id.clone()),
        provider: Some(profile.runner.clone()),
        auto_load: Some(auto_load),
    };
    pull::validate_pull_request(&request)?;
    Ok(request)
}

fn profile_artifact_path(
    models_root: &FilePath,
    model_type: &str,
    profile: &ai_daemon::runners::ModelProfile,
) -> Result<PathBuf, String> {
    if profile.format == "directory" {
        Ok(models_root
            .join(model_type)
            .join(&profile.artifacts.directory))
    } else if profile.artifacts.files.len() == 1 {
        Ok(models_root
            .join(model_type)
            .join(&profile.artifacts.files[0]))
    } else {
        Err(format!(
            "file Profile '{}' must contain exactly one artifact",
            profile.id
        ))
    }
}

async fn pull_model_profile(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<PullModelProfileRequest>,
) -> Response {
    let Some(profile) = state.runtime.runner_profile(&id) else {
        return api_error(
            AIError::ModelNotFound,
            format!("model profile '{id}' not found"),
        );
    };
    let request = match pull_request_for_profile(&profile, body.auto_load) {
        Ok(request) => request,
        Err(error) => return api_error(AIError::InvalidRequest, error),
    };
    pull_model(State(state), Json(request)).await
}

/// Runner 管理面状态：已发现/受信任 Runner + python 环境 phase +
/// bundled Model Profile。UI 只消费 daemon 数据，不自行推断。
async fn runner_statuses(State(state): State<AppState>) -> Json<Value> {
    let Some(manager) = state.runtime.runner_instances() else {
        return Json(json!({ "data": [] }));
    };
    let statuses = manager.environments().statuses();
    let mut data: Vec<Value> = Vec::new();
    for entry in manager.discovered() {
        let Some(manifest) = entry.manifest.clone() else {
            continue;
        };
        if manifest.runtime.runtime_type != "python-uv" {
            continue;
        }
        let environment_id = manifest.runtime.id.clone();
        let environment_phase = statuses
            .iter()
            .find(|status| status.environment_id == environment_id)
            .map(|status| status.phase.as_str().to_string())
            .unwrap_or_else(|| "missing".to_string());
        let engine_binary = manifest.engine.as_ref().and_then(|asset| {
            std::env::var_os("HOME").map(|home| {
                ai_daemon::runners::engine_binary_path(
                    &PathBuf::from(home).join("Library/Application Support/MacAIConsole"),
                    &manifest.id,
                    &asset.binary,
                )
            })
        });
        let engine_ready = engine_binary.as_ref().is_none_or(|path| path.is_file());
        let phase = if environment_phase == "ready" && !engine_ready {
            "missing".to_string()
        } else {
            environment_phase
        };
        let environment = statuses
            .iter()
            .find(|status| status.environment_id == environment_id)
            .cloned();
        let instance = manager.instance_snapshot(&manifest.id).await;
        let models: Vec<Value> = manifest
            .models
            .iter()
            .map(|model| json!({ "profile": format!("{}", model.profile) }))
            .collect();
        let local_detectors: Vec<Value> = manifest
            .local_detectors
            .iter()
            .map(|detector| {
                json!({
                    "id": detector.id,
                    "capability": detector.capability,
                    "adapter": detector.adapter,
                })
            })
            .collect();
        data.push(json!({
            "id": manifest.id,
            "version": manifest.version,
            "root": format!("{}", entry.root.display()),
            "state": if matches!(entry.state, ai_daemon::runners::RunnerState::Trusted) { "trusted" } else { "untrusted" },
            "reason": entry.reason,
            "capabilities": manifest.capabilities,
            "capacity": {
                "max_instances": manifest.capacity.max_instances,
                "max_concurrency_per_instance": manifest.capacity.max_concurrency_per_instance,
            },
            "permissions": {
                "os_scope": "daemon-user",
                "network_during_runtime_declared": manifest.security.network_during_runtime,
                "inherit_environment": manifest.security.inherit_environment,
            },
            "environment_id": environment_id,
            "phase": phase,
            "engine_ready": engine_ready,
            "engine_binary": engine_binary.map(|path| path.display().to_string()),
            "environment": environment,
            "instance": instance,
            "models": models,
            "local_detectors": local_detectors,
        }));
    }
    Json(json!({ "data": data }))
}

/// 显式安装 Runner python 环境（唯一网络/安装入口，daemon 拥有 uv）。幂等：
/// 已 ready 时直接返回 ready。同步等待完成（uv sync 上限 600s）。
async fn install_runner_environment(
    State(state): State<AppState>,
    AxumPath(runner): AxumPath<String>,
) -> Response {
    let Some(manager) = state.runtime.runner_instances() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": { "code": "runner_unavailable", "message": "no Runner assembly" } })),
        )
            .into_response();
    };
    let Some(entry) = manager.discovered().into_iter().find(|entry| {
        entry
            .manifest
            .as_ref()
            .is_some_and(|manifest| manifest.id == runner)
    }) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": { "code": "runner_not_found", "message": format!("runner '{runner}' not discovered") } })),
        )
            .into_response();
    };
    let manifest = entry
        .manifest
        .clone()
        .expect("filtered runner has manifest");
    if manifest.runtime.runtime_type != "python-uv" {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": { "code": "unsupported_runner", "message": "only python-uv runtime supports explicit install" } })),
        )
            .into_response();
    }
    match manager
        .ensure_environment(&manifest, &entry.root)
        .await
    {
        Ok(status) => {
            // 原生引擎产物（cpp 引擎 Runner 化）：环境就绪后下载、校验，
            // manifest 声明 build 时再从 source archive 构建。
            // 失败不掩盖环境安装结果——产物失败在响应中显式报告。
            let engine_result: Option<
                Result<std::path::PathBuf, ai_daemon::runners::EngineInstallError>,
            > = match manifest.engine.as_ref() {
                Some(asset) => {
                    let Some(app_support) = std::env::var_os("HOME").map(|home| {
                        PathBuf::from(home).join("Library/Application Support/MacAIConsole")
                    }) else {
                        return (
                            StatusCode::CONFLICT,
                            Json(json!({ "error": { "code": "engine_asset_failed", "message": "cannot resolve HOME for engine install" } })),
                        )
                            .into_response();
                    };
                    let client = match crate::pull::build_client() {
                        Ok(client) => client,
                        Err(error) => {
                            return (
                                StatusCode::CONFLICT,
                                Json(json!({ "error": { "code": "engine_asset_failed", "message": error } })),
                            )
                                .into_response();
                        }
                    };
                    Some(
                        ai_daemon::runners::ensure_engine_asset(
                            &app_support,
                            &manifest.id,
                            asset,
                            &client,
                        )
                        .await,
                    )
                }
                None => None,
            };
            match engine_result {
                Some(Ok(binary)) => {
                    tracing::info!(runner = %manifest.id, binary = %binary.display(), "engine asset installed");
                    (
                        StatusCode::OK,
                        Json(json!({
                            "environment_id": manifest.runtime.id,
                            "phase": status.phase.as_str(),
                            "engine_binary": binary.display().to_string(),
                        })),
                    )
                        .into_response()
                }
                Some(Err(error)) => (
                    StatusCode::CONFLICT,
                    Json(json!({ "error": { "code": "engine_asset_failed", "message": error.to_string() } })),
                )
                    .into_response(),
                None => (
                    StatusCode::OK,
                    Json(json!({
                        "environment_id": manifest.runtime.id,
                        "phase": status.phase.as_str(),
                    })),
                )
                    .into_response(),
            }
        }
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": { "code": "environment_not_ready", "message": format!("{error:?}") } })),
        )
            .into_response(),
    }
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
            // 下载/TLS/UA 拒绝是传输层故障：映射 DownloadFailed（502）并保留底层
            // connect/response 错误链，供 UI 与日志诊断（不再伪装成内部 500）。
            return api_error(AIError::DownloadFailed, error);
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
        routing_token: None,
        context_length: None,
        keep_alive: None,
    };
    register_and_load_model(State(state), Json(load_request)).await
}

async fn inspect_models(
    State(state): State<AppState>,
    Json(request): Json<InspectModelsRequest>,
) -> Response {
    if request.paths.is_empty() || request.paths.len() > 64 {
        return api_error(AIError::InvalidRequest, "inspect requires 1 to 64 paths");
    }
    let descriptors = state
        .runtime
        .runner_instances()
        .map(|instances| instances.discovered())
        .unwrap_or_default();
    let statuses: HashMap<_, _> = state
        .runtime
        .provider_statuses()
        .await
        .into_iter()
        .map(|(descriptor, status)| (descriptor.id, status.available))
        .collect();
    let mut data = Vec::new();
    for path in request.paths {
        match ai_daemon::runners::inspect_local_directory(FilePath::new(&path), &descriptors) {
            Ok(inspection) => {
                let status = match inspection.matches.len() { 0 => "unsupported", 1 => "recognized", _ => "ambiguous" };
                let token = if inspection.matches.len() == 1 {
                    let matched = inspection.matches[0].clone();
                    let raw = format!("{}:{}:{}:{:?}", inspection.canonical_path, inspection.fingerprint, matched.manifest_digest, std::time::SystemTime::now());
                    let token = format!("{:x}", sha2::Sha256::digest(raw.as_bytes()));
                    state.routing_tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|_, token| token.expires_at > std::time::Instant::now());
                    state.routing_tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(token.clone(), RoutingToken { path: inspection.canonical_path.clone(), fingerprint: inspection.fingerprint.clone(), matched, expires_at: std::time::Instant::now() + Duration::from_secs(300) });
                    Some(token)
                } else { None };
                data.push(json!({
                    "path": path, "canonical_path": inspection.canonical_path, "size_bytes": inspection.size_bytes,
                    "status": status, "matches": inspection.matches, "diagnostics": inspection.diagnostics,
                    "routing_token": token,
                    "runner_available": inspection.matches.first().and_then(|matched| statuses.get(&matched.runner)).copied().unwrap_or(false),
                }));
            }
            Err(error) => data.push(json!({ "path": path, "status": "unsupported", "diagnostics": [error], "matches": [] })),
        }
    }
    Json(json!({ "data": data })).into_response()
}

fn capability_model_type(capability: &str) -> Option<&'static str> {
    match capability {
        "chat.v1" => Some("llm"),
        "stt.v1" => Some("stt"),
        "tts.v1" => Some("tts"),
        _ => None,
    }
}

fn inspected_profile(
    id: &str,
    name: &str,
    matched: &ai_daemon::runners::DetectorMatch,
) -> ai_daemon::runners::ModelProfile {
    ai_daemon::runners::ModelProfile {
        schema: "macai.model.v1".to_string(),
        id: id.to_string(),
        name: name.to_string(),
        capabilities: vec![matched.capability.clone()],
        runner: matched.runner.clone(),
        adapter: matched.adapter.clone(),
        format: "directory".to_string(),
        source: ai_daemon::runners::ProfileSource {
            source_type: "local".to_string(),
            repo: String::new(),
            revision: String::new(),
        },
        artifacts: ai_daemon::runners::ProfileArtifacts {
            directory: id.to_string(),
            files: Vec::new(),
        },
        defaults: ai_daemon::runners::ProfileDefaults::default(),
        resources: ai_daemon::runners::ProfileResources::default(),
        routing: Some(ai_daemon::runners::ProfileRouting {
            detector_id: matched.detector_id.clone(),
            manifest_digest: matched.manifest_digest.clone(),
            reason: matched.reason.clone(),
        }),
        compatibility: ai_daemon::runners::ProfileCompatibility {
            runner: ">=0.1,<2".to_string(),
        },
    }
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
    let inspected = match request.routing_token.as_deref() {
        Some(token) => {
            if request.provider.is_some() || request.model_type.is_some() {
                return api_error(
                    AIError::InvalidRequest,
                    "routing_token cannot be combined with provider or model_type",
                );
            }
            let token = state
                .routing_tokens
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(token);
            let Some(token) = token.filter(|token| {
                token.expires_at > std::time::Instant::now() && token.path == path.to_string_lossy()
            }) else {
                return api_error(
                    AIError::InvalidRequest,
                    "routing token expired or does not match this path; inspect again",
                );
            };
            let descriptors = state
                .runtime
                .runner_instances()
                .map(|instances| instances.discovered())
                .unwrap_or_default();
            let inspection = match ai_daemon::runners::inspect_local_directory(&path, &descriptors)
            {
                Ok(value) => value,
                Err(error) => return api_error(AIError::InvalidRequest, error),
            };
            let matching = inspection.matches.into_iter().find(|matched| {
                matched.detector_id == token.matched.detector_id
                    && matched.runner == token.matched.runner
                    && matched.manifest_digest == token.matched.manifest_digest
            });
            let Some(matched) = matching.filter(|_| inspection.fingerprint == token.fingerprint)
            else {
                return api_error(
                    AIError::InvalidRequest,
                    "model directory or Runner manifest changed; inspect again",
                );
            };
            Some(matched)
        }
        None => None,
    };
    let model_type = match inspected
        .as_ref()
        .and_then(|matched| capability_model_type(&matched.capability))
        .or(request.model_type.as_deref())
    {
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
    let requested_provider_audit = requested_provider.unwrap_or("auto");
    let (provider, provider_selection_reason) = if let Some(matched) = &inspected {
        (
            matched.runner.clone(),
            format!("local detector {}: {}", matched.detector_id, matched.reason),
        )
    } else {
        match select_provider(model_type, requested_provider, |provider, capability| {
            state.runtime.provider_has_capability(provider, capability)
        }) {
            Ok((provider, reason)) => (provider.into_owned(), reason.to_string()),
            Err(message) => return api_error(AIError::InvalidRequest, message),
        }
    };
    let provider = provider.as_ref();
    // 注册形态校验：llm 是 .gguf 文件（Runner 侧聚合成目录），目录型 provider
    // 要求目录。深度内容校验仍由 provider 自己拥有。
    match provider {
        p if p == "org.macai.llama.cpp" && !path.is_file() => {
            return api_error(
                AIError::InvalidRequest,
                format!(
                    "expected a .gguf model file for provider '{p}', got '{}'",
                    path.display()
                ),
            );
        }
        // 两个 cpp Runner 接受单文件；其余 Runner 引擎要求目录型 artifact。
        p if p.starts_with("org.macai.")
            && !matches!(p, "org.macai.llama.cpp" | "org.macai.whisper.cpp")
            && !path.is_dir() =>
        {
            return api_error(
                AIError::InvalidRequest,
                format!(
                    "expected a model directory for provider '{p}', got '{}'",
                    path.display()
                ),
            );
        }
        "org.macai.whisper.cpp"
            if path.extension().and_then(|value| value.to_str()) != Some("bin") =>
        {
            return api_error(
                AIError::InvalidRequest,
                format!(
                    "expected a .bin model for provider 'org.macai.whisper.cpp', got '{}'",
                    path.display()
                ),
            );
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
        // llama.cpp Runner 的本地 GGUF 注册默认驻留 5 分钟。
        "org.macai.llama.cpp" => (Some("gguf"), Some("5m"), Some(size_bytes)),
        "org.macai.whisper.cpp" => (Some("bin"), Some("always"), Some(size_bytes)),
        _ => (None, Some("always"), Some(size_bytes)),
    };
    let spec = ai_core::model::ModelSpec {
        id: id.clone(),
        name: request.name.unwrap_or_else(|| id.clone()),
        model_type: model_type.to_string(),
        provider: provider.to_string(),
        requested_provider: Some(requested_provider_audit.to_string()),
        provider_selection_reason: Some(provider_selection_reason.to_string()),
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

    // ad-hoc 绑定：无 catalog Profile 的 Runner（如 llama.cpp）在注册路径上
    // 建立内存绑定。adapter 名来自 manifest 的 default_adapter；重复注册幂等。
    if inspected.is_none() && provider.starts_with("org.macai.") {
        let manifest = state.runtime.runner_instances().and_then(|manager| {
            manager
                .discovered()
                .into_iter()
                .find(|entry| {
                    entry
                        .manifest
                        .as_ref()
                        .is_some_and(|manifest| manifest.id == provider)
                })
                .and_then(|entry| entry.manifest.clone())
        });
        if let Some(adapter) = manifest
            .as_ref()
            .and_then(|m| m.runtime.default_adapter.as_deref())
        {
            if let Err(error) = state
                .runtime
                .bind_adhoc_runner_model(
                    provider,
                    &id,
                    &path,
                    adapter,
                    format.unwrap_or("directory"),
                )
                .await
            {
                return api_error(AIError::ProviderUnavailable, error);
            }
        }
    }

    let result = match inspected {
        Some(matched) => {
            state
                .runtime
                .register_and_load_inspected(
                    spec.clone(),
                    inspected_profile(&id, &spec.name, &matched),
                )
                .await
        }
        None => state.runtime.register_and_load(spec).await,
    };
    match result {
        Ok(handle) => Json(json!({
            "id": handle.model_id,
            "provider": handle.provider_id,
            "requested_provider": requested_provider_audit,
            "provider_selection_reason": provider_selection_reason,
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
    // Qwen3-TTS CustomVoice 的内置 speaker 清单（引擎语义，无 voices/ 目录文件）。
    const QWEN3_TTS_VOICES: &[&str] = &[
        "Vivian", "Serena", "Uncle_Fu", "Dylan", "Eric", "Ryan", "Aiden", "Ono_Anna", "Sohee",
    ];
    let (voices, default_voice) = if spec.provider == "org.macai.qwen3-tts" {
        (
            QWEN3_TTS_VOICES
                .iter()
                .map(|voice| (*voice).to_string())
                .collect(),
            spec.default_voice
                .clone()
                .filter(|voice| QWEN3_TTS_VOICES.contains(&voice.as_str()))
                .or_else(|| Some(QWEN3_TTS_VOICES.first().unwrap().to_string())),
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
        // 未指定模型时自动选第一个已注册的 TTS 模型（如 Runner 版 Kokoro）；
        // 没有任何 TTS 模型时明确报错，不再指向已删除的 legacy provider id。
        let default_tts = state
            .runtime
            .list_models()
            .await
            .into_iter()
            .find(|entry| entry.spec.model_type == "tts")
            .map(|entry| entry.spec.id);
        let Some(default_tts) = default_tts else {
            return api_error(AIError::ModelNotFound, "no TTS model registered");
        };
        request.model = default_tts;
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

/// Provider 裁决的单一入口。返回最终 provider 与可持久化的审计原因；显式选择
/// 始终经过当前 descriptor 的 capability 校验。
fn select_provider<'a>(
    model_type: &str,
    requested: Option<&'a str>,
    supports: impl Fn(&str, ai_core::provider::Capability) -> bool,
) -> Result<(std::borrow::Cow<'a, str>, &'static str), String> {
    match (model_type, requested) {
        ("llm", None) => Ok((
            "org.macai.llama.cpp".into(),
            "default provider for model type 'llm'",
        )),
        ("llm", Some("llama.cpp")) => Ok((
            "org.macai.llama.cpp".into(),
            "legacy provider alias 'llama.cpp' resolved to Runner",
        )),
        ("llm", Some("org.macai.llama.cpp")) => {
            Ok(("org.macai.llama.cpp".into(), "explicit provider selection"))
        }
        ("stt", None) => Ok((
            "org.macai.whisper.cpp".into(),
            "default provider for model type 'stt'",
        )),
        ("stt", Some("whisper.cpp")) => Ok((
            "org.macai.whisper.cpp".into(),
            "legacy provider alias 'whisper.cpp' resolved to Runner",
        )),
        (other_type, None) => Err(format!(
            "no default provider is defined for model type '{other_type}'"
        )),
        (_, Some(provider)) => {
            let capability = match model_type {
                "tts" => ai_core::provider::Capability::TextToSpeech,
                "stt" => ai_core::provider::Capability::SpeechToText,
                _ => ai_core::provider::Capability::Chat,
            };
            if !supports(provider, capability) {
                return Err(format!(
                    "provider '{provider}' does not support model type '{model_type}'"
                ));
            }
            Ok((provider.to_string().into(), "explicit provider selection"))
        }
    }
}

fn provider_error(error: ProviderError) -> Response {
    api_error(error.kind, error.message)
}

fn api_error(error: AIError, message: impl Into<String>) -> Response {
    let status =
        StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ApiErrorBody::new(error, message))).into_response()
}

/// 装配仓库随附 built-in Runner。
///
/// 流程：定位 Runner 目录（`MACAI_RUNNERS_DIR` → cwd `runners`/`../runners`）
/// → discovery（built-in root 直接信任）→ 逐 manifest 读取 bundled Model
/// Profile → 持久化 profile snapshot（models.db）→ 对已有模型 artifact 的
/// profile 建立 RunnerProvider 绑定并 `attach_runner` 进 Runtime。
///
/// 找不到 Runner 目录、无 python-uv Runner 或模型目录缺失时静默跳过，保留纯
/// 内置 Provider 路径；模型目录缺失表示用户尚未下载 artifact，不假装 ready。
/// Plugins 目录与显式信任留到安装 slice。
/// 定位 built-in Runner 目录：`MACAI_RUNNERS_DIR` → cwd `runners`/`../runners`
/// → daemon 可执行文件祖先中的仓库 `runners/`。最后一条让 Finder/launchd
/// 启动（cwd 不可靠）也能发现仓库随附 Runner。
fn builtin_runners_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MACAI_RUNNERS_DIR") {
        let path = PathBuf::from(dir);
        if path.is_dir() {
            return Some(path);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for candidate in [cwd.join("runners"), cwd.join("../runners")] {
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    for ancestor in exe_dir.ancestors().take(6) {
        let candidate = ancestor.join("runners");
        if candidate.join("kokoro").join("runner.toml").is_file() {
            return Some(candidate);
        }
    }
    None
}

async fn bootstrap_runners(runtime: &mut Runtime) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let app_support = PathBuf::from(&home).join("Library/Application Support/MacAIConsole");
    let Some(runner_root) = builtin_runners_root() else {
        tracing::info!("no built-in runners dir; Runner assembly skipped");
        return;
    };
    if !runner_root.is_dir() {
        tracing::info!(path = %runner_root.display(), "built-in runners dir unavailable");
        return;
    }

    bootstrap_runners_from_root(runtime, runner_root, app_support).await;
}

/// 从已经解析出的 built-in root 装配 Runner。拆出该边界后，首次安装测试可以使用
/// 隔离目录验证“模型 artifact 尚不存在时 Provider 仍已 attach”。
async fn bootstrap_runners_from_root(
    runtime: &mut Runtime,
    runner_root: PathBuf,
    app_support: PathBuf,
) {
    use std::collections::HashMap;
    use std::collections::HashSet;

    use ai_daemon::runners::{
        EnvironmentManager, EnvironmentManagerConfig, ModelProfile, RunnerInstanceManager,
        RunnerModelBinding, RunnerProvider, RunnerRegistry, RunnerState,
    };

    let registry = RunnerRegistry::discover(&[runner_root.clone()], &[], &HashSet::new());
    // 诊断：打印非可用/非 trusted 条目及其原因，便于定位 manifest/校验拒绝点。
    for entry in registry.entries() {
        let id = entry
            .manifest
            .as_ref()
            .map(|manifest| manifest.id.as_str())
            .unwrap_or("<no manifest>");
        let state = match entry.state {
            ai_daemon::runners::RunnerState::Trusted => "trusted",
            ai_daemon::runners::RunnerState::Untrusted => "untrusted",
            _ => "unavailable",
        };
        if state != "trusted" {
            tracing::warn!(
                runner = %id,
                path = %entry.root.display(),
                state,
                reason = ?entry.reason,
                "runner discovery rejected"
            );
        }
    }
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

    let environments = EnvironmentManager::new(EnvironmentManagerConfig {
        runtime_root: app_support.join("Runtimes/python"),
    });
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
        let provider = Arc::new(RunnerProvider::new(
            runner_id.clone(),
            &manifest.capabilities,
            instances.clone(),
            temp_root.clone(),
        ));
        providers.insert(runner_id.clone(), provider.clone());
        for model in &manifest.models {
            let catalog_profile = match ModelProfile::load(&entry.root.join(&model.profile)) {
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
            match runtime.register_runner_profile(&catalog_profile) {
                Ok(Some(digest)) => {
                    tracing::info!(profile = %catalog_profile.id, digest = %digest, "runner profile persisted")
                }
                Ok(None) => {
                    tracing::info!(profile = %catalog_profile.id, "runner profile registered (memory mode)")
                }
                Err(error) => {
                    tracing::warn!(profile = %catalog_profile.id, %error, "profile persist failed")
                }
            }
            if let Ok(true) = runtime
                .freeze_existing_runner_profile(&catalog_profile)
                .await
            {
                tracing::info!(
                    profile = %catalog_profile.id,
                    "froze current Profile for a pre-snapshot model registration"
                );
            }
            // 已注册模型以 per-model snapshot 为准；catalog 更新只影响未来注册。
            let profile = runtime
                .registered_runner_profile(&catalog_profile.id)
                .await
                .unwrap_or(catalog_profile);
            let kind = if profile.capabilities.iter().any(|cap| cap == "stt.v1") {
                "stt"
            } else if profile.capabilities.iter().any(|cap| cap == "tts.v1") {
                "tts"
            } else {
                "llm"
            };
            let model_dir = match profile_artifact_path(&models_root, kind, &profile) {
                Ok(path) => path,
                Err(error) => {
                    tracing::warn!(profile = %profile.id, %error, "skip invalid Profile artifact path");
                    continue;
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
                artifact_present = model_dir.is_dir(),
                "runner model bound"
            );
        }
        if let Some(adapter) = manifest.runtime.default_adapter.as_deref() {
            for registered in runtime
                .list_models()
                .await
                .into_iter()
                .filter(|entry| entry.profile.is_none() && entry.spec.provider == runner_id)
            {
                let Some(path) = registered.spec.path.as_deref() else {
                    continue;
                };
                let format = registered.spec.format.as_deref().unwrap_or("directory");
                if let Err(error) = provider
                    .bind_adhoc_model(
                        &registered.spec.id,
                        std::path::Path::new(path),
                        adapter,
                        format,
                    )
                    .await
                {
                    tracing::warn!(
                        runner = %runner_id,
                        model = %registered.spec.id,
                        %error,
                        "cannot restore ad-hoc Runner binding"
                    );
                }
            }
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

    fn future_profile() -> ai_daemon::runners::ModelProfile {
        ai_daemon::runners::ModelProfile::parse(
            r#"
schema = "macai.model.v1"
id = "future-model"
name = "Future Model"
capabilities = ["stt.v1"]
runner = "org.example.future"
adapter = "future"
format = "directory"
[source]
type = "huggingface"
repo = "owner/future"
revision = "0123456789abcdef0123456789abcdef01234567"
[artifacts]
directory = "future-model"
files = ["config.json", "model.bin"]
[resources]
memory_estimate_bytes = 123
[compatibility]
runner = ">=1,<2"
"#,
        )
        .unwrap()
    }

    #[test]
    fn profile_catalog_and_pull_are_derived_from_daemon_profile() {
        let profile = future_profile();
        let json = model_profile_json(&profile).unwrap();
        assert_eq!(json["runner"], "org.example.future");
        assert_eq!(json["model_type"], "stt");

        let request = pull_request_for_profile(&profile, false).unwrap();
        assert_eq!(request.repo, "owner/future");
        assert_eq!(request.provider.as_deref(), Some("org.example.future"));
        assert_eq!(request.files, ["config.json", "model.bin"]);
        assert_eq!(request.auto_load, Some(false));
    }

    #[test]
    fn file_profile_uses_its_single_artifact_as_runtime_path() {
        let profile_path = FilePath::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../runners/whisper.cpp/profiles/whisper-large-v3-turbo-q5.toml");
        let profile = ai_daemon::runners::ModelProfile::load(&profile_path).unwrap();
        assert_eq!(
            profile_artifact_path(FilePath::new("/Models"), "stt", &profile).unwrap(),
            PathBuf::from("/Models/stt/ggml-large-v3-turbo-q5_0.bin")
        );
    }

    #[test]
    fn validates_model_aliases() {
        assert!(valid_model_id("SmolLM2-135M.Q4_K_M"));
        assert!(!valid_model_id("../model"));
        assert!(!valid_model_id("model name"));
    }

    #[test]
    fn provider_selection_records_default_alias_and_explicit_reasons() {
        let default = select_provider("llm", None, |_, _| false).unwrap();
        assert_eq!(default.0, "org.macai.llama.cpp");
        assert_eq!(default.1, "default provider for model type 'llm'");

        let alias = select_provider("llm", Some("llama.cpp"), |_, _| false).unwrap();
        assert_eq!(alias.0, "org.macai.llama.cpp");
        assert_eq!(
            alias.1,
            "legacy provider alias 'llama.cpp' resolved to Runner"
        );

        let explicit = select_provider("tts", Some("org.macai.kokoro"), |id, capability| {
            id == "org.macai.kokoro" && capability == ai_core::provider::Capability::TextToSpeech
        })
        .unwrap();
        assert_eq!(explicit.0, "org.macai.kokoro");
        assert_eq!(explicit.1, "explicit provider selection");

        let stt_default = select_provider("stt", None, |_, _| false).unwrap();
        assert_eq!(stt_default.0, "org.macai.whisper.cpp");
        assert_eq!(stt_default.1, "default provider for model type 'stt'");
        let stt_alias = select_provider("stt", Some("whisper.cpp"), |_, _| false).unwrap();
        assert_eq!(stt_alias.0, "org.macai.whisper.cpp");
        assert!(stt_alias.1.contains("legacy provider alias"));
        assert!(
            select_provider("tts", Some("org.macai.mlx-lm"), |_, _| false)
                .unwrap_err()
                .contains("does not support")
        );
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
            requested_provider: Some("mock".to_string()),
            provider_selection_reason: Some("test provider selection".to_string()),
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
            routing_tokens: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    #[tokio::test]
    async fn model_list_exposes_provider_selection_audit_fields() {
        let runtime = Arc::new(Runtime::new());
        runtime.register(mock_model()).await;
        let Json(body) = list_models(State(app_state(runtime))).await;
        let model = &body["data"][0];
        assert_eq!(model["requested_provider"], "mock");
        assert_eq!(
            model["provider_selection_reason"],
            "test provider selection"
        );
        assert_eq!(model["owned_by"], "aiworkd/mock");
    }

    fn qwen_tts_model() -> ai_core::model::ModelSpec {
        ai_core::model::ModelSpec {
            id: "qwen-tts".to_string(),
            name: "Qwen3-TTS".to_string(),
            model_type: "tts".to_string(),
            provider: "org.macai.qwen3-tts".to_string(),
            requested_provider: Some("org.macai.qwen3-tts".to_string()),
            provider_selection_reason: Some("test provider selection".to_string()),
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
    async fn runner_statuses_report_empty_without_assembly() {
        let runtime = Arc::new(Runtime::new());
        let state = app_state(runtime);
        let Json(body) = runner_statuses(State(state.clone())).await;
        assert_eq!(body["data"], serde_json::json!([]));

        let unknown =
            install_runner_environment(State(state), AxumPath("org.missing.runner".to_string()))
                .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn builtin_runner_attaches_before_its_model_artifact_exists() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "macai-bootstrap-runner-{}-{unique}",
            std::process::id()
        ));
        let package = root.join("runners/example");
        std::fs::create_dir_all(package.join("profiles")).unwrap();
        std::fs::write(
            package.join("runner.toml"),
            r#"schema = "macai.runner.v1"
id = "org.example.fresh"
version = "0.1.0"
protocols = ["macai.runner.v1"]
capabilities = ["tts.v1"]
[entrypoint]
command = ["runner"]
working_directory = "package"
[runtime]
type = "python-uv"
id = "org.example.fresh-python"
project = "."
lock = "uv.lock"
python = ">=3.12,<3.13"
probe = ["{environment.python}", "-c", "print('ok')"]
default_adapter = "fresh-ad-hoc"
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
profile = "profiles/fresh.toml"
adapter = "fresh"
"#,
        )
        .unwrap();
        std::fs::write(
            package.join("profiles/fresh.toml"),
            r#"schema = "macai.model.v1"
id = "fresh-model"
name = "Fresh model"
capabilities = ["tts.v1"]
runner = "org.example.fresh"
adapter = "fresh"
format = "directory"
[source]
type = "huggingface"
repo = "org/fresh"
revision = "0123456789abcdef0123456789abcdef01234567"
[artifacts]
directory = "fresh-model"
files = ["model.safetensors"]
[compatibility]
runner = ">=0.1,<0.2"
"#,
        )
        .unwrap();
        std::fs::write(
            package.join("pyproject.toml"),
            "[project]\nname='fresh'\nversion='0.1.0'\nrequires-python='>=3.12'\n",
        )
        .unwrap();
        std::fs::write(
            package.join("uv.lock"),
            "version = 1\nrevision = 3\nrequires-python = '>=3.12'\n",
        )
        .unwrap();

        let app_support = root.join("Application Support/MacAIConsole");
        let mut runtime = Runtime::new();
        let ad_hoc_model = root.join("existing.bin");
        std::fs::write(&ad_hoc_model, b"model").unwrap();
        runtime
            .register(ai_core::model::ModelSpec {
                id: "fresh-ad-hoc".to_string(),
                name: "Fresh ad-hoc".to_string(),
                model_type: "tts".to_string(),
                provider: "org.example.fresh".to_string(),
                requested_provider: Some("org.example.fresh".to_string()),
                provider_selection_reason: Some("test restart restore".to_string()),
                source: None,
                path: Some(ad_hoc_model.display().to_string()),
                format: Some("bin".to_string()),
                size_bytes: Some(5),
                memory_estimate: Some(5),
                keep_alive: Some("always".to_string()),
                context_length: None,
                default_voice: None,
            })
            .await;
        bootstrap_runners_from_root(&mut runtime, root.join("runners"), app_support.clone()).await;

        assert!(runtime.provider_has_capability(
            "org.example.fresh",
            ai_core::provider::Capability::TextToSpeech
        ));
        assert!(runtime.runner_instances().is_some());
        assert!(!app_support.join("Models/tts/fresh-model").exists());
        let runtime = Arc::new(runtime);
        let Json(body) = runner_statuses(State(app_state(runtime.clone()))).await;
        assert_eq!(body["data"][0]["id"], "org.example.fresh");
        assert_eq!(body["data"][0]["capabilities"], json!(["tts.v1"]));
        assert_eq!(body["data"][0]["instance"], Value::Null);
        // bootstrap 必须从已注册 ModelSpec 恢复 ad-hoc binding。删除 artifact 后
        // load 应命中该 binding 的 artifact 校验，而非报告 "not bound"。
        std::fs::remove_file(&ad_hoc_model).unwrap();
        let error = runtime.load_model("fresh-ad-hoc").await.unwrap_err();
        assert!(error.message.contains("artifact"), "{error}");
        assert!(!error.message.contains("not bound"), "{error}");
        runtime.shutdown_all().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn repo_builtin_runner_stays_trusted_with_dev_venv_present() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runners");
        let registry = ai_daemon::runners::RunnerRegistry::discover(
            &[root.clone()],
            &[],
            &std::collections::HashSet::new(),
        );
        // 回归：live repo 里包内 `.venv`（Phase 2 起 uv sync 生成，含 symlink）
        // 必须被排除在 package digest 之外，builtin discovery 保持 Trusted。
        let kokoro = registry
            .entries()
            .iter()
            .find(|entry| {
                entry
                    .manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.id == "org.macai.kokoro")
            })
            .expect("repo built-in kokoro runner must be discovered");
        assert_eq!(kokoro.state, ai_daemon::runners::RunnerState::Trusted);
        assert_eq!(kokoro.reason, None);
        assert_eq!(
            kokoro
                .manifest
                .as_ref()
                .map(|m| m.runtime.runtime_type.as_str()),
            Some("python-uv")
        );

        // chat.v1 Runner（LLM 迁移）与既有 Runner 同一 discovery 契约。
        let mlx_lm = registry
            .entries()
            .iter()
            .find(|entry| {
                entry
                    .manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.id == "org.macai.mlx-lm")
            })
            .expect("repo built-in mlx-lm runner must be discovered");
        assert_eq!(mlx_lm.state, ai_daemon::runners::RunnerState::Trusted);
        assert_eq!(mlx_lm.reason, None);

        let whisper = registry
            .entries()
            .iter()
            .find(|entry| {
                entry
                    .manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.id == "org.macai.whisper.cpp")
            })
            .expect("repo built-in whisper runner must be discovered");
        assert_eq!(whisper.state, ai_daemon::runners::RunnerState::Trusted);
        assert_eq!(whisper.reason, None);
        let engine = whisper
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.engine.as_ref())
            .expect("whisper Runner must declare its managed engine");
        assert!(engine.build.is_some(), "whisper engine comes from source");
    }

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
