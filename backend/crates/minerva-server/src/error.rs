use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AppError {
    #[error("not found")]
    NotFound,

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("daily token quota exceeded")]
    QuotaExceeded,

    #[error("course owner has reached their daily AI spending cap; contact lambda@dsv.su.se to request an increase")]
    OwnerQuotaExceeded,

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::NotFound => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, self.to_string()),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            AppError::QuotaExceeded => (StatusCode::TOO_MANY_REQUESTS, self.to_string()),
            AppError::OwnerQuotaExceeded => (StatusCode::TOO_MANY_REQUESTS, self.to_string()),
            AppError::Database(e) => {
                tracing::error!("database error: {:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
            AppError::Internal(msg) => {
                tracing::error!("internal error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
        };

        let body = axum::Json(json!({ "error": message }));
        (status, body).into_response()
    }
}
