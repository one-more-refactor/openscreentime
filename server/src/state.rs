//! Shared application state, in-memory stores, the agent WebSocket hub, and the
//! request extractors that enforce auth + tenant isolation.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::CookieJar;
use sqlx::PgPool;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;
use webauthn_rs::prelude::{PasskeyAuthentication, PasskeyRegistration};
use webauthn_rs::Webauthn;

use crate::error::AppError;

pub const SESSION_COOKIE: &str = "ost_session";
pub const REG_COOKIE: &str = "reg_sid";
pub const AUTH_COOKIE: &str = "auth_sid";

/// An environment variable the operator actually configured, or `None`.
///
/// Every optional setting goes through this, because "the variable is present"
/// is not the same question as "the operator set it":
///
/// * `""` — compose forwards `${VAR:-}` as an empty string, not as unset.
/// * `"${VAR:-}"` — podman-compose does not expand a `:-` default when the
///   variable is undefined, and forwards the literal instead. That string is
///   not empty, so a naive check accepts it: a fresh no-OIDC deploy crash-looped
///   running discovery against a URL made of shell syntax.
/// * `"https://host}"` — a nested `${A:-${B}}` comes back with the trailing
///   brace still attached.
///
/// No legitimate value contains `${` or `}`, and these failures are the
/// expensive kind: the server boots, then dies citing a variable the operator
/// never touched.
pub fn configured(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && !v.contains("${") && !v.contains('}'))
}

#[cfg(test)]
mod env_tests {
    use super::configured;

    /// These tests share one process environment, so each cleans up after
    /// itself rather than relying on ordering.
    fn with_var<T>(key: &str, value: &str, f: impl FnOnce() -> T) -> T {
        std::env::set_var(key, value);
        let out = f();
        std::env::remove_var(key);
        out
    }

    #[test]
    fn a_real_value_is_configured() {
        with_var("OST_TEST_REAL", "https://auth.example.com/application/o/ost/", || {
            assert_eq!(
                configured("OST_TEST_REAL").as_deref(),
                Some("https://auth.example.com/application/o/ost/")
            );
        });
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        with_var("OST_TEST_PAD", "  https://a.example  ", || {
            assert_eq!(configured("OST_TEST_PAD").as_deref(), Some("https://a.example"));
        });
    }

    #[test]
    fn absent_and_empty_are_both_unset() {
        assert_eq!(configured("OST_TEST_NEVER_SET"), None);
        with_var("OST_TEST_EMPTY", "", || {
            assert_eq!(configured("OST_TEST_EMPTY"), None);
        });
        with_var("OST_TEST_BLANK", "   ", || {
            assert_eq!(configured("OST_TEST_BLANK"), None);
        });
    }

    /// The regression this function exists for: podman-compose forwarding an
    /// unexpanded `${VAR:-}` crash-looped the server on every startup.
    #[test]
    fn an_unexpanded_placeholder_is_unset() {
        with_var("OST_TEST_LITERAL", "${OST_OIDC_ISSUER:-}", || {
            assert_eq!(configured("OST_TEST_LITERAL"), None);
        });
    }

    /// And the nested-default form, which arrives expanded but broken.
    #[test]
    fn a_leftover_brace_is_unset() {
        with_var("OST_TEST_BRACE", "https://ost.example.com}", || {
            assert_eq!(configured("OST_TEST_BRACE"), None);
        });
    }
}

/// How long an unconsumed WebAuthn challenge lives before it's swept. Abandoned
/// register/login ceremonies would otherwise accumulate in memory forever.
pub const CHALLENGE_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// Server-side WebAuthn registration challenge, keyed by a temp cookie.
pub struct RegChallenge {
    pub email: String,
    pub display_name: String,
    pub reg: PasskeyRegistration,
    pub created: std::time::Instant,
}

/// Server-side WebAuthn authentication challenge, keyed by a temp cookie.
pub struct AuthChallenge {
    pub admin_id: Uuid,
    pub tenant_id: Uuid,
    pub auth: PasskeyAuthentication,
    pub created: std::time::Instant,
}

/// Hub of live agent WebSocket connections.
#[derive(Default)]
pub struct Hub {
    /// device_id -> sender that writes JSON frames to that agent's socket.
    agents: RwLock<HashMap<Uuid, mpsc::UnboundedSender<serde_json::Value>>>,
}

impl Hub {
    pub async fn register_agent(
        &self,
        device_id: Uuid,
        tx: mpsc::UnboundedSender<serde_json::Value>,
    ) {
        self.agents.write().await.insert(device_id, tx);
    }

    pub async fn unregister_agent(&self, device_id: Uuid) {
        self.agents.write().await.remove(&device_id);
    }

    pub async fn is_online(&self, device_id: Uuid) -> bool {
        self.agents.read().await.contains_key(&device_id)
    }

