//! 统一错误模型（文档 §45）。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 统一错误类型。API 层将其映射为 `{"error": {"type": ..., "message": ...}}`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AIError {
    ModelNotFound,
    TaskNotFound,
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
            AIError::TaskNotFound => "task_not_found",
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
            AIError::ModelNotFound | AIError::TaskNotFound => 404,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// as_str 字符串与 HTTP 状态码是 API 对外契约（GUI/CLI 按它归类错误），
    /// 任何调整都必须同步客户端解码逻辑，这里固化当前映射。
    #[test]
    fn wire_names_and_http_status_are_stable() {
        let expected = [
            (AIError::ModelNotFound, "model_not_found", 404),
            (AIError::TaskNotFound, "task_not_found", 404),
            (AIError::InvalidRequest, "invalid_request", 400),
            (AIError::ProviderUnavailable, "provider_unavailable", 503),
            (AIError::OutOfMemory, "out_of_memory", 503),
            (AIError::ModelLoadFailed, "model_load_failed", 500),
            (AIError::DownloadFailed, "download_failed", 502),
            (AIError::BackendCrashed, "backend_crashed", 502),
            (AIError::Timeout, "timeout", 504),
            (AIError::Internal, "internal", 500),
        ];
        for (error, name, status) in expected {
            assert_eq!(error.as_str(), name, "{name} wire name changed");
            assert_eq!(error.to_string(), name, "{name} Display changed");
            assert_eq!(error.http_status(), status, "{name} http status changed");
        }
    }

    #[test]
    fn api_error_body_serializes_to_documented_shape() {
        let body = ApiErrorBody::new(AIError::ModelNotFound, "model 'x' is not registered");
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "error": {
                    "type": "model_not_found",
                    "message": "model 'x' is not registered",
                }
            })
        );
        let decoded: ApiErrorBody = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.error.error_type, "model_not_found");
        assert_eq!(decoded.error.message, "model 'x' is not registered");
    }
}
