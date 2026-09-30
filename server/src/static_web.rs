//! Serves the built web SPA (see `web/`) as the Axum router fallback.
//!
//! In production the server is a single-origin deployment: the container
//! image bundles the Vite build output and this module serves it directly,
//! so there is no separate web server / CORS hop in front of the API.
//!
//! Controlled by `OST_WEB_DIR` (default `/app/web`). If the directory
//! doesn't exist — e.g. running `cargo run` in dev without a web build — we
//! just skip mounting it and log a warning; the API still works on its own
//! (paired with the Vite dev server's proxy in that case).

use std::path::{Path, PathBuf};

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::any;
use axum::{Json, Router};
use tower_http::services::{ServeDir, ServeFile};

/// The directory to serve the web UI from, if it exists. `None` (and a warning)
/// when absent, so the caller mounts no fallback and serves the API only.
pub fn web_dir() -> Option<PathBuf> {
    let dir = std::env::var("OST_WEB_DIR").unwrap_or_else(|_| "/app/web".into());
    let path = PathBuf::from(&dir);
    if path.is_dir() {
        tracing::info!("serving web UI from {dir}");
        Some(path)
    } else {
        tracing::warn!("OST_WEB_DIR ({dir}) not found; serving API only (no web UI mounted)");
        None
    }
}

/// An unmatched `/api` or `/agent` path: the JSON error envelope's 404.
async fn api_not_found() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": { "code": "not_found", "message": "no such route" } })),
    )
}

/// Put the console behind `app`, for every path none of its routes matched:
///
/// - an unmatched `/api/…` or `/agent/…` path is a real JSON 404 — never the
///   SPA shell rewritten to 200, or a route that silently stopped existing
///   would look healthy to a monitor (routes beat these wildcards);
/// - a file of the web build is that file, with its `Last-Modified` and
///   conditional requests (a 304 when the browser's copy is current);
/// - any other path — `/computers`, a bookmarked `/child/<id>` — is the SPA
///   shell, `index.html`, for the client-side router: **200**, or **304**
///   when the browser's cached copy is still valid.
///
/// The shell is `ServeDir`'s `fallback`, whose status passes through as
/// served. It used to be `not_found_service`, which stamps 404 on whatever
/// the fallback answers: a 200 shell (rewritten back to 200 afterwards) —
/// and, on every reload, a 304 turned into a bare 404 with no body, a blank
/// page on every console page but `/` (acceptance round 5).
///
/// Without a web build (`dir` is `None`) only the API is served.
pub fn mount(app: Router, dir: Option<&Path>) -> Router {
    let app = app
        .route("/api/{*rest}", any(api_not_found))
        .route("/agent/{*rest}", any(api_not_found));
    let Some(dir) = dir else {
        return app;
    };
    let shell = ServeFile::new(dir.join("index.html"));
    app.fallback_service(ServeDir::new(dir).fallback(shell))
        .layer(axum::middleware::map_response(shell_revalidates))
}

