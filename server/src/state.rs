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

pub const SESSION_COOKIE: &str = "sid";
pub const REG_COOKIE: &str = "reg_sid";
pub const AUTH_COOKIE: &str = "auth_sid";

/// An authenticated admin session.
#[derive(Clone, Copy)]
pub struct SessionData {
    pub admin_id: Uuid,
    pub tenant_id: Uuid,
}

/// Server-side WebAuthn registration challenge, keyed by a temp cookie.
pub struct RegChallenge {
    #[allow(dead_code)] // retained for audit / debugging of the ceremony
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub reg: PasskeyRegistration,
}

/// Server-side WebAuthn authentication challenge, keyed by a temp cookie.
pub struct AuthChallenge {
    pub admin_id: Uuid,
    pub tenant_id: Uuid,
    pub auth: PasskeyAuthentication,
}

/// In-memory bridge for one reverse-SSH session (skeleton).
///
/// Production path is a real `ssh -R` from the agent to the broker's embedded
/// SSH server (see TAMPER.md). Here we keep a channel that a server-side
/// `ws->pty` bridge / `sentinel ssh` CLI could attach to: bytes the agent
/// sends over the WS `ssh_data` frame are forwarded to `to_admin`.
#[allow(dead_code)] // device_id/broker_port kept for the production ssh -R bridge
pub struct SshBridge {
    pub device_id: Uuid,
    pub broker_port: i32,
    pub to_admin: mpsc::UnboundedSender<Vec<u8>>,
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

    pub async fn open_ssh(&self, session_id: Uuid, bridge: SshBridge) {
        self.ssh.write().await.insert(session_id, bridge);
    }

    pub async fn close_ssh(&self, session_id: Uuid) {
        self.ssh.write().await.remove(&session_id);
    }

    /// Forward a chunk of agent->admin SSH data to whoever is attached.
    pub async fn ssh_data_from_agent(&self, session_id: Uuid, data: Vec<u8>) {
        if let Some(b) = self.ssh.read().await.get(&session_id) {
            let _ = b.to_admin.send(data);
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub webauthn: Arc<Webauthn>,
    pub broker_host: String,
    pub sessions: Arc<RwLock<HashMap<String, SessionData>>>,
    pub reg_states: Arc<RwLock<HashMap<String, RegChallenge>>>,
    pub auth_states: Arc<RwLock<HashMap<String, AuthChallenge>>>,
    pub hub: Arc<Hub>,
}

impl AppState {
    pub async fn session_from_jar(&self, jar: &CookieJar) -> Option<SessionData> {
        let sid = jar.get(SESSION_COOKIE)?.value().to_string();
        self.sessions.read().await.get(&sid).copied()
    }
}

/// Extractor: an authenticated admin. Carries tenant_id so every downstream
/// query can scope by it.
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
        let s = state
            .session_from_jar(&jar)
            .await
            .ok_or_else(|| AppError::Unauthorized("no session".into()))?;
        Ok(AuthAdmin {
            admin_id: s.admin_id,
            tenant_id: s.tenant_id,
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
