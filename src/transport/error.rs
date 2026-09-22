//! 错误信封:错误必须匹配 HTTP 状态,不返回 200+success:false(http-api §1)。
//! message 只含脱敏可操作说明;Cookie/Token/正文不得进入错误信封。

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidRequest,
    InvalidCursor,
    AuthenticationRequired,
    InvalidCredentials,
    CsrfRejected,
    OriginRejected,
    ResourceNotFound,
    VersionConflict,
    ActionInProgress,
    IdempotencyConflict,
    AlreadyInitialized,
    RestoreQuarantined,
    UnsupportedCapability,
    RuleConflict,
    IncompleteOrder,
    ContentTooLong,
    IneligibleOrder,
    UnsafeRetry,
    RateLimited,
    PersistenceUnavailable,
    AccountUnavailable,
    ServiceStopping,
}

impl ErrorCode {
    fn snake(self) -> &'static str {
        match self {
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::InvalidCursor => "invalid_cursor",
            ErrorCode::AuthenticationRequired => "authentication_required",
            ErrorCode::InvalidCredentials => "invalid_credentials",
            ErrorCode::CsrfRejected => "csrf_rejected",
            ErrorCode::OriginRejected => "origin_rejected",
            ErrorCode::ResourceNotFound => "resource_not_found",
            ErrorCode::VersionConflict => "version_conflict",
            ErrorCode::ActionInProgress => "action_in_progress",
            ErrorCode::IdempotencyConflict => "idempotency_conflict",
            ErrorCode::AlreadyInitialized => "already_initialized",
            ErrorCode::RestoreQuarantined => "restore_quarantined",
            ErrorCode::UnsupportedCapability => "unsupported_capability",
            ErrorCode::RuleConflict => "rule_conflict",
            ErrorCode::IncompleteOrder => "incomplete_order",
            ErrorCode::ContentTooLong => "content_too_long",
            ErrorCode::IneligibleOrder => "ineligible_order",
            ErrorCode::UnsafeRetry => "unsafe_retry",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::PersistenceUnavailable => "persistence_unavailable",
            ErrorCode::AccountUnavailable => "account_unavailable",
            ErrorCode::ServiceStopping => "service_stopping",
        }
    }

    fn status(self) -> StatusCode {
        match self {
            ErrorCode::InvalidRequest | ErrorCode::InvalidCursor => StatusCode::BAD_REQUEST,
            ErrorCode::AuthenticationRequired | ErrorCode::InvalidCredentials => {
                StatusCode::UNAUTHORIZED
            }
            ErrorCode::CsrfRejected | ErrorCode::OriginRejected => StatusCode::FORBIDDEN,
            ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
            ErrorCode::VersionConflict
            | ErrorCode::ActionInProgress
            | ErrorCode::IdempotencyConflict
            | ErrorCode::AlreadyInitialized
            | ErrorCode::RestoreQuarantined => StatusCode::CONFLICT,
            ErrorCode::UnsupportedCapability
            | ErrorCode::RuleConflict
            | ErrorCode::IncompleteOrder
            | ErrorCode::ContentTooLong
            | ErrorCode::IneligibleOrder
            | ErrorCode::UnsafeRetry => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::PersistenceUnavailable
            | ErrorCode::AccountUnavailable
            | ErrorCode::ServiceStopping => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn retryable(self) -> bool {
        matches!(
            self,
            ErrorCode::RateLimited | ErrorCode::PersistenceUnavailable | ErrorCode::ServiceStopping
        )
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub request_id: String,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            request_id: request_id.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.code.status();
        let body = json!({
            "error": {
                "code": self.code.snake(),
                "message": self.message,
                "request_id": self.request_id,
                "retryable": self.code.retryable(),
            }
        });
        (status, Json(body)).into_response()
    }
}

pub fn request_id(ext: &axum::Extension<String>) -> String {
    ext.0.clone()
}
