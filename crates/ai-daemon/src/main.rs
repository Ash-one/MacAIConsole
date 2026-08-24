//! aiworkd — macOS Local AI Runtime daemon。
//!
//! 对外提供 OpenAI-compatible chat API；管理面提供 Provider 状态与模型
//! load/unload。默认只绑定 127.0.0.1:11435。

mod providers;
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
use axum::routing::{get, post};
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
        .route("/api/models/{id}/load", post(load_registered_model))
        .route("/api/models/{id}/unload", post(unload_model))
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .with_state(state);

    let addr = "127.0.0.1:11435";
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");

    tracing::info!(%addr, "aiworkd listening");
    println!("AI Runtime running at http://{addr}");
    axum::serve(listener, app).await.expect("server error");
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
