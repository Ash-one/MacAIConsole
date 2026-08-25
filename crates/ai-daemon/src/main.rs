//! aiworkd — macOS Local AI Runtime daemon。
//!
//! 对外提供 OpenAI-compatible chat API；管理面提供 Provider 状态与模型
//! load/unload。默认只绑定 127.0.0.1:11435。

mod providers;
mod process_memory;
mod pull;
mod registry;
mod runtime;
mod scheduler;

use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use ai_core::errors::{AIError, ApiErrorBody};
use ai_core::provider::ProviderError;
use ai_core::request::{ChatRequest, SpeechRequest, TranscriptionRequest};
use ai_core::response::ModelEntry;

use crate::runtime::Runtime;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
struct AppState {
    runtime: Arc<Runtime>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
    model_count: usize,
}

#[derive(Debug, Deserialize)]
struct LoadModelRequest {
    path: String,
    id: Option<String>,
    name: Option<String>,
    /// llm（默认）/ stt。llm → llama.cpp (.gguf)，stt → whisper.cpp (.bin)。
    model_type: Option<String>,
    context_length: Option<u64>,
    keep_alive: Option<String>,
}

#[tokio::main]
async fn main() {
    init_tracing();

    // 注册表数据库与 GUI 模型仓库同根（~/Library/Application Support/MacAIConsole）。
    let db_path = std::env::var("HOME")
        .map(|home| {
            std::path::PathBuf::from(home)
                .join("Library/Application Support/MacAIConsole/models.db")
        })
        .unwrap_or_else(|_| std::path::PathBuf::from("models.db"));
    let runtime = Arc::new(Runtime::with_store(&db_path));
    runtime.spawn_idle_reaper();

    let state = AppState {
        runtime: Arc::clone(&runtime),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/audio/transcriptions", post(audio_transcriptions))
        .route("/v1/audio/speech", post(audio_speech))
        .route("/api/runtime", get(runtime_info))
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

    let addr = "127.0.0.1:11435";
    let listener = tokio::net::TcpListener::bind(addr)
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

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: VERSION.to_string(),
        model_count: state.runtime.list_models().await.len(),
    })
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

