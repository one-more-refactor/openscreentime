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
    /// and `OST_OPEN_REGISTRATION` isn't set (see docs/DEPLOY.md).
    #[error("{0}")]
    RegistrationClosed(String),
    /// 428 with the stable code `step_up_required`: the route is in the
    /// sensitive corner and the session's confirm window is shut (docs/AUTH.md).
    /// The client's contract is to confirm it's you (a passkey, or a code from
    /// your computer) and retry the same request — hence not a 403.
    #[error("{0}")]
    StepUpRequired(String),
    /// 401 `wrong_code`: a sign-in / confirm code didn't match; type it again.
    #[error("{0}")]
    WrongCode(String),
    /// 410 `code_expired`: the code ran out (time or tries); ask for a new one.
    #[error("{0}")]
    CodeExpired(String),
    /// 403 with the stable code `forbidden_for_member`: a member session
    /// (a child, or a self-tracking adult) asked for something only the hub
    /// (owner/parent) may do. The member layer in `members.rs` fails closed.
    #[error("{0}")]
    ForbiddenForMember(String),
    /// 404 with the stable code `no_account`: the OS login that asked for a
    /// device voucher is not linked to any person on this household.
    #[error("{0}")]
    NoAccount(String),
    /// 410 with the stable code `device_retired` and a top-level
    /// `"retired": true`: this device token belonged to a computer that was
    /// removed from the household. The agent's contract is to take itself off
    /// that computer (thaw, drop the lock, the firewall and the DNS pin) and
    /// stop — the one answer it does that on; a plain 401 never does.
    #[error("{0}")]
    DeviceRetired(String),
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
            AppError::StepUpRequired(_) => (StatusCode::PRECONDITION_REQUIRED, "step_up_required"),
            AppError::WrongCode(_) => (StatusCode::UNAUTHORIZED, "wrong_code"),
            AppError::CodeExpired(_) => (StatusCode::GONE, "code_expired"),
            AppError::ForbiddenForMember(_) => (StatusCode::FORBIDDEN, "forbidden_for_member"),
            AppError::NoAccount(_) => (StatusCode::NOT_FOUND, "no_account"),
            AppError::DeviceRetired(_) => (StatusCode::GONE, "device_retired"),
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
        let mut body = json!({
            "error": { "code": code, "message": message }
        });
        if code == "device_retired" {
            body["retired"] = json!(true);
        }
        (status, Json(body)).into_response()
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
