use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use tracing::error;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("resource not found")]
    NotFound,
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("resource conflict: {0}")]
    Conflict(String),
    #[error("operation is blocked: {0}")]
    Blocked(String),
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("internal operation failed: {0}")]
    Internal(String),
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
    correlation_id: Uuid,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let correlation_id = Uuid::now_v7();
        let (status, code, message) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", self.to_string()),
            Self::Invalid(message) => (StatusCode::BAD_REQUEST, "invalid_request", message),
            Self::Conflict(message) => (StatusCode::CONFLICT, "conflict", message),
            Self::Blocked(message) => (StatusCode::CONFLICT, "operation_blocked", message),
            Self::Database(ref source) => {
                error!(%correlation_id, error = %source, "database request failed");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "database_unavailable",
                    "The durable state store is unavailable.".to_owned(),
                )
            }
            Self::Internal(ref message) => {
                error!(%correlation_id, error = %message, "internal request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "The operation could not be completed.".to_owned(),
                )
            }
        };
        (
            status,
            Json(ErrorBody {
                error: ErrorDetail {
                    code,
                    message,
                    correlation_id,
                },
            }),
        )
            .into_response()
    }
}
