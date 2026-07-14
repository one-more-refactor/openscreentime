//! Agent API (`/agent/*`): enrollment, heartbeat poll, policy pull, event push,
//! command ack, and the WebSocket bus (command push + event/ack + reverse-SSH
//! data frames). Auth via `Authorization: Bearer <device_token>` (except
//! enrollment, which consumes a one-time `enroll_token`).

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::Response,
    Json,
};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{gen_token, hash_token};
use crate::error::{AppError, AppResult};
use crate::events;
use crate::state::{AgentAuth, AppState, SshToAdmin};
use sentinel_policy::Policy;

/// Default poll interval handed to agents that fall back to heartbeat polling.
const POLL_INTERVAL_SECS: u64 = 15;

// ---------------------------------------------------------------------------
// Command queue
// ---------------------------------------------------------------------------

/// Enqueue a command for a device and, if the agent has a live WS, push it
/// immediately and mark it `sent`. Returns the command id plus whether the
/// frame was actually delivered to a live agent (false = stays `queued` until
/// the agent's next heartbeat/WS connect).
pub async fn enqueue_command_delivered(
    st: &AppState,
    device_id: Uuid,
    ctype: &str,
    payload: Value,
) -> AppResult<(Uuid, bool)> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO commands (device_id, type, payload) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(device_id)
    .bind(ctype)
    .bind(&payload)
    .fetch_one(&st.db)
    .await?;

    let frame = json!({
        "type": "command",
        "command": { "id": id, "type": ctype, "payload": payload }
    });
    let delivered = st.hub.push(device_id, frame).await;
    if delivered {
        sqlx::query("UPDATE commands SET status = 'sent' WHERE id = $1")
            .bind(id)
            .execute(&st.db)
            .await?;
    }
    Ok((id, delivered))
}

/// `enqueue_command_delivered` for call sites that don't care about delivery.
pub async fn enqueue_command(
    st: &AppState,
    device_id: Uuid,
    ctype: &str,
    payload: Value,
) -> AppResult<Uuid> {
    let (id, _delivered) = enqueue_command_delivered(st, device_id, ctype, payload).await?;
    Ok(id)
}

async fn default_profile_id(db: &sqlx::PgPool, tenant_id: Uuid) -> AppResult<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "SELECT id FROM profiles WHERE tenant_id = $1 AND kind = 'default' AND is_preset
         ORDER BY created_at LIMIT 1",
    )
    .bind(tenant_id)
    .fetch_one(db)
    .await?;
    Ok(id)
}

/// Derive a stable policy version string from the max `updated_at` across the
/// profiles assigned to a device's users.
async fn policy_version(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<String> {
    let ts: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT max(p.updated_at) FROM device_users du
         JOIN profiles p ON p.id = du.profile_id WHERE du.device_id = $1",
    )
    .bind(device_id)
    .fetch_one(db)
    .await?;
    Ok(match ts {
        Some(t) => t.timestamp_millis().to_string(),
        None => "0".to_string(),
    })
}

async fn upsert_os_users(
    db: &sqlx::PgPool,
    device_id: Uuid,
    default_profile: Uuid,
    users: &[OsUser],
) -> AppResult<()> {
    for u in users {
        if u.username.trim().is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT INTO device_users (device_id, os_username, display_name, profile_id)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (device_id, os_username) DO NOTHING",
        )
        .bind(device_id)
        .bind(&u.username)
        .bind(&u.display_name)
        .bind(default_profile)
        .execute(db)
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Enrollment
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct OsUser {
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Deserialize)]
pub struct EnrollReq {
    pub enroll_token: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub agent_version: String,
    #[serde(default)]
    pub os_users: Vec<OsUser>,
}

