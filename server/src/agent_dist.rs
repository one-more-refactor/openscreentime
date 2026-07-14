//! Agent binary distribution: the container image bundles the musl-static
//! headless agent under `SENTINEL_AGENT_DIR` (default `/app/agent`, see
//! Containerfile) together with a `manifest.json` describing version, target,
//! and sha256. Three public (unauthenticated) endpoints serve it:
//!
//!   * `GET /api/agent/latest`          → the manifest JSON
//!   * `GET /api/agent/download/:file`  → the binary itself
//!   * `GET /install.sh`                → the curl|sh installer (server/install.sh)
//!
//! The binary is not a secret (auth happens at enrollment with the one-time
//! token), but the routes are rate-limited (see rate_limit.rs) so they can't
//! be used as a cheap bandwidth amplifier. On a dev box without the image
//! layout these endpoints 404 with the normal error envelope.

use std::path::PathBuf;

use axum::extract::Path;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use crate::error::{AppError, AppResult};

/// The curl|sh installer, embedded at compile time so the endpoint works even
/// without the agent artifacts present (it degrades at runtime with a clear
/// error when `/api/agent/latest` 404s).
const INSTALL_SH: &str = include_str!("../install.sh");

fn agent_dir() -> PathBuf {
    PathBuf::from(std::env::var("SENTINEL_AGENT_DIR").unwrap_or_else(|_| "/app/agent".into()))
}

/// `GET /api/agent/latest` — the artifact manifest bundled with this image.
pub async fn latest() -> AppResult<Json<Value>> {
    let path = agent_dir().join("manifest.json");
    let body = tokio::fs::read_to_string(&path).await.map_err(|_| {
        AppError::NotFound("no agent build bundled with this server (dev build?)".into())
    })?;
    let manifest: Value = serde_json::from_str(&body)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("corrupt agent manifest: {e}")))?;
    Ok(Json(manifest))
}

/// `GET /api/agent/download/:file` — one of the manifest's artifacts.
///
/// `:file` is matched as a single path segment (so `/` can't appear), but we
/// still reject separators and `..` defensively — this handler must never be
/// able to read outside `SENTINEL_AGENT_DIR`.
pub async fn download(Path(file): Path<String>) -> AppResult<Response> {
    if file.is_empty()
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
        || file == "manifest.json"
    {
        return Err(AppError::BadRequest("invalid artifact name".into()));
    }
    let path = agent_dir().join(&file);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound(format!("no such agent artifact: {file}")))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// `GET /install.sh` — the one-command installer shown in the enroll modal.
pub async fn install_sh() -> Response {
    (
        [(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")],
        INSTALL_SH,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn install_sh_is_posix_and_nonempty() {
        // The installer is embedded at compile time; make sure a refactor never
        // ships an empty or bash-only script (it must start with a sh shebang
        // and keep `set -eu` near the top — the security posture depends on it).
        assert!(super::INSTALL_SH.starts_with("#!/bin/sh"));
        assert!(super::INSTALL_SH.contains("set -eu"));
    }
}
