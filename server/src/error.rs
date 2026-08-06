//! Unified error type. Serializes to `{ "error": { code, message } }` per API.md.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    RateLimited(String),
    /// 403 with the stable code `registration_closed`: an admin already exists
    /// and `SENTINEL_OPEN_REGISTRATION` isn't set (see docs/DEPLOY.md).
    #[error("{0}")]
    RegistrationClosed(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::RateLimited(_) => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            AppError::RegistrationClosed(_) => (StatusCode::FORBIDDEN, "registration_closed"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = self.parts();
        // Internal errors wrap anyhow/sqlx detail (table names, constraint
        // names, `invalid input syntax for type inet`, …). Log the full error
        // server-side, but never ship it to the client — an enrolled device is
        // a hostile caller and would use it to map the schema.
        let message = match self {
            AppError::Internal(ref e) => {
                tracing::error!(error = %e, "internal error");
                "internal error".to_string()
            }
            _ => self.to_string(),
        };
        let body = Json(json!({
            "error": { "code": code, "message": message }
        }));
        (status, body).into_response()
    }
}

// sqlx errors become internal errors (unique-violation mapping happens at call
// sites that care, via `is_unique_violation`).
impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => AppError::NotFound("resource not found".into()),
            other => AppError::Internal(other.into()),
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