pub async fn enroll(
    State(st): State<AppState>,
    Json(req): Json<EnrollReq>,
) -> AppResult<Json<Value>> {
    // Consume the one-time enroll token. An expired token is rejected exactly
    // like a consumed one (24 h TTL; the admin can regenerate while pending).
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, tenant_id FROM devices WHERE enroll_token = $1
           AND (enroll_token_expires_at IS NULL OR enroll_token_expires_at > now())",
    )
    .bind(&req.enroll_token)
    .fetch_optional(&st.db)
    .await?;
    let (device_id, tenant_id) =
        row.ok_or_else(|| AppError::Unauthorized("invalid, used or expired enroll token".into()))?;

    let device_token = gen_token();
    let token_hash = hash_token(&device_token);

    sqlx::query(
        "UPDATE devices SET device_token = $1, enroll_token = NULL,
             enroll_token_expires_at = NULL, status = 'online',
             hostname = $2, os = $3, agent_version = $4, last_seen = now()
         WHERE id = $5",
    )
    .bind(&token_hash)
    .bind(&req.hostname)
    .bind(&req.os)
    .bind(&req.agent_version)
    .bind(device_id)
    .execute(&st.db)
    .await?;

    let default_profile = default_profile_id(&st.db, tenant_id).await?;
    upsert_os_users(&st.db, device_id, default_profile, &req.os_users).await?;

    events::insert(
        &st.db,
        tenant_id,
        Some(device_id),
        None,
        "enrolled",
        "info",
        json!({ "hostname": req.hostname, "os": req.os, "users": req.os_users.len() }),
    )
    .await?;

    Ok(Json(json!({
        "device_id": device_id,
        "device_token": device_token,
        "poll_interval_secs": POLL_INTERVAL_SECS,
    })))
}

// ---------------------------------------------------------------------------
// Heartbeat
// ---------------------------------------------------------------------------

/// Per-user screen-time usage reported with each heartbeat.
#[derive(Deserialize)]
pub struct UsageEntry {
    pub os_username: String,
    pub used_minutes_today: i32,
}

