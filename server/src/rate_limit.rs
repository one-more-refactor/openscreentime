//! In-memory fixed-window rate limiting for the unauthenticated surfaces
//! (`/api/auth/*` login/register/OIDC attempts and `/agent/enroll`).
//!
//! Keyed by client IP: the first `X-Forwarded-For` value when
//! `SENTINEL_TRUST_PROXY=1`, otherwise the peer address. Over-limit requests
//! get a 429 with the standard error envelope.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::state::AppState;

const WINDOW: Duration = Duration::from_secs(60);
const AUTH_MAX: u32 = 10;
const ENROLL_MAX: u32 = 5;
/// Agent distribution (`/install.sh`, `/api/agent/latest`, downloads): public
/// but bounded so the endpoints can't be used as a free bandwidth amplifier.
/// One install touches ~3 of these; fleets poll the manifest once a day.
const DIST_MAX: u32 = 30;
/// Parent companion API (`/api/parent/*`): token-authenticated, but a companion
/// polling for requests/alerts shouldn't hammer the server. Generous enough for
/// a 15–30s poll plus a burst of approvals.
const PARENT_MAX: u32 = 60;
/// Prune dead windows once the bucket map grows past this.
const PRUNE_THRESHOLD: usize = 10_000;

/// (scope, client key) → (window start, hits in window).
type Buckets = HashMap<(&'static str, String), (Instant, u32)>;

/// Fixed-window counters, one bucket per (scope, client key).
pub struct RateLimiter {
    trust_proxy: bool,
    buckets: Mutex<Buckets>,
}

impl RateLimiter {
    pub fn from_env() -> Self {
        let trust_proxy = std::env::var("SENTINEL_TRUST_PROXY").map(|v| v == "1") == Ok(true);
        RateLimiter {
            trust_proxy,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Record a hit; returns false when the window's budget is exhausted.
    fn check(&self, scope: &'static str, key: String, max: u32) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().expect("rate limiter poisoned");
        if buckets.len() > PRUNE_THRESHOLD {
            buckets.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
        }
        let entry = buckets.entry((scope, key)).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= max
    }

    /// The client key for a request: the *last* X-Forwarded-For value when the
    /// proxy is trusted, else the peer address.
    ///
    /// A trusted reverse proxy appends the real peer IP to the end of XFF, so the
    /// last hop is the only element the client can't forge. Keying on the first
    /// value would let an attacker rotate `X-Forwarded-For` per request and land
    /// each one in a fresh bucket, defeating the limiter entirely.
    fn client_key(&self, req: &Request) -> String {
        if self.trust_proxy {
            if let Some(xff) = req
                .headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit(',').next())
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
            {
                return xff.to_string();
            }
        }
        req.extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

async fn limit(st: AppState, scope: &'static str, max: u32, req: Request, next: Next) -> Response {
    let key = st.rate_limiter.client_key(&req);
    if !st.rate_limiter.check(scope, key, max) {
        return AppError::RateLimited("too many requests, slow down".into()).into_response();
    }
    next.run(req).await
}

/// Middleware for the auth attempt endpoints: 10 req / 60 s / IP.
pub async fn limit_auth(State(st): State<AppState>, req: Request, next: Next) -> Response {
    limit(st, "auth", AUTH_MAX, req, next).await
}

/// Middleware for `/agent/enroll`: 5 req / 60 s / IP.
pub async fn limit_enroll(State(st): State<AppState>, req: Request, next: Next) -> Response {
    limit(st, "enroll", ENROLL_MAX, req, next).await
}

/// Middleware for the agent-distribution endpoints: 30 req / 60 s / IP.
pub async fn limit_dist(State(st): State<AppState>, req: Request, next: Next) -> Response {
    limit(st, "dist", DIST_MAX, req, next).await
}

/// Middleware for the parent companion API: 60 req / 60 s / IP.
pub async fn limit_parent(State(st): State<AppState>, req: Request, next: Next) -> Response {
    limit(st, "parent", PARENT_MAX, req, next).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_window_enforces_budget() {
        let rl = RateLimiter {
            trust_proxy: false,
            buckets: Mutex::new(HashMap::new()),
        };
        for _ in 0..AUTH_MAX {
            assert!(rl.check("auth", "1.2.3.4".into(), AUTH_MAX));
        }
        assert!(!rl.check("auth", "1.2.3.4".into(), AUTH_MAX));
        // Other keys and scopes are independent buckets.
        assert!(rl.check("auth", "5.6.7.8".into(), AUTH_MAX));
        assert!(rl.check("enroll", "1.2.3.4".into(), ENROLL_MAX));
    }

    #[test]
    fn xff_keys_on_last_hop_not_client_spoofed_first() {
        // Behind a trusted proxy, `X-Forwarded-For: <spoofed>, <real-peer>` must
        // key on the real peer (last value) so a rotating first value can't dodge
        // the limiter.
        let rl = RateLimiter {
            trust_proxy: true,
            buckets: Mutex::new(HashMap::new()),
        };
        let mut req = Request::new(axum::body::Body::empty());
        req.headers_mut()
            .insert("x-forwarded-for", "203.0.113.9, 10.0.0.5".parse().unwrap());
        assert_eq!(rl.client_key(&req), "10.0.0.5");
    }
}
