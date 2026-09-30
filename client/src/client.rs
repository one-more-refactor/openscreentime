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
    /// The login the install ran from (`SUDO_USER`), if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installer: Option<String>,
    /// The login the person at the keyboard picked as the owner's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_login: Option<String>,
    /// Which machine this is, for this household only (`enroll::machine_hash`):
    /// enrolling it again folds its older record in, so a day isn't counted
    /// twice. Never the raw machine-id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EnrollResponse {
    pub device_id: String,
    pub device_token: String,
    #[serde(default = "default_poll")]
    pub poll_interval_secs: u64,
    /// Who each OS login turned out to be.
    #[serde(default)]
    pub users: Vec<EnrolledUser>,
}

#[derive(Debug, Deserialize)]
pub struct EnrolledUser {
    pub os_username: String,
    pub person: String,
    #[serde(default)]
    pub parent: bool,
}

/// Whose computer an enroll token is for (`/agent/enroll/preview`).
#[derive(Debug, Default, Deserialize)]
pub struct EnrollPreview {
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub owner_is_parent: bool,
    /// The household's key for the machine identity (servers from 0.7).
    #[serde(default)]
    pub machine_salt: Option<String>,
}

fn default_poll() -> u64 {
    30
}

/// POST /agent/enroll/preview — whose computer this is, without using the
/// token up. Older servers don't have it; the caller treats any error as
/// "don't ask".
pub async fn enroll_preview(base_url: &str, token: &str) -> Result<EnrollPreview> {
    let base = base_url.trim_end_matches('/');
    let http = reqwest::Client::builder()
        .user_agent(format!("openscreentime/{AGENT_VERSION}"))
        .build()?;
    let resp = http
        .post(format!("{base}/agent/enroll/preview"))
        .json(&json!({ "enroll_token": token }))
        .send()
        .await
        .context("POST /agent/enroll/preview")?
        .error_for_status()?;
    Ok(resp.json().await?)
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
    /// The person's day on their other computers (see `PersonDay`).
    #[serde(default)]
    pub usage: Vec<crate::protocol::PersonDay>,
    /// The server's clock — a time source the agent trusts.
    #[serde(default)]
    pub server_time: Option<chrono::DateTime<chrono::Utc>>,
}

pub use crate::protocol::UsageReport;

/// The server said this computer was removed from its household: `410
/// device_retired` with `"retired": true`, from the configured server. The
/// one answer on which the agent takes itself off the computer — never a
/// plain 401, a network error, or an answer from anywhere else.
#[derive(Debug, thiserror::Error)]
#[error("this computer was removed from its household")]
pub struct Retired;

/// Is this error the server retiring this computer?
pub fn is_retired(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<Retired>().is_some())
}

/// The retirement answer, exactly: status 410, from the configured server's
/// origin, with the flag set in a JSON body.
fn retirement_answer(status: u16, same_origin: bool, body: &[u8]) -> bool {
    status == 410
        && same_origin
        && serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|v| v.get("retired").and_then(Value::as_bool))
            == Some(true)
}

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
    /// `error_for_status`, except that the retirement answer becomes
    /// [`Retired`]. Only an answer whose final URL (after any redirect) is
    /// on the configured server counts.
    async fn checked(&self, resp: reqwest::Response) -> Result<reqwest::Response> {
        if resp.status() == reqwest::StatusCode::GONE {
            let same_origin = reqwest::Url::parse(&self.base)
                .is_ok_and(|base| base.origin() == resp.url().origin());
            let body = resp.bytes().await.unwrap_or_default();
            if retirement_answer(410, same_origin, &body) {
                return Err(Retired.into());
            }
            anyhow::bail!("server answered 410 Gone");
        }
        Ok(resp.error_for_status()?)
    }
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
            "features": crate::protocol::FEATURES,
        });
        let resp = self
            .http
            .post(format!("{}/agent/heartbeat", self.base))
            .header("Authorization", self.bearer())
            .json(&body)
            .send()
            .await
            .context("POST /agent/heartbeat")?;
        let resp = self.checked(resp).await?;
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

    /// POST /agent/usage — where-the-time-goes slices (CONTRACT-0.6 §3).
    pub async fn post_usage_slices(&self, slices: &[serde_json::Value]) -> Result<()> {
        let resp = self
            .http
            .post(format!("{}/agent/usage", self.base))
            .header("Authorization", self.bearer())
            .json(&json!({ "slices": slices }))
            .send()
            .await
            .context("POST /agent/usage")?;
        self.checked(resp).await?;
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
            .context("GET /agent/policy")?;
        let resp = self.checked(resp).await?;
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
        let resp = self
            .http
            .post(format!("{}/agent/events", self.base))
            .header("Authorization", self.bearer())
            .json(&json!({ "events": events }))
            .send()
            .await
            .context("POST /agent/events")?;
        self.checked(resp).await?;
        Ok(())
    }

    /// POST /agent/commands/:id/ack
    pub async fn ack_command(&self, ack: &CommandAck) -> Result<()> {
        let resp = self
            .http
            .post(format!(
                "{}/agent/commands/{}/ack",
                self.base, ack.command_id
            ))
            .header("Authorization", self.bearer())
            .json(&json!({ "status": ack.status, "result": ack.result }))
            .send()
            .await
            .context("POST command ack")?;
        self.checked(resp).await?;
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
        // Bounded like every HTTP call: a blackholed server must not park the
        // reconnect loop forever.
        let connected = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .context("ws connect timed out")?;
        match connected {
            Ok((stream, _resp)) => Ok(stream),
            // The upgrade is refused with the same answer as any other call;
            // the handshake goes to the configured URL itself (no redirects).
            Err(tokio_tungstenite::tungstenite::Error::Http(resp))
                if retirement_answer(
                    resp.status().as_u16(),
                    true,
                    resp.body().as_deref().unwrap_or_default(),
                ) =>
            {
                Err(Retired.into())
            }
            Err(e) => Err(anyhow::Error::from(e).context("ws connect")),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the exact retirement answer from the configured server retires
    /// the computer: not a 401 (a hiccup, a proxy, a bad token), not a 410
    /// without the flag, not a 410 from somewhere a redirect led.
    #[test]
    fn only_the_servers_retirement_answer_counts() {
        let retired = br#"{"error":{"code":"device_retired","message":"x"},"retired":true}"#;
        assert!(retirement_answer(410, true, retired));
        assert!(!retirement_answer(410, false, retired), "another origin");
        assert!(
            !retirement_answer(401, true, retired),
            "a 401 never retires"
        );
        assert!(!retirement_answer(
            410,
            true,
            br#"{"error":{"code":"code_expired"}}"#
        ));
        assert!(!retirement_answer(410, true, br#"{"retired":"yes"}"#));
        assert!(!retirement_answer(410, true, b"<html>Gone</html>"));
        assert!(is_retired(
            &anyhow::Error::from(Retired).context("GET /agent/policy")
        ));
        assert!(!is_retired(&anyhow::anyhow!("401 Unauthorized")));
    }
}