/// POST /api/models/pull —— 下载 HF 单文件到模型仓库并注册加载（handoff §20）。
async fn pull_model(
    State(state): State<AppState>,
    Json(request): Json<pull::PullRequest>,
) -> Response {
    if let Err(error) = pull::validate_pull_parts(&request.repo, &request.filename) {
        return api_error(AIError::InvalidRequest, error);
    }
    if !matches!(request.model_type.as_str(), "llm" | "stt" | "tts") {
        return api_error(
            AIError::InvalidRequest,
            format!("unsupported model_type '{}'", request.model_type),
        );
    }
    let (dest, part) = pull::pull_target(&request.model_type, &request.filename);
    if let Some(parent) = dest.parent() {
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            return api_error(
                AIError::Internal,
                format!("cannot create model dir {}: {error}", parent.display()),
            );
        }
    }

    // HEAD 拿总大小；失败不阻塞下载（只是少了续传与完整校验）。
    let url = pull::resolve_url(&request.repo, &request.filename);
    let head_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok();
    let expected_len: Option<u64> = match head_client.as_ref() {
        Some(client) => match client.head(&url).send().await {
            Ok(resp) => resp
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok()),
            Err(_) => None,
        },
        None => None,
    };

    // 目标已完整存在：幂等跳过下载。
    if dest.exists() {
        if let Some(expected) = expected_len {
            if std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0) == expected {
                tracing::info!(dest = %dest.display(), "pull target already complete");
                return finish_pull(state, request, dest).await;
            }
        }
    }

    // 断点续传：从 .part 已有字节处继续。
    let offset = part
        .exists()
        .then(|| std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0))
        .unwrap_or(0);
    let mut client_builder = reqwest::Client::builder();
    if head_client.is_some() {
        client_builder = client_builder.pool_idle_timeout(std::time::Duration::from_secs(90));
    }
    let download_client = match client_builder.build() {
        Ok(client) => client,
        Err(error) => return api_error(AIError::Internal, format!("http client: {error}")),
    };
    let mut get = download_client.get(&url);
    if offset > 0 && expected_len.is_some() {
        get = get.header(header::RANGE, format!("bytes={offset}-"));
        tracing::info!(offset, "resuming pull");
    } else if offset > 0 {
        // 无期望大小时无法确认服务器支持 Range，保守从头下。
        let _ = tokio::fs::remove_file(&part).await;
    }

    let response = match get.send().await {
        Ok(response) => response,
        Err(error) => return api_error(AIError::Internal, format!("download failed: {error}")),
    };
    if !response.status().is_success() {
        return api_error(
            AIError::ModelNotFound,
            format!("HF returned {} for {url}", response.status()),
        );
    }
    let total: Option<u64> = response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(|len| len + offset);

    let mut file = match (if offset > 0 && response.status() == StatusCode::PARTIAL_CONTENT {
        tokio::fs::OpenOptions::new().append(true).open(&part).await
    } else {
        let _ = tokio::fs::remove_file(&part).await;
        tokio::fs::File::create(&part).await
    }) {
        Ok(file) => file,
        Err(error) => return api_error(AIError::Internal, format!("cannot open .part: {error}")),
    };

    let mut stream = response.bytes_stream();
    let mut downloaded = offset;
    let mut last_report = downloaded;
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                if let Err(error) = file.write_all(&bytes).await {
                    return api_error(AIError::Internal, format!("write failed: {error}"));
                }
                downloaded += bytes.len() as u64;
                // 每 16MB 打一条进度日志，避免刷屏。
                if downloaded - last_report >= 16 * 1024 * 1024 {
                    tracing::info!(downloaded, total = total.unwrap_or(0), "pull progress");
                    last_report = downloaded;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "pull interrupted; .part kept for resume");
                return api_error(
                    AIError::Internal,
                    format!("download interrupted at {downloaded} bytes: {error}"),
                );
            }
        }
    }
    if let Some(total) = total {
        if downloaded != total {
            return api_error(
                AIError::Internal,
                format!("size mismatch: got {downloaded}, expected {total}"),
            );
        }
    }
    if let Err(error) = tokio::fs::rename(&part, &dest).await {
        return api_error(AIError::Internal, format!("rename failed: {error}"));
    }
    tracing::info!(dest = %dest.display(), bytes = downloaded, "pull complete");

    finish_pull(state, request, dest).await
}