/// Upsert today's per-user usage into the screen-time ledger. Shared by the HTTP
/// heartbeat and the WS `heartbeat` frame so both report identically.
async fn upsert_usage(
    db: &sqlx::PgPool,
    device_id: Uuid,
    usage: &[UsageEntry],
) -> Result<(), sqlx::Error> {
    for u in usage {
        sqlx::query(
            // used_seconds is monotonic within a day: take the max so an agent
            // whose in-memory counter reset (reboot / process restart) reports a
            // low number and can't erase the day's real total.
            "INSERT INTO screen_time_ledger (device_user_id, day, used_seconds)
             SELECT du.id, CURRENT_DATE, $3 FROM device_users du
             WHERE du.device_id = $1 AND du.os_username = $2
             ON CONFLICT (device_user_id, day)
             DO UPDATE SET used_seconds = GREATEST(screen_time_ledger.used_seconds, EXCLUDED.used_seconds)",
        )
        .bind(device_id)
        .bind(&u.os_username)
        .bind(u.used_minutes_today.max(0) * 60)
        .execute(db)
        .await?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct HeartbeatReq {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub public_ip: Option<String>,
    #[serde(default)]
    pub usage: Vec<UsageEntry>,
    #[serde(default)]
    pub os_users: Vec<OsUser>,
}

pub async fn heartbeat(
    State(st): State<AppState>,
    agent: AgentAuth,
    Json(req): Json<HeartbeatReq>,
) -> AppResult<Json<Value>> {
    // Never clobber a `locked` status from a heartbeat.
    let reported = req.status.unwrap_or_else(|| "online".into());
    sqlx::query(
        "UPDATE devices SET last_seen = now(),
             public_ip = COALESCE($2::inet, public_ip),
             status = CASE WHEN status = 'locked' THEN status ELSE $3 END
         WHERE id = $1",
    )
    .bind(agent.device_id)
    .bind(req.public_ip)
    .bind(reported)
    .execute(&st.db)
    .await?;

    if !req.os_users.is_empty() {
        let default_profile = default_profile_id(&st.db, agent.tenant_id).await?;
        upsert_os_users(&st.db, agent.device_id, default_profile, &req.os_users).await?;
    }

    // Persist today's per-user usage into the screen-time ledger.
    upsert_usage(&st.db, agent.device_id, &req.usage).await?;

    // Return queued/sent (undelivered-or-unacked) commands and mark them sent.
    let cmds = pull_pending_commands(&st.db, agent.device_id).await?;
    let version = policy_version(&st.db, agent.device_id).await?;

    Ok(Json(json!({
        "commands": cmds,
        "policy_version": version,
    })))
}

async fn pull_pending_commands(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<Vec<Value>> {
    let rows: Vec<(Uuid, String, Value)> = sqlx::query_as(
        "SELECT id, type, payload FROM commands
         WHERE device_id = $1 AND status IN ('queued','sent')
         ORDER BY created_at",
    )
    .bind(device_id)
    .fetch_all(db)
    .await?;

    // Mark queued ones sent.
    sqlx::query("UPDATE commands SET status = 'sent' WHERE device_id = $1 AND status = 'queued'")
        .bind(device_id)
        .execute(db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|(id, ctype, payload)| json!({ "id": id, "type": ctype, "payload": payload }))
        .collect())
}

// ---------------------------------------------------------------------------
// Policy pull
// ---------------------------------------------------------------------------

pub async fn policy(State(st): State<AppState>, agent: AgentAuth) -> AppResult<Json<Value>> {
    let tamper_level: i32 = sqlx::query_scalar("SELECT tamper_level FROM devices WHERE id = $1")
        .bind(agent.device_id)
        .fetch_one(&st.db)
        .await?;

    let rows: Vec<(String, String, Value)> = sqlx::query_as(
        "SELECT du.os_username, p.kind, p.policy FROM device_users du
         JOIN profiles p ON p.id = du.profile_id WHERE du.device_id = $1
         ORDER BY du.os_username",
    )
    .bind(agent.device_id)
    .fetch_all(&st.db)
    .await?;

    let users: Vec<Value> = rows
        .into_iter()
        .map(|(os_username, kind, policy)| {
            // Round-trip through the shared type for forward-compat normalization.
            let normalized: Policy = serde_json::from_value(policy).unwrap_or_default();
            json!({
                "os_username": os_username,
                "profile_kind": kind,
                "policy": normalized,
            })
        })
        .collect();

    let version = policy_version(&st.db, agent.device_id).await?;
    Ok(Json(json!({
        "policy_version": version,
        "device_tamper_level": tamper_level,
        "users": users,
    })))
}

// ---------------------------------------------------------------------------
// Events push
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PushEvent {
    pub r#type: String,
    #[serde(default = "default_severity")]
    pub severity: String,
    #[serde(default)]
    pub device_user: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

fn default_severity() -> String {
    "info".into()
}

#[derive(Deserialize)]
pub struct PushEventsReq {
    pub events: Vec<PushEvent>,
}

pub async fn push_events(
    State(st): State<AppState>,
    agent: AgentAuth,
    Json(req): Json<PushEventsReq>,
) -> AppResult<axum::http::StatusCode> {
    for ev in req.events {
        let device_user_id = resolve_device_user(&st.db, agent.device_id, ev.device_user).await?;
        events::insert(
            &st.db,
            agent.tenant_id,
            Some(agent.device_id),
            device_user_id,
            &ev.r#type,
            &ev.severity,
            ev.payload,
        )
        .await?;
    }
    Ok(axum::http::StatusCode::ACCEPTED)
}

async fn resolve_device_user(
    db: &sqlx::PgPool,
    device_id: Uuid,
    os_username: Option<String>,
) -> AppResult<Option<Uuid>> {
    let Some(username) = os_username else {
        return Ok(None);
    };
    let id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM device_users WHERE device_id = $1 AND os_username = $2")
            .bind(device_id)
            .bind(username)
            .fetch_optional(db)
            .await?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Command ack
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AckReq {
    pub status: String,
    #[serde(default)]
    pub result: Option<Value>,
}

/// Apply a command ack (shared by the HTTP endpoint and the WS `ack` frame).
/// Any status other than "failed" is normalized to "acked". Returns whether a
/// matching command row was updated.
///
/// Truthful lock state: `devices.status` only flips to `locked`/`online` when
/// the agent confirms a `lock`/`unlock` — this is where a lock that was merely
/// queued (device offline at click time) becomes real once applied.
async fn apply_command_ack(
    db: &sqlx::PgPool,
    device_id: Uuid,
    command_id: Uuid,
    status: &str,
    result: Option<&Value>,
) -> AppResult<bool> {
    let status = if status == "failed" {
        "failed"
    } else {
        "acked"
    };
    let ctype: Option<String> = sqlx::query_scalar(
        "UPDATE commands SET status = $1, result = $2, acked_at = now()
         WHERE id = $3 AND device_id = $4 RETURNING type",
    )
    .bind(status)
    .bind(result)
    .bind(command_id)
    .bind(device_id)
    .fetch_optional(db)
    .await?;

    if status == "acked" {
        match ctype.as_deref() {
            Some("lock") => {
                sqlx::query("UPDATE devices SET status = 'locked' WHERE id = $1")
                    .bind(device_id)
                    .execute(db)
                    .await?;
            }
            Some("unlock") => {
                sqlx::query("UPDATE devices SET status = 'online' WHERE id = $1")
                    .bind(device_id)
                    .execute(db)
                    .await?;
            }
            _ => {}
        }
    }
    Ok(ctype.is_some())
}

pub async fn ack_command(
    State(st): State<AppState>,
    agent: AgentAuth,
    Path(command_id): Path<Uuid>,
    Json(req): Json<AckReq>,
) -> AppResult<Json<Value>> {
    let updated = apply_command_ack(
        &st.db,
        agent.device_id,
        command_id,
        &req.status,
        req.result.as_ref(),
    )
    .await?;
    if !updated {
        return Err(AppError::NotFound("command not found".into()));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// WebSocket bus
// ---------------------------------------------------------------------------

pub async fn ws(
    State(st): State<AppState>,
    agent: AgentAuth,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| handle_ws(st, agent, socket))
}

async fn handle_ws(st: AppState, agent: AgentAuth, socket: WebSocket) {
    let device_id = agent.device_id;
    let (mut sink, mut stream) = socket.split();

    // Channel the hub uses to push frames to this agent.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    st.hub.register_agent(device_id, tx).await;

    // Mark online.
    let _ = sqlx::query(
        "UPDATE devices SET status = CASE WHEN status='locked' THEN status ELSE 'online' END, \
         last_seen = now() WHERE id = $1",
    )
    .bind(device_id)
    .execute(&st.db)
    .await;

    // Writer task: forward hub frames to the socket.
    let mut writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if sink
                .send(Message::Text(frame.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Push any already-queued commands on connect.
    if let Ok(cmds) = pull_pending_commands(&st.db, device_id).await {
        for c in cmds {
            let _ = st
                .hub
                .push(device_id, json!({ "type": "command", "command": c }))
                .await;
        }
    }

    // Reader loop.
    loop {
        tokio::select! {
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(v) = serde_json::from_str::<Value>(t.as_str()) {
                            handle_ws_frame(&st, agent, v).await;
                        }
                    }
                    Some(Ok(Message::Binary(_))) => { /* ignore in skeleton */ }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                }
            }
            _ = &mut writer => break,
        }
    }

    writer.abort();
    st.hub.unregister_agent(device_id).await;
    let _ = sqlx::query(
        "UPDATE devices SET status = CASE WHEN status='locked' THEN status ELSE 'offline' END \
         WHERE id = $1",
    )
    .bind(device_id)
    .execute(&st.db)
    .await;
}

/// Handle one inbound WS frame from an agent. The envelope is tagged with
/// `type` (the agent's `AgentFrame`, per CONTRACT-PROD.md §3); the legacy
/// `kind` tag is accepted as an alias.
///
/// Frame types:
///   - `{ type:"event", event:{ type, severity?, device_user?, payload } }`
///     (fields at the top level also accepted)
///   - `{ type:"ack", ack:{ command_id, status, result? } }` (idem)
///   - `{ type:"ssh_data", session_id, data_b64 }` — base64 raw PTY output
///   - `{ type:"ssh_closed", session_id, exit_code? }` (`ssh_close` accepted)
///   - `{ type:"heartbeat" }` / `{ type:"pong" }`
async fn handle_ws_frame(st: &AppState, agent: AgentAuth, v: Value) {
    let kind = v
        .get("kind")
        .or_else(|| v.get("type"))
        .and_then(|k| k.as_str());
    match kind {
        Some("event") => {
            // The agent nests the event under "event"; tolerate flat frames.
            let ev = v.get("event").unwrap_or(&v);
            let etype = ev
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("heartbeat");
            let severity = ev
                .get("severity")
                .and_then(|s| s.as_str())
                .unwrap_or("info");
            let device_user = ev
                .get("device_user")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string());
            let payload = ev.get("payload").cloned().unwrap_or_else(|| json!({}));
            if let Ok(device_user_id) =
                resolve_device_user(&st.db, agent.device_id, device_user).await
            {
                let _ = events::insert(
                    &st.db,
                    agent.tenant_id,
                    Some(agent.device_id),
                    device_user_id,
                    etype,
                    severity,
                    payload,
                )
                .await;
            }
        }
        Some("ack") => {
            let ack = v.get("ack").unwrap_or(&v);
            if let Some(cmd_id) = ack
                .get("command_id")
                .and_then(|c| c.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                let status = ack
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("acked");
                let result = ack.get("result").cloned();
                let _ = apply_command_ack(&st.db, agent.device_id, cmd_id, status, result.as_ref())
                    .await;
            }
        }
        Some("ssh_data") => {
            let Some(session_id) = frame_session_id(&v) else {
                return;
            };
            let Some(data) = v
                .get("data_b64")
                .and_then(|d| d.as_str())
                .and_then(|s| B64.decode(s).ok())
            else {
                return;
            };
            ssh_frame_from_agent(st, agent, session_id, SshToAdmin::Data(data)).await;
        }
        Some("ssh_closed") | Some("ssh_close") => {
            let Some(session_id) = frame_session_id(&v) else {
                return;
            };
            let exit_code = v.get("exit_code").and_then(|e| e.as_i64());
            ssh_frame_from_agent(st, agent, session_id, SshToAdmin::Closed(exit_code)).await;
            // The agent side is gone: mark the row closed and drop the bridge.
            let _ = sqlx::query(
                "UPDATE ssh_sessions SET status = 'closed', closed_at = now()
                 WHERE id = $1 AND device_id = $2 AND status IN ('opening','open')",
            )
            .bind(session_id)
            .bind(agent.device_id)
            .execute(&st.db)
            .await;
            st.hub.close_ssh(session_id, Some(agent.device_id)).await;
            // Tell the agent to drop its session handle (PTY fd + writer thread);
            // a self-exited shell would otherwise leak them until an explicit close.
            st.hub
                .push(
                    agent.device_id,
                    json!({ "type": "ssh_close", "session_id": session_id }),
                )
                .await;
            let _ = events::insert(
                &st.db,
                agent.tenant_id,
                Some(agent.device_id),
                None,
                "ssh",
                "info",
                json!({ "action": "ssh_closed", "session_id": session_id, "exit_code": exit_code }),
            )
            .await;
        }
        Some("heartbeat") => {
            let _ = sqlx::query("UPDATE devices SET last_seen = now() WHERE id = $1")
                .bind(agent.device_id)
                .execute(&st.db)
                .await;
            // A WS-connected agent has no HTTP heartbeat; persist its usage here so
            // the ledger stays current in the normal (non-poll) path.
            if let Some(usage) = v.get("usage") {
                if let Ok(entries) = serde_json::from_value::<Vec<UsageEntry>>(usage.clone()) {
                    let _ = upsert_usage(&st.db, agent.device_id, &entries).await;
                }
            }
        }
        _ => {}
    }
}

fn frame_session_id(v: &Value) -> Option<Uuid> {
    v.get("session_id")
        .and_then(|s| s.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// Route an agent-side SSH frame into the bridge; the agent's first frame for
/// a session confirms it (`opening` -> `open`).
async fn ssh_frame_from_agent(st: &AppState, agent: AgentAuth, session_id: Uuid, msg: SshToAdmin) {
    match st
        .hub
        .ssh_from_agent(session_id, agent.device_id, msg)
        .await
    {
        Some(true) => {
            let _ = sqlx::query(
                "UPDATE ssh_sessions SET status = 'open'
                 WHERE id = $1 AND device_id = $2 AND status = 'opening'",
            )
            .bind(session_id)
            .bind(agent.device_id)
            .execute(&st.db)
            .await;
        }
        Some(false) => {}
        None => {
            tracing::debug!(%session_id, "ssh frame for unknown session");
        }
    }
}
