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

pub const SESSION_COOKIE: &str = "sentinel_session";
pub const REG_COOKIE: &str = "reg_sid";
pub const AUTH_COOKIE: &str = "auth_sid";

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

/// A frame the SSH bridge delivers to the attached admin terminal.
pub enum SshToAdmin {
    /// Raw terminal output bytes (already base64-decoded).
    Data(Vec<u8>),
    /// The agent-side shell exited (or the session was closed server-side).
    Closed(Option<i64>),
}

/// Cap on bytes buffered per SSH session while no admin terminal is attached.
const SSH_BACKLOG_MAX_BYTES: usize = 256 * 1024;

/// In-memory bridge for one reverse-SSH session: agent WS frames on one side,
/// the admin's browser-terminal WS on the other. Output that arrives before
/// the admin attaches is buffered (bounded).
struct SshBridge {
    device_id: Uuid,
    /// Set once the agent has confirmed the session with its first frame.
    confirmed: bool,
    to_admin: Option<mpsc::UnboundedSender<SshToAdmin>>,
    backlog: Vec<SshToAdmin>,
    backlog_bytes: usize,
}

/// Hub of live agent WebSocket connections + active SSH bridges.
#[derive(Default)]
pub struct Hub {
    /// device_id -> sender that writes JSON frames to that agent's socket.
    agents: RwLock<HashMap<Uuid, mpsc::UnboundedSender<serde_json::Value>>>,
    /// ssh_session_id -> bridge.
    ssh: RwLock<HashMap<Uuid, SshBridge>>,
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

    /// Register a bridge for a freshly-created (still `opening`) SSH session.
    pub async fn open_ssh(&self, session_id: Uuid, device_id: Uuid) {
        self.ssh.write().await.insert(
            session_id,
            SshBridge {
                device_id,
                confirmed: false,
                to_admin: None,
                backlog: Vec::new(),
                backlog_bytes: 0,
            },
        );
    }

    /// Attach the admin terminal to a session; drains buffered agent output
    /// into `tx`. Returns the session's device_id, or None if unknown.
    pub async fn attach_ssh_admin(
        &self,
        session_id: Uuid,
        tx: mpsc::UnboundedSender<SshToAdmin>,
    ) -> Option<Uuid> {
        let mut ssh = self.ssh.write().await;
        let bridge = ssh.get_mut(&session_id)?;
        for msg in bridge.backlog.drain(..) {
            let _ = tx.send(msg);
        }
        bridge.backlog_bytes = 0;
        bridge.to_admin = Some(tx);
        Some(bridge.device_id)
    }

    /// Deliver an agent-side frame to the attached admin (or buffer it).
    /// Returns `Some(newly_confirmed)` if the session is known AND owned by
    /// `from_device` — the first agent frame confirms the session
    /// (`opening` -> `open`). A frame whose `session_id` belongs to a different
    /// device is rejected (`None`), so one agent can't inject into or confirm
    /// another device's terminal by guessing its session id.
    pub async fn ssh_from_agent(
        &self,
        session_id: Uuid,
        from_device: Uuid,
        msg: SshToAdmin,
    ) -> Option<bool> {
        let mut ssh = self.ssh.write().await;
        let bridge = ssh.get_mut(&session_id)?;
        if bridge.device_id != from_device {
            tracing::warn!(%session_id, "ssh frame from wrong device, dropping");
            return None;
        }
        let newly_confirmed = !bridge.confirmed;
        bridge.confirmed = true;
        match &bridge.to_admin {
            Some(tx) => {
                let _ = tx.send(msg);
            }
            None => {
                let size = match &msg {
                    SshToAdmin::Data(d) => d.len(),
                    SshToAdmin::Closed(_) => 0,
                };
                if bridge.backlog_bytes + size <= SSH_BACKLOG_MAX_BYTES {
                    bridge.backlog_bytes += size;
                    bridge.backlog.push(msg);
                } else {
                    tracing::warn!(%session_id, "ssh backlog full, dropping agent frame");
                }
            }
        }
        Some(newly_confirmed)
    }

    /// Tear down a bridge; notifies an attached admin terminal, if any. When
    /// `from_device` is set, only tears down the session if that device owns it
    /// (agent-initiated close); admin/server-initiated closes pass `None`.
    pub async fn close_ssh(&self, session_id: Uuid, from_device: Option<Uuid>) {
        let mut ssh = self.ssh.write().await;
        if let Some(bridge) = ssh.get(&session_id) {
            if let Some(dev) = from_device {
                if bridge.device_id != dev {
                    tracing::warn!(%session_id, "ssh close from wrong device, ignoring");
                    return;
                }
            }
        }
        if let Some(bridge) = ssh.remove(&session_id) {
            if let Some(tx) = bridge.to_admin {
                let _ = tx.send(SshToAdmin::Closed(None));
            }
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub webauthn: Arc<Webauthn>,
    /// Session cookies are `Secure` unless SENTINEL_INSECURE_COOKIES=1 (dev).
    pub cookie_secure: bool,
    /// Public base URL (SENTINEL_PUBLIC_URL, falls back to the WebAuthn RP
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

        let row: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT admin_id, tenant_id FROM admin_sessions
             WHERE token_hash = $1 AND expires_at > now()",
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