/// 下载完成后的公共尾部：按类型注册并加载。
async fn finish_pull(state: AppState, request: pull::PullRequest, dest: PathBuf) -> Response {
    let id = request.id.clone().unwrap_or_else(|| {
        dest.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("pulled-model")
            .to_string()
    });
    let load_request = LoadModelRequest {
        path: dest.to_string_lossy().into_owned(),
        id: Some(id),
        name: None,
        model_type: Some(request.model_type.clone()),
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
    // 类型路由：stt → whisper.cpp (.bin)，tts → kokoro-mlx（模型目录），默认 llm → llama.cpp (.gguf)。
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
    let expected_ext = match model_type {
        "llm" => Some("gguf"),
        "stt" => Some("bin"),
        _ => None,
    };
    if let Some(expected_ext) = expected_ext {
        if path.extension().and_then(|value| value.to_str()) != Some(expected_ext) {
            return api_error(
                AIError::InvalidRequest,
                format!(
                    "expected a .{expected_ext} model for type '{model_type}', got '{}'",
                    path.display()
                ),
            );
        }
    }
    if model_type == "tts" && !path.is_dir() {
        return api_error(
            AIError::InvalidRequest,
            format!(
                "expected a model directory containing model.safetensors for type 'tts', got '{}'",
                path.display()
            ),
        );
    }

    let id = request.id.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("model")
            .to_string()
    });
    if !valid_model_id(&id) {
        return api_error(
            AIError::InvalidRequest,
            "model id may contain only letters, numbers, '.', '_' and '-'",
        );
    }
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            return api_error(
                AIError::ModelNotFound,
                format!("cannot inspect model '{}': {error}", path.display()),
            )
        }
    };
    let (provider, format, keep_alive_default) = match model_type {
        "llm" => ("llama.cpp", Some("gguf"), Some("5m")),
        "stt" => ("whisper.cpp", Some("bin"), Some("always")),
        _ => ("kokoro-mlx", None, Some("always")),
    };
    let spec = ai_core::model::ModelSpec {
        id: id.clone(),
        name: request.name.unwrap_or_else(|| id.clone()),
        model_type: model_type.to_string(),
        provider: provider.to_string(),
        source: None,
        path: Some(path.to_string_lossy().into_owned()),
        format: format.map(String::from),
        size_bytes: Some(metadata.len()),
        memory_estimate: Some(metadata.len()),
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
async fn unregister_model(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
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
    let keep_alive = req.keep_alive.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
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

/// 列出 TTS 模型可用的音色：扫描模型目录下 voices/*.safetensors。
async fn list_model_voices(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    let Some(spec) = state.runtime.get_model(&id).await else {
        return api_error(AIError::ModelNotFound, format!("model '{id}' not found"));
    };
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
    Json(json!({"id": id, "voices": voices, "default_voice": spec.default_voice}))
        .into_response()
}

async fn chat_completions(State(state): State<AppState>, Json(req): Json<ChatRequest>) -> Response {
    let request_id = state.runtime.next_request_id();
    tracing::info!(%request_id, model = %req.model, stream = req.stream, "chat request");

    let model_id = req.model.clone();
    let model_lease = state.runtime.model_lease(&model_id);
    let provider = match state.runtime.chat_provider(&model_id).await {
        Ok(provider) => provider,
        Err(error) => return provider_error(error),
    };
    state.runtime.touch_model(&model_id).await;
    let request_guard = state.runtime.request_guard();

    if req.stream {
        match provider.chat_stream(req).await {
            Ok(stream) => {
                let chunks = stream.map(move |result| {
                    let _request_guard = &request_guard;
                    let _model_lease = &model_lease;
                    let event = match result {
                        Ok(chunk) => Event::default().json_data(chunk).unwrap(),
                        Err(error) => Event::default()
                            .event("error")
                            .json_data(ApiErrorBody::new(error.kind, error.message))
                            .unwrap(),
                    };
                    Ok::<_, std::convert::Infallible>(event)
                });
                let done = futures::stream::once(async {
                    Ok::<_, std::convert::Infallible>(Event::default().data("[DONE]"))
                });
                let sse = Sse::new(chunks.chain(done))
                    .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)));
                (StatusCode::OK, sse).into_response()
            }
            Err(error) => provider_error(error),
        }
    } else {
        match provider.chat(req).await {
            Ok(response) => Json(response).into_response(),
            Err(error) => provider_error(error),
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
                if FilePath::new(&file_name)
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(|value| value.eq_ignore_ascii_case("wav"))
                    != Some(true)
                {
                    return api_error(
                        AIError::InvalidRequest,
                        "whisper.cpp STT currently accepts .wav uploads",
                    );
                }
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
                let uploaded = UploadedAudio::new(state.runtime.next_request_id());
                if let Err(error) = tokio::fs::write(&uploaded.path, &bytes).await {
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
    match state.runtime.transcribe(request).await {
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
    match state.runtime.synthesize(request).await {
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
}

impl UploadedAudio {
    fn new(request_id: String) -> Self {
        Self {
            path: std::env::temp_dir().join(format!("macai-upload-{request_id}.wav")),
        }
    }
}

impl Drop for UploadedAudio {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_model_aliases() {
        assert!(valid_model_id("SmolLM2-135M.Q4_K_M"));
        assert!(!valid_model_id("../model"));
        assert!(!valid_model_id("model name"));
    }
}
