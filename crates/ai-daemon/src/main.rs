//! aiworkd — Local AI Runtime daemon（文档 §4.2、§13）。
//!
//! 原型阶段实现 OpenAI-compatible API：
//!   GET  /health
//!   GET  /v1/models
//!   POST /v1/chat/completions   （含 SSE streaming）
//!   GET  /api/runtime
//!
//! 默认监听 127.0.0.1:11435（文档 §13、§43：默认不得绑定 0.0.0.0）。

mod providers;
mod runtime;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};

use ai_core::errors::{AIError, ApiErrorBody};
use ai_core::provider::{ChatProvider, Provider};
use ai_core::request::ChatRequest;
use ai_core::response::ModelEntry;

use crate::runtime::Runtime;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 共享状态。
#[derive(Clone)]
struct AppState {
    runtime: Arc<Runtime>,
}

/// 简单的健康检查响应。
#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
    model_count: usize,
}

#[tokio::main]
async fn main() {
    init_tracing();

    let runtime = Arc::new(Runtime::new());

    // 原型内置模型：mock（文档 §62 验收对象）。
    runtime
        .register(ai_core::model::ModelSpec {
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
        })
        .await;

    // 演示用第二个模型：stub 模型，用于展示 /v1/models 的多模型与错误路径。
    runtime
        .register(ai_core::model::ModelSpec {
            id: "demo".to_string(),
            name: "Demo Model (unavailable)".to_string(),
            model_type: "llm".to_string(),
            provider: "llama.cpp".to_string(),
            source: None,
            path: None,
            format: Some("gguf".to_string()),
            size_bytes: None,
            memory_estimate: Some(4 * 1024 * 1024 * 1024),
            keep_alive: Some("5m".to_string()),
            context_length: Some(8192),
        })
        .await;

    let state = AppState {
        runtime: runtime.clone(),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/api/runtime", get(runtime_info))
        .with_state(state);

    let addr = "127.0.0.1:11435";
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");

    tracing::info!(%addr, "aiworkd listening");
    println!("AI Runtime running at http://{}", addr);

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

// ---------- handlers ----------

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let count = state.runtime.list_models().await.len();
    Json(HealthResponse {
        status: "ok".to_string(),
        version: VERSION.to_string(),
        model_count: count,
    })
}

/// GET /v1/models（OpenAI-compatible）。
async fn list_models(State(state): State<AppState>) -> Json<Value> {
    let models: Vec<ModelEntry> = state
        .runtime
        .list_models()
        .await
        .into_iter()
        .map(|e| ModelEntry {
            id: e.spec.id,
            object: "model".to_string(),
            created: 0,
            owned_by: format!("aiworkd/{}", e.spec.provider),
        })
        .collect();
    Json(json!({
        "object": "list",
        "data": models,
    }))
}

/// GET /api/runtime（内部管理 API，文档 §17）。
async fn runtime_info(State(state): State<AppState>) -> Json<ai_core::response::RuntimeInfo> {
    Json(state.runtime.runtime_info(VERSION).await)
}

/// POST /v1/chat/completions（文档 §14）。
/// stream=true → SSE；否则普通 JSON。
async fn chat_completions(State(state): State<AppState>, Json(req): Json<ChatRequest>) -> Response {
    let request_id = state.runtime.next_request_id();
    tracing::info!(%request_id, model = %req.model, stream = req.stream, "chat request");

    // 模型解析（文档 §26：request → model loaded? → memory check → eviction → load → inference）。
    let Some(model) = state.runtime.get_model(&req.model).await else {
        return api_error(
            AIError::ModelNotFound,
            format!("model '{}' not found", req.model),
        );
    };

    let provider = state.runtime.chat_provider();
    if model.provider != provider.id() {
        return api_error(
            AIError::ProviderUnavailable,
            format!(
                "provider '{}' is not available in this prototype",
                model.provider
            ),
        );
    }

    let request_guard = state.runtime.request_guard();

    if req.stream {
        match provider.chat_stream(req).await {
            Ok(stream) => {
                let chunks = stream.map(move |chunk| {
                    let _request_guard = &request_guard;
                    Ok::<_, std::convert::Infallible>(Event::default().json_data(chunk).unwrap())
                });
                let done = futures::stream::once(async {
                    Ok::<_, std::convert::Infallible>(Event::default().data("[DONE]"))
                });
                let sse = Sse::new(chunks.chain(done))
                    .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)));
                (StatusCode::OK, sse).into_response()
            }
            Err(e) => api_error(e, "chat stream failed"),
        }
    } else {
        match provider.chat(req).await {
            Ok(resp) => Json(resp).into_response(),
            Err(e) => api_error(e, "chat failed"),
        }
    }
}

/// 统一 API 错误（文档 §45）。
fn api_error(err: AIError, message: impl Into<String>) -> Response {
    let status =
        StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = Json(ApiErrorBody::new(err, message));
    (status, body).into_response()
}
