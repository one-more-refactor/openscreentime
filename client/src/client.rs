//! HTTP + WebSocket transport to the Sentinel server (the `/agent/*` surface in
//! `docs/API.md`). Auth is a bearer `device_token` on every call except enrollment.

use crate::protocol::{Command, CommandAck, Event};
use crate::sysusers::OsUser;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

// ---- Enrollment (no token yet) -------------------------------------------------

#[derive(Debug, Serialize)]
pub struct EnrollRequest {
    pub enroll_token: String,
    pub hostname: String,
    pub os: String,
    pub agent_version: String,
    pub os_users: Vec<OsUser>,
}

#[derive(Debug, Deserialize)]
pub struct EnrollResponse {
    pub device_id: String,
    pub device_token: String,
    #[serde(default = "default_poll")]
    pub poll_interval_secs: u64,
}

fn default_poll() -> u64 {
    30
}

/// POST /agent/enroll — consumes the one-time enroll token, returns identity.
pub async fn enroll(base_url: &str, req: &EnrollRequest) -> Result<EnrollResponse> {
    let base = base_url.trim_end_matches('/');
    let http = reqwest::Client::builder()
        .user_agent(format!("sentinel-agent/{AGENT_VERSION}"))
        .build()?;
    let resp = http
        .post(format!("{base}/agent/enroll"))
        .json(req)
        .send()
        .await
        .context("POST /agent/enroll")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("enroll failed ({status}): {text}");
    }
    serde_json::from_str(&text).with_context(|| format!("decoding enroll response: {text}"))
}

// ---- Authenticated client ------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct HeartbeatResponse {
    #[serde(default)]
    pub commands: Vec<Command>,
    #[serde(default)]
    pub policy_version: String,
}

#[derive(Clone)]
pub struct ServerClient {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl ServerClient {
    pub fn new(base_url: &str, token: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("sentinel-agent/{AGENT_VERSION}"))
            .build()?;
        Ok(ServerClient {
            http,
            base: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
        })
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// POST /agent/heartbeat — poll fallback when the WS bus is down.
    pub async fn heartbeat(
        &self,
        status: &str,
        public_ip: Option<&str>,
        os_users: &[OsUser],
    ) -> Result<HeartbeatResponse> {
        let body = json!({
            "status": status,
            "public_ip": public_ip,
            "os_users": os_users,
        });
        let resp = self
            .http
            .post(format!("{}/agent/heartbeat", self.base))
            .header("Authorization", self.bearer())
            .json(&body)
            .send()
            .await
            .context("POST /agent/heartbeat")?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// GET /agent/policy — the full per-user policy bundle.
    pub async fn get_policy(&self) -> Result<crate::policy::PolicyBundle> {
        let resp = self
            .http
            .get(format!("{}/agent/policy", self.base))
            .header("Authorization", self.bearer())
            .send()
            .await
            .context("GET /agent/policy")?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// POST /agent/events — buffered telemetry & audit.
    pub async fn post_events(&self, events: &[Event]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        self.http
            .post(format!("{}/agent/events", self.base))
            .header("Authorization", self.bearer())
            .json(&json!({ "events": events }))
            .send()
            .await
            .context("POST /agent/events")?
            .error_for_status()?;
        Ok(())
    }

    /// POST /agent/commands/:id/ack
    pub async fn ack_command(&self, ack: &CommandAck) -> Result<()> {
        self.http
            .post(format!(
                "{}/agent/commands/{}/ack",
                self.base, ack.command_id
            ))
            .header("Authorization", self.bearer())
            .json(&json!({ "status": ack.status, "result": ack.result }))
            .send()
            .await
            .context("POST command ack")?
            .error_for_status()?;
        Ok(())
    }

    /// Open the WS bus (GET /agent/ws upgrade) with the bearer token as a header.
    pub async fn connect_ws(&self) -> Result<WsStream> {
        let ws_url = self
            .base
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let url = format!("{ws_url}/agent/ws");
        let mut request = url.into_client_request().context("building ws request")?;
        request
            .headers_mut()
            .insert("Authorization", self.bearer().parse()?);
        let (stream, _resp) = tokio_tungstenite::connect_async(request)
            .await
            .context("ws connect")?;
        Ok(stream)
    }
}

/// Best-effort public IP host extracted from the server URL (used by the firewall
/// allowlist so the agent can always reach home).
pub fn server_host(base_url: &str) -> Option<String> {
    let after = base_url.split("://").nth(1)?;
    let host = after.split('/').next()?.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}