    /// Push a JSON frame to a connected agent. Returns true if delivered.
    pub async fn push(&self, device_id: Uuid, frame: serde_json::Value) -> bool {
        if let Some(tx) = self.agents.read().await.get(&device_id) {
            tx.send(frame).is_ok()
        } else {
            false
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub webauthn: Arc<Webauthn>,
    /// Session cookies are `Secure` unless OST_INSECURE_COOKIES=1 (dev).
    pub cookie_secure: bool,
    /// Public base URL (OST_PUBLIC_URL, falls back to the WebAuthn RP
    /// origin) — used for OIDC redirect URIs and post-login redirects.
    pub public_url: String,
    pub reg_states: Arc<RwLock<HashMap<String, RegChallenge>>>,
    pub auth_states: Arc<RwLock<HashMap<String, AuthChallenge>>>,
    pub oidc: Option<Arc<crate::auth_oidc::Oidc>>,
    pub rate_limiter: Arc<crate::rate_limit::RateLimiter>,
    pub hub: Arc<Hub>,
}

/// Extractor: an authenticated admin. Carries tenant_id so every downstream
/// query can scope by it. Sessions live in Postgres (`admin_sessions`), the
/// cookie value is sha256-hashed at rest like device tokens.
#[derive(Clone, Copy)]
pub struct AuthAdmin {
    pub admin_id: Uuid,
    pub tenant_id: Uuid,
}

impl FromRequestParts<AppState> for AuthAdmin {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar
            .get(SESSION_COOKIE)
            .map(|c| c.value().to_string())
            .ok_or_else(|| AppError::Unauthorized("no session".into()))?;
        let hash = crate::auth::hash_token(&token);

        // The step-up flow rotates this token (docs/AUTH.md); the superseded
        // hash stays valid for a short grace so a request already in flight
        // with the old cookie — or a second tab — does not get thrown out.
        let row: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT admin_id, tenant_id FROM admin_sessions
             WHERE (token_hash = $1
                    OR (prev_token_hash = $1 AND prev_valid_until > now()))
               AND expires_at > now()",
        )
        .bind(&hash)
        .fetch_optional(&state.db)
        .await?;

        match row {
            Some((admin_id, tenant_id)) => Ok(AuthAdmin {
                admin_id,
                tenant_id,
            }),
            None => {
                // Opportunistic lazy cleanup of expired sessions.
                let _ = sqlx::query("DELETE FROM admin_sessions WHERE expires_at < now()")
                    .execute(&state.db)
                    .await;
                Err(AppError::Unauthorized("no session".into()))
            }
        }
    }
}

/// Extractor: a paired parent companion, via `Authorization: Bearer
/// <parent_token>`. The token is sha256-hashed and matched against
/// `parent_access_tokens` (not revoked). Scope is fixed by the `/api/parent/*`
/// routes that accept it — approve/deny earn-requests and read alerts, nothing
/// else. Touches `last_used_at` best-effort so the admin can see stale tokens.
#[derive(Clone, Copy)]
pub struct ParentAuth {
    #[allow(dead_code)] // carried for audit payloads / future per-token scoping
    pub token_id: Uuid,
    pub tenant_id: Uuid,
}

impl FromRequestParts<AppState> for ParentAuth {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::Unauthorized("missing bearer token".into()))?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AppError::Unauthorized("malformed authorization header".into()))?;
        let hash = crate::auth::hash_token(token);

        let row: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT id, tenant_id FROM parent_access_tokens
             WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(&hash)
        .fetch_optional(&state.db)
        .await?;

        let (token_id, tenant_id) =
            row.ok_or_else(|| AppError::Unauthorized("invalid parent token".into()))?;
        // Best-effort last-used stamp; never fail the request over it.
        let _ = sqlx::query("UPDATE parent_access_tokens SET last_used_at = now() WHERE id = $1")
            .bind(token_id)
            .execute(&state.db)
            .await;
        Ok(ParentAuth {
            token_id,
            tenant_id,
        })
    }
}

/// Extractor: an authenticated agent (device), via `Authorization: Bearer
/// <device_token>`. The token is sha256-hashed and matched against
/// `devices.device_token`.
#[derive(Clone, Copy)]
pub struct AgentAuth {
    pub device_id: Uuid,
    pub tenant_id: Uuid,
}

impl FromRequestParts<AppState> for AgentAuth {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::Unauthorized("missing bearer token".into()))?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AppError::Unauthorized("malformed authorization header".into()))?;
        let hash = crate::auth::hash_token(token);

        let row: Option<(Uuid, Uuid)> =
            sqlx::query_as("SELECT id, tenant_id FROM devices WHERE device_token = $1")
                .bind(&hash)
                .fetch_optional(&state.db)
                .await?;

        let (device_id, tenant_id) =
            row.ok_or_else(|| AppError::Unauthorized("invalid device token".into()))?;
        Ok(AgentAuth {
            device_id,
            tenant_id,
        })
    }
}
