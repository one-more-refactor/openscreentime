//! Serves the built web SPA (see `web/`) as the Axum router fallback.
//!
//! In production the server is a single-origin deployment: the container
//! image bundles the Vite build output and this module serves it directly,
//! so there is no separate web server / CORS hop in front of the API.
//!
//! Controlled by `SENTINEL_WEB_DIR` (default `/app/web`). If the directory
//! doesn't exist — e.g. running `cargo run` in dev without a web build — we
//! just skip mounting it and log a warning; the API still works on its own
//! (paired with the Vite dev server's proxy in that case).

use std::path::PathBuf;

use axum::http::{header, StatusCode};
use axum::response::Response;

/// The directory to serve the web UI from, if it exists. `None` (and a warning)
/// when absent, so the caller mounts no fallback and serves the API only.
pub fn web_dir() -> Option<PathBuf> {
    let dir = std::env::var("SENTINEL_WEB_DIR").unwrap_or_else(|_| "/app/web".into());
    let path = PathBuf::from(&dir);
    if path.is_dir() {
        tracing::info!("serving web UI from {dir}");
        Some(path)
    } else {
        tracing::warn!("SENTINEL_WEB_DIR ({dir}) not found; serving API only (no web UI mounted)");
        None
    }
}

/// Rewrite the SPA fallback's 404 to a 200. `ServeDir`'s `not_found_service`
/// serves `index.html` for any unmatched path (so the client-side router can
/// take over), but carries a 404 status through — wrong for a real app route
/// like `/devices`, and it trips monitoring and some client routers.
///
/// Applied as an `axum::middleware::map_response` layer. It only flips a 404
/// whose body is `text/html` — i.e. the SPA shell. API errors are JSON, so a
/// genuine `/api/...` 404 is untouched.
pub async fn spa_ok(resp: Response) -> Response {
    let is_html = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.as_bytes().starts_with(b"text/html"))
        .unwrap_or(false);
    if resp.status() == StatusCode::NOT_FOUND && is_html {
        let (mut parts, body) = resp.into_parts();
        parts.status = StatusCode::OK;
        return Response::from_parts(parts, body);
    }
    resp
}
