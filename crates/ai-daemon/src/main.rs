//! aiworkd — macOS Local AI Runtime daemon。
//!
//! 对外提供 OpenAI-compatible chat API；管理面提供 Provider 状态与模型
//! load/unload。默认只绑定 127.0.0.1:11435。

mod providers;
mod runtime;

use std::path::Path as FilePath;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use ai_core::errors::{AIError, ApiErrorBody};
use ai_core::provider::ProviderError;
use ai_core::request::ChatRequest;
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
    context_length: Option<u64>,
    keep_alive: Option<String>,
}

#[tokio::main]
async fn main() {
    init_tracing();

    let runtime = Arc::new(Runtime::new());
    runtime.register(mock_model()).await;

    let state = AppState {
        runtime: Arc::clone(&runtime),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/api/runtime", get(runtime_info))
        .route("/api/providers", get(provider_statuses))
        .route("/api/models/load", post(register_and_load_model))
        .route("/api/models/{id}/load", post(load_registered_model))
        .route("/api/models/{id}/unload", post(unload_model))
        .with_state(state);

    let addr = "127.0.0.1:11435";
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");

    tracing::info!(%addr, "aiworkd listening");
    println!("AI Runtime running at http://{addr}");
    axum::serve(listener, app).await.expect("server error");
}

fn mock_model() -> ai_core::model::ModelSpec {
    ai_core::model::ModelSpec {
        id: "mock".to_string(),
        name: "Mock Model (echo)".to_string(),
        model_type: "llm".to_string(),
        provider: "mock".to_string(),
        source: None,
        path: None,
        format: Some("mock".to_string()),
        size_bytes: None,
        memory_estimate: Some(0),
        keep_alive: Some("always".to_string()),
        context_length: Some(4096),
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
    if path.extension().and_then(|value| value.to_str()) != Some("gguf") {
        return api_error(
            AIError::InvalidRequest,
            format!("expected a .gguf model, got '{}'", path.display()),
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
    let spec = ai_core::model::ModelSpec {
        id: id.clone(),
        name: request.name.unwrap_or_else(|| id.clone()),
        model_type: "llm".to_string(),
        provider: "llama.cpp".to_string(),
        source: None,
        path: Some(path.to_string_lossy().into_owned()),
        format: Some("gguf".to_string()),
        size_bytes: Some(metadata.len()),
        memory_estimate: Some(metadata.len()),
        keep_alive: request.keep_alive.or_else(|| Some("5m".to_string())),
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