/// The SPA shell (any `text/html` answer) is revalidated on every load
/// (`Cache-Control: no-cache` — a 304 when it hasn't changed): a shell kept
/// by heuristic caching past an update names asset files that are gone.
/// Assets keep their plain `Last-Modified` caching.
async fn shell_revalidates(mut resp: Response) -> Response {
    let is_html = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"text/html"));
    if is_html && !resp.headers().contains_key(header::CACHE_CONTROL) {
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{HeaderMap, Request};
    use axum::routing::get;
    use tower::ServiceExt;

    const INDEX: &str = "<!doctype html><title>OpenScreenTime</title><div id=root></div>";

    /// A web build on disk: the shell and one asset.
    fn build() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ost-web-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), INDEX).unwrap();
        std::fs::write(dir.join("assets/app-3f2a.js"), "console.log(1)").unwrap();
        dir
    }

    fn app(dir: &Path) -> Router {
        let api = Router::new().route("/api/health-ish", get(|| async { Json("ok") }));
        mount(api, Some(dir))
    }

    async fn fetch(
        app: &Router,
        path: &str,
        headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, String) {
        let mut req = Request::get(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    /// Acceptance round 5: F5 on /computers (or a bookmarked /child/<id>)
    /// was a blank page — the browser revalidated its copy, the shell said
    /// 304, and `not_found_service` made that a bare 404. Every console
    /// path is the shell: 200 the first time, 304 on a reload while it's
    /// current, 200 again when it isn't (or the validator is not ours).
    #[tokio::test]
    async fn a_reload_of_any_console_page_is_the_shell_or_a_304() {
        let dir = build();
        let app = app(&dir);
        for path in [
            "/",
            "/computers",
            "/child/7b1c9f",
            "/child/7b1c9f/rules",
            "/settings",
        ] {
            let (status, h, body) = fetch(&app, path, &[]).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(body, INDEX, "{path}");
            assert!(h[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html"));
            assert_eq!(h[header::CACHE_CONTROL], "no-cache", "{path}");
            let modified = h[header::LAST_MODIFIED].to_str().unwrap().to_string();

            // The reload: the browser's copy is current.
            let (status, _, body) = fetch(&app, path, &[("if-modified-since", &modified)]).await;
            assert_eq!(status, StatusCode::NOT_MODIFIED, "{path}");
            assert!(body.is_empty());

            // A copy from before the build: the shell again, in full.
            let old = "Mon, 01 Jan 2001 00:00:00 GMT";
            let (status, _, body) = fetch(&app, path, &[("if-modified-since", old)]).await;
            assert_eq!((status, body.as_str()), (StatusCode::OK, INDEX), "{path}");
            // An entity tag we never gave out matches nothing: the shell.
            let (status, _, body) = fetch(&app, path, &[("if-none-match", "\"not-ours\"")]).await;
            assert_eq!((status, body.as_str()), (StatusCode::OK, INDEX), "{path}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The API is never the shell: its routes answer, and a path that
    /// isn't one is a JSON 404 — conditional headers or not.
    #[tokio::test]
    async fn api_paths_stay_api() {
        let dir = build();
        let app = app(&dir);
        let (status, _, body) = fetch(&app, "/api/health-ish", &[]).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "\"ok\""));
        for path in ["/api/nope", "/agent/nope", "/api/devices/x/nope"] {
            for headers in [
                &[][..],
                &[("if-modified-since", "Mon, 01 Jan 2035 00:00:00 GMT")][..],
            ] {
                let (status, h, body) = fetch(&app, path, headers).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
                assert!(h[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with("application/json"));
                assert!(body.contains("\"not_found\""), "{body}");
                assert!(!h.contains_key(header::CACHE_CONTROL));
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Assets are served as files and keep caching: `Last-Modified`, a 304
    /// for a current copy, no `no-cache`.
    #[tokio::test]
    async fn assets_keep_caching() {
        let dir = build();
        let app = app(&dir);
        let (status, h, body) = fetch(&app, "/assets/app-3f2a.js", &[]).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "console.log(1)"));
        assert!(h[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("javascript"));
        assert!(!h.contains_key(header::CACHE_CONTROL));
        let modified = h[header::LAST_MODIFIED].to_str().unwrap().to_string();
        let (status, _, _) = fetch(
            &app,
            "/assets/app-3f2a.js",
            &[("if-modified-since", &modified)],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// No web build: the API alone, its unmatched paths still JSON 404s.
    #[tokio::test]
    async fn without_a_web_build_only_the_api_is_served() {
        let api = Router::new().route("/api/health-ish", get(|| async { Json("ok") }));
        let app = mount(api, None);
        let (status, _, body) = fetch(&app, "/api/nope", &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("\"not_found\""));
        let (status, _, _) = fetch(&app, "/computers", &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
