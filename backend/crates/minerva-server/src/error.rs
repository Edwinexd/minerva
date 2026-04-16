use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

/// Template parameters for a localized message. Keys are stable identifiers
/// (`max_bytes`, `status`, etc.) that the frontend i18n layer interpolates
/// into the translated string.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ErrorParams(BTreeMap<&'static str, String>);

impl ErrorParams {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<const N: usize> From<[(&'static str, String); N]> for ErrorParams {
    fn from(items: [(&'static str, String); N]) -> Self {
        Self(items.into_iter().collect())
    }
}

/// A structured, translatable message. Used for warnings/errors in response
/// bodies where the message is not itself the HTTP error (e.g. canvas sync
/// per-item failures).
#[derive(Debug, Clone, Serialize)]
pub struct LocalizedMessage {
    pub code: &'static str,
    #[serde(skip_serializing_if = "params_is_empty")]
    pub params: ErrorParams,
}

fn params_is_empty(p: &ErrorParams) -> bool {
    p.0.is_empty()
}

impl LocalizedMessage {
    pub fn new(code: &'static str) -> Self {
        Self {
            code,
            params: ErrorParams::default(),
        }
    }

    pub fn with<P: Into<ErrorParams>>(code: &'static str, params: P) -> Self {
        Self {
            code,
            params: params.into(),
        }
    }

    /// Builds a localized message from an AppError, preserving the caller's
    /// code/params for BadRequest and collapsing internals to `"internal"`.
    pub fn from_app_error(err: &AppError) -> Self {
        match err {
            AppError::NotFound => Self::new("not_found"),
            AppError::Unauthorized => Self::new("unauthorized"),
            AppError::Forbidden => Self::new("forbidden"),
            AppError::BadRequest { code, params } => Self {
                code,
                params: params.clone(),
            },
            AppError::QuotaExceeded => Self::new("quota.student_exceeded"),
            AppError::OwnerQuotaExceeded => Self::new("quota.owner_exceeded"),
            AppError::PrivacyNotAcknowledged => Self::new("privacy.not_acknowledged"),
            AppError::Database(_) | AppError::Internal(_) => Self::new("internal"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AppError {
    #[error("not found")]
    NotFound,

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("bad request ({code})")]
    BadRequest {
        code: &'static str,
        params: ErrorParams,
    },

    #[error("daily token quota exceeded")]
    QuotaExceeded,

    #[error("course owner has reached their daily AI spending cap")]
    OwnerQuotaExceeded,

    #[error("privacy acknowledgment required")]
    PrivacyNotAcknowledged,

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Constructs a parameter-free BadRequest for the given stable error code.
    pub fn bad_request(code: &'static str) -> Self {
        Self::BadRequest {
            code,
            params: ErrorParams::default(),
        }
    }

    /// Constructs a BadRequest with template parameters for the frontend's
    /// i18n layer to interpolate.
    pub fn bad_request_with<P: Into<ErrorParams>>(code: &'static str, params: P) -> Self {
        Self::BadRequest {
            code,
            params: params.into(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let msg = LocalizedMessage::from_app_error(&self);
        let status = match &self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden | AppError::PrivacyNotAcknowledged => StatusCode::FORBIDDEN,
            AppError::BadRequest { .. } => StatusCode::BAD_REQUEST,
            AppError::QuotaExceeded | AppError::OwnerQuotaExceeded => StatusCode::TOO_MANY_REQUESTS,
            AppError::Database(_) | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        // Internal details are logged, never returned to the client.
        match &self {
            AppError::Database(e) => tracing::error!("database error: {:?}", e),
            AppError::Internal(m) => tracing::error!("internal error: {}", m),
            _ => {}
        }

        // `message` is an English fallback for logs / curl / dev. The frontend
        // ignores it and renders from `code` + `params` via i18next.
        let body = axum::Json(json!({
            "code": msg.code,
            "params": msg.params,
            "message": self.to_string(),
        }));
        (status, body).into_response()
    }
}
