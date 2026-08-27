//! HTTP + WebSocket transport to the OpenScreenTime server (the `/agent/*` surface in
//! `docs/API.md`). Auth is a bearer `device_token` on every call except enrollment.

use crate::protocol::{Command, CommandAck, Event};
use crate::sysusers::OsUser;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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
        .user_agent(format!("openscreentime/{AGENT_VERSION}"))
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

pub use crate::protocol::UsageReport;

/// `POST /agent/earn-request` response (CONTRACT-PROD.md §4).
#[derive(Debug, Deserialize)]
pub struct EarnRequestResponse {
    pub request: EarnRequestInfo,
}

#[derive(Debug, Deserialize)]
pub struct EarnRequestInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub status: String,
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
            .user_agent(format!("openscreentime/{AGENT_VERSION}"))
            // Bound every request. Without this, a blackholed server stalls the
            // caller indefinitely — including the earn-request POST that runs
            // inside the enforcement tick on the WS select loop, which would
            // otherwise wedge enforcement and frame processing behind one hung call.
            .timeout(std::time::Duration::from_secs(10))
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

    /// POST /agent/heartbeat — poll fallback when the WS bus is down. `usage`
    /// carries each managed user's used minutes today (CONTRACT-PROD.md §5); the
    /// server upserts it into `screen_time_ledger`.
    pub async fn heartbeat(
        &self,
        status: &str,
        public_ip: Option<&str>,
        os_users: &[OsUser],
        usage: &[UsageReport],
    ) -> Result<HeartbeatResponse> {
        let body = json!({
            "status": status,
            "public_ip": public_ip,
            "os_users": os_users,
            "usage": usage,
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

    /// POST /agent/earn-request — auto-requested when a lockout engages and an
    /// earn-time offer is available (CONTRACT-PROD.md §4).
    pub async fn post_earn_request(
        &self,
        os_username: &str,
        task_id: &str,
        task_label: &str,
        minutes: u32,
    ) -> Result<EarnRequestResponse> {
        let body = json!({
            "os_username": os_username,
            "task_id": task_id,
            "task_label": task_label,
            "minutes": minutes,
        });
        let resp = self
            .http
            .post(format!("{}/agent/earn-request", self.base))
            .header("Authorization", self.bearer())
            .json(&body)
            .send()
            .await
            .context("POST /agent/earn-request")?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// POST /agent/login-decision — the human at this machine answered a
    /// web sign-in prompt (CONTRACT-0.6 client-first login).
    pub async fn post_login_decision(
        &self,
        request_id: &str,
        approve: bool,
        os_username: &str,
    ) -> Result<()> {
        let body = json!({
            "request_id": request_id,
            "approve": approve,
            "os_username": os_username,
        });
        self.http
            .post(format!("{}/agent/login-decision", self.base))
            .header("Authorization", self.bearer())
            .json(&body)
            .send()
            .await
            .context("POST /agent/login-decision")?
            .error_for_status()?;
        Ok(())
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

    /// POST /agent/voucher — a one-time, two-minute token that a local browser
    /// exchanges for a session on this machine (`ost login`).
    ///
    /// Returns the voucher and its lifetime in seconds. The voucher is a live
    /// credential for as long as it lasts, so it is never logged here.
    ///
    /// `os_username` is the desktop user asking: the server binds the voucher
    /// to the *account* that OS login belongs to, so a child's machine signs
    /// the child in — never the parent.
    pub async fn mint_voucher(&self, os_username: &str) -> Result<(String, u64)> {
        let res: Value = self
            .http
            .post(format!("{}/agent/voucher", self.base))
            .header("Authorization", self.bearer())
            .json(&json!({ "os_username": os_username }))
            .send()
            .await
            .context("POST /agent/voucher")?
            .error_for_status()?
            .json()
            .await
            .context("reading the voucher response")?;

        let voucher = res
            .get("voucher")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("server returned no voucher"))?
            .to_string();
        let expires = res
            .get("expires_in_secs")
            .and_then(Value::as_u64)
            .unwrap_or(120);
        Ok((voucher, expires))
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
