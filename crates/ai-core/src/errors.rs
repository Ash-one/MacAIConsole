//! 统一错误模型（文档 §45）。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 统一错误类型。API 层将其映射为 `{"error": {"type": ..., "message": ...}}`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AIError {
    ModelNotFound,
    ModelLoadFailed,
    ProviderUnavailable,
    OutOfMemory,
    InvalidRequest,
    DownloadFailed,
    BackendCrashed,
    Timeout,
    Internal,
}

impl AIError {
    pub fn as_str(&self) -> &'static str {
        match self {
            AIError::ModelNotFound => "model_not_found",
            AIError::ModelLoadFailed => "model_load_failed",
            AIError::ProviderUnavailable => "provider_unavailable",
            AIError::OutOfMemory => "out_of_memory",
            AIError::InvalidRequest => "invalid_request",
            AIError::DownloadFailed => "download_failed",
            AIError::BackendCrashed => "backend_crashed",
            AIError::Timeout => "timeout",
            AIError::Internal => "internal",
        }
    }

    pub fn http_status(&self) -> u16 {
        match self {
            AIError::ModelNotFound => 404,
            AIError::InvalidRequest => 400,
            AIError::OutOfMemory => 503,
            AIError::ProviderUnavailable => 503,
            AIError::ModelLoadFailed => 500,
            AIError::DownloadFailed => 502,
            AIError::BackendCrashed => 502,
            AIError::Timeout => 504,
            AIError::Internal => 500,
        }
    }
}

impl fmt::Display for AIError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::error::Error for AIError {}

/// API 错误响应体（文档 §45）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

impl ApiErrorBody {
    pub fn new(error: AIError, message: impl Into<String>) -> Self {
        Self {
            error: ApiErrorDetail {
                error_type: error.as_str().to_string(),
                message: message.into(),
            },
        }
    }
}
