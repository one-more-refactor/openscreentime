//! Agent API (`/agent/*`): enrollment, heartbeat poll, policy pull, event push,
//! command ack, and the WebSocket bus (command push + event/ack
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
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{gen_token, hash_token};
use crate::error::{AppError, AppResult};
use crate::events;
use crate::state::{AgentAuth, AppState};
use openscreentime_policy::Policy;

/// Default poll interval handed to agents that fall back to heartbeat polling.
const POLL_INTERVAL_SECS: u64 = 15;

// ---------------------------------------------------------------------------
// Command queue
// ---------------------------------------------------------------------------

/// Command types where at most one pending (queued|sent) instance per device
/// makes sense; enqueueing again coalesces into the pending row (payload
/// refreshed) instead of stacking duplicates. `credit_time`/`deny_earn` are
/// deliberately absent — every grant is a distinct command.
/// Mirrors the partial unique index in migration 0009.
const COALESCE_TYPES: &[&str] = &["lock", "unlock", "apply_policy", "set_tamper_level"];

/// The id of a pending (queued|sent) command of this type, if one exists.
pub async fn pending_command(
    db: &sqlx::PgPool,
    device_id: Uuid,
    ctype: &str,
) -> AppResult<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM commands
         WHERE device_id = $1 AND type = $2 AND status IN ('queued','sent')",
    )
    .bind(device_id)
    .bind(ctype)
    .fetch_optional(db)
    .await?)
}

/// Enqueue a command for a device and, if the agent has a live WS, push it
/// immediately and mark it `sent`. Returns the command id plus whether the
/// frame was actually delivered to a live agent (false = stays `queued` until
/// the agent's next heartbeat/WS connect).
///
/// For `COALESCE_TYPES` this is an upsert against the one-pending-per-type
/// guard: a duplicate enqueue refreshes the pending row's payload and re-pushes
/// it, it never stacks a second copy.
pub async fn enqueue_command_delivered(
    st: &AppState,
    device_id: Uuid,
    ctype: &str,
    payload: Value,
) -> AppResult<(Uuid, bool)> {
    let id: Uuid = if COALESCE_TYPES.contains(&ctype) {
        sqlx::query_scalar(
            "INSERT INTO commands (device_id, type, payload) VALUES ($1, $2, $3)
             ON CONFLICT (device_id, type)
               WHERE status IN ('queued','sent')
                 AND type IN ('lock','unlock','apply_policy','set_tamper_level')
             DO UPDATE SET payload = EXCLUDED.payload
             RETURNING id",
        )
        .bind(device_id)
        .bind(ctype)
        .bind(&payload)
        .fetch_one(&st.db)
        .await?
    } else {
        sqlx::query_scalar(
            "INSERT INTO commands (device_id, type, payload) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(device_id)
        .bind(ctype)
        .bind(&payload)
        .fetch_one(&st.db)
        .await?
    };

    let frame = json!({
        "type": "command",
        "command": { "id": id, "type": ctype, "payload": payload }
    });
    let delivered = st.hub.push(device_id, frame).await;
    if delivered {
        sqlx::query("UPDATE commands SET status = 'sent', sent_at = now() WHERE id = $1")
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

/// Derive a stable policy version string from the max `updated_at` across the
/// profiles assigned to a device's users — and the device's VPN profile, so a
/// set/removed VPN config also bumps the version and poll-mode agents re-pull.
async fn policy_version(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<String> {
    let ts: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT GREATEST(
            (SELECT max(p.updated_at) FROM device_users du
             JOIN profiles p ON p.id = du.profile_id WHERE du.device_id = $1),
            (SELECT vpn_updated_at FROM devices WHERE id = $1))",
    )
    .bind(device_id)
    .fetch_one(db)
    .await?;
    Ok(match ts {
        Some(t) => t.timestamp_millis().to_string(),
        None => "0".to_string(),
    })
}

/// Every OS login becomes a `device_users` row linked to a person
/// (`members::link_os_user`): name-match, else the device's owner, else a new
/// member. Nothing stays unlinked.
async fn upsert_os_users(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
    users: &[OsUser],
) -> AppResult<()> {
    for u in users {
        if u.username.trim().is_empty() {
            continue;
        }
        crate::members::link_os_user(
            db,
            tenant_id,
            device_id,
            &u.username,
            u.display_name.as_deref(),
        )
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

    // No recovery PIN is minted here any more (0.5): the offline ways back
    // into a device are the unlock code and the recovery codes, both read off
    // the console after a step-up and verified by the agent. Nothing is shown
    // once on a terminal that a parent then has to write on a sticker.
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

    upsert_os_users(&st.db, tenant_id, device_id, &req.os_users).await?;

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

/// A reported daily total may dip this far below the recorded total without
/// being flagged — absorbs clock jitter and the minute-granularity of the wire
/// format. A larger drop is a real regression (a wiped or rolled-back client
/// ledger) worth an `evasion` event.
const USAGE_REGRESSION_SECS: i32 = 300;

/// Upsert today's per-user usage into the screen-time ledger. Shared by the HTTP
/// heartbeat and the WS `heartbeat` frame so both report identically. Also the
/// server-side anti-cheat hook: the client ledger only ever moves forward within
/// a day, so a heartbeat reporting *less* than we've already recorded means the
/// counter was reset behind our back. The monotonic GREATEST clamp neutralizes
/// the cheat (the total can't go down); this records it so it isn't invisible.
async fn upsert_usage(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
    usage: &[UsageEntry],
) -> Result<(), sqlx::Error> {
    for u in usage {
        let new_seconds = u.used_minutes_today.max(0) * 60;

        // Read the recorded total for today BEFORE the GREATEST clamp hides a drop.
        let prev: Option<(Uuid, i32)> = sqlx::query_as(
            "SELECT stl.device_user_id, stl.used_seconds
             FROM screen_time_ledger stl
             JOIN device_users du ON du.id = stl.device_user_id
             WHERE du.device_id = $1 AND du.os_username = $2 AND stl.day = CURRENT_DATE",
        )
        .bind(device_id)
        .bind(&u.os_username)
        .fetch_optional(db)
        .await?;

        if let Some((device_user_id, prev_seconds)) = prev {
            if new_seconds + USAGE_REGRESSION_SECS < prev_seconds {
                // Best-effort audit; a failed insert must not drop the heartbeat.
                let _ = events::insert(
                    db,
                    tenant_id,
                    Some(device_id),
                    Some(device_user_id),
                    "evasion",
                    // Critical, not warn: this is the one evasion signal the
                    // server derives independently of the device's honesty, and
                    // the alert fan-out only pushes `critical` to the parent's
                    // phone. A warn here means a confirmed ledger reset that
                    // never leaves the console.
                    "critical",
                    json!({
                        "kind": "usage_regression",
                        "os_username": u.os_username,
                        "reported_seconds": new_seconds,
                        "ledger_seconds": prev_seconds,
                        "message": "reported usage dropped below the recorded daily total; \
                                    counter clamped (possible client-ledger reset)",
                    }),
                )
                .await;
            }
        }

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
        .bind(new_seconds)
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
    /// Optional `state` (same shape as the WS frame) for poll-mode agents.
    #[serde(default)]
    pub state: Option<Value>,
}

// ---------------------------------------------------------------------------
// Presence: the agent's `state` frame
// ---------------------------------------------------------------------------

/// Apply an agent `state` frame — what the device *is*, read back from the
/// kernel, not what we asked for: `{ locked, frozen_users, enforcing, gaps,
/// agent_version, active_users }`. Best-effort; a malformed frame is ignored.
async fn apply_state(db: &sqlx::PgPool, device_id: Uuid, state: &Value) {
    let Some(locked) = state.get("locked").and_then(Value::as_bool) else {
        tracing::warn!(%device_id, "state frame without a boolean `locked`; ignored");
        return;
    };
    // Bound what we persist; the agent is a semi-trusted origin.
    let mut stored = state.clone();
    if let Some(o) = stored.as_object_mut() {
        o.remove("type");
        o.remove("kind");
    }
    if serde_json::to_string(&stored)
        .map(|s| s.len())
        .unwrap_or(usize::MAX)
        > 8 * 1024
    {
        tracing::warn!(%device_id, "oversize state frame; ignored");
        return;
    }
    let version = state
        .get("agent_version")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.len() < 64)
        .map(str::to_string);
    let _ = sqlx::query(
        "UPDATE devices SET locked = $2, last_state = $3, last_seen = now(), status = 'online',
                agent_version = COALESCE($4, agent_version)
          WHERE id = $1",
    )
    .bind(device_id)
    .bind(locked)
    .bind(&stored)
    .bind(version)
    .execute(db)
    .await;
}

pub async fn heartbeat(
    State(st): State<AppState>,
    agent: AgentAuth,
    Json(req): Json<HeartbeatReq>,
) -> AppResult<Json<Value>> {
    // A heartbeat is life: the device is online. `locked` is its own column
    // and is only ever written from the agent's `state` frame or a lock ack.
    let _ = req.status;
    sqlx::query(
        "UPDATE devices SET last_seen = now(),
             public_ip = COALESCE($2::inet, public_ip),
             status = 'online'
         WHERE id = $1",
    )
    .bind(agent.device_id)
    .bind(req.public_ip)
    .execute(&st.db)
    .await?;

    if !req.os_users.is_empty() {
        upsert_os_users(&st.db, agent.tenant_id, agent.device_id, &req.os_users).await?;
    }
    if let Some(state) = &req.state {
        apply_state(&st.db, agent.device_id, state).await;
    }

    // Persist today's per-user usage into the screen-time ledger.
    upsert_usage(&st.db, agent.tenant_id, agent.device_id, &req.usage).await?;

    // Return queued/sent (undelivered-or-unacked) commands and mark them sent.
    let cmds = pull_pending_commands(&st.db, agent.device_id).await?;
    let version = policy_version(&st.db, agent.device_id).await?;

    Ok(Json(json!({
        "commands": cmds,
        "policy_version": version,
    })))
}

/// A `sent` command whose ack hasn't arrived is redelivered only after this
/// grace window — NOT on every heartbeat, which used to re-execute unacked
/// commands over and over.
const REDELIVERY_GRACE_SECS: i64 = 90;

async fn pull_pending_commands(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<Vec<Value>> {
    let rows: Vec<(Uuid, String, Value)> = sqlx::query_as(&format!(
        "SELECT id, type, payload FROM commands
         WHERE device_id = $1
           AND (status = 'queued'
                OR (status = 'sent'
                    AND sent_at < now() - interval '{REDELIVERY_GRACE_SECS} seconds'))
         ORDER BY created_at",
    ))
    .bind(device_id)
    .fetch_all(db)
    .await?;

    let ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    sqlx::query("UPDATE commands SET status = 'sent', sent_at = now() WHERE id = ANY($1)")
        .bind(&ids)
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
    let vpn = crate::vpn::active_for_agent(&st.db, agent.device_id).await?;

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
            // Round-trip through the shared type for forward-compat
            // normalization. A profile-level `parent_pin_hash` (set
            // deliberately by an admin) passes through untouched; the device
            // recovery PIN is no longer folded in — recovery codes replaced it.
            let normalized: Policy = serde_json::from_value(policy).unwrap_or_default();
            json!({
                "os_username": os_username,
                "profile_kind": kind,
                "policy": normalized,
            })
        })
        .collect();

    let version = policy_version(&st.db, agent.device_id).await?;
    let totp_secret = crate::devices::ensure_parent_code(&st.db, agent.device_id).await?;
    let recovery_codes = crate::devices::recovery_codes_for_agent(&st.db, agent.device_id).await?;
    Ok(Json(json!({
        "policy_version": version,
        "device_tamper_level": tamper_level,
        "users": users,
        // The per-device unlock code (docs/CONTRACT-0.5.md §1): the secret
        // behind the 6-digit code the console shows, plus the keyed MACs of
        // the unused one-time recovery codes. Both verified offline by the
        // agent; the parent only ever sees codes, never this.
        "parent_code": { "totp_secret": totp_secret, "recovery_codes": recovery_codes },
        // The device's ACTIVE named VPN profile (raw config, private keys and
        // all) — only served here, on the authenticated agent pull. A
        // status of "testing" asks the agent to verify-then-report.
        "vpn": vpn,
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
    // An enrolled device is a semi-trusted origin (a rooted managed device holds
    // a valid token and could forge events — e.g. a `critical` one whose message
    // is relayed to the parent's phone). The DB CHECK constrains `type`; here we
    // reject an out-of-range severity with a clean 400 and bound the batch +
    // payload size so a device can't blast oversized/unbounded events.
    const MAX_EVENTS: usize = 100;
    const MAX_PAYLOAD_BYTES: usize = 8 * 1024;
    if req.events.len() > MAX_EVENTS {
        return Err(AppError::BadRequest("too many events in one push".into()));
    }
    for ev in req.events {
        if !matches!(ev.severity.as_str(), "info" | "warn" | "critical") {
            return Err(AppError::BadRequest("invalid event severity".into()));
        }
        if ev.r#type.trim().is_empty() {
            return Err(AppError::BadRequest("event type required".into()));
        }
        if serde_json::to_string(&ev.payload)
            .map(|s| s.len())
            .unwrap_or(usize::MAX)
            > MAX_PAYLOAD_BYTES
        {
            return Err(AppError::BadRequest("event payload too large".into()));
        }
        let device_user_id = resolve_device_user(&st.db, agent.device_id, ev.device_user).await?;
        if ev.r#type == "vpn_profile" {
            // The agent's verdict on a tested profile lands in the profile row.
            crate::vpn::apply_agent_report(&st.db, agent.device_id, &ev.payload).await;
        }
        if ev.r#type == "parent_code_backup_used" {
            // A recovery code was spent offline; retire it here too.
            crate::devices::mark_recovery_code_used(&st.db, agent.device_id, &ev.payload).await;
        }
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
/// Truthful lock state: `devices.locked` only flips when the agent confirms a
/// `lock`/`unlock` (or reports it in a `state` frame) — this is where a lock
/// that was merely queued (device offline at click time) becomes real.
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
    // Only a still-open command may be acked. Without the status guard a device
    // could re-ack any command id it has ever seen — replaying an old `unlock`
    // to forge its own lock state, resurrecting a `cancelled` command, or
    // rewriting acked_at/result on historical rows (audit tampering).
    let ctype: Option<String> = sqlx::query_scalar(
        "UPDATE commands SET status = $1, result = $2, acked_at = now()
         WHERE id = $3 AND device_id = $4 AND status IN ('queued','sent') RETURNING type",
    )
    .bind(status)
    .bind(result)
    .bind(command_id)
    .bind(device_id)
    .fetch_optional(db)
    .await?;

    if status == "acked" {
        // The agent confirmed it applied the lock/unlock: that is the truth
        // until its next `state` frame says otherwise.
        match ctype.as_deref() {
            Some("lock") => {
                sqlx::query("UPDATE devices SET locked = true WHERE id = $1")
                    .bind(device_id)
                    .execute(db)
                    .await?;
            }
            Some("unlock") => {
                sqlx::query("UPDATE devices SET locked = false WHERE id = $1")
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
    let conn_token = st.hub.register_agent(device_id, tx).await;

    // Mark online. `locked` is a separate column now, so nothing to preserve.
    let _ = sqlx::query("UPDATE devices SET status = 'online', last_seen = now() WHERE id = $1")
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
    // Evict only our own connection; a reconnect that already replaced us keeps
    // its live channel. `unregister` no-ops if we're not the current entry.
    st.hub.unregister_agent(device_id, conn_token).await;
    // The socket is gone: offline, immediately — but only if nothing else has
    // re-registered this device in the meantime (a reconnect that raced us).
    if !st.hub.is_online(device_id).await {
        let _ = sqlx::query("UPDATE devices SET status = 'offline' WHERE id = $1")
            .bind(device_id)
            .execute(&st.db)
            .await;
    }
}

/// Handle one inbound WS frame from an agent. The envelope is tagged with
/// `type` (the agent's `AgentFrame`, per CONTRACT-PROD.md §3); the legacy
/// `kind` tag is accepted as an alias.
///
/// Frame types:
///   - `{ type:"event", event:{ type, severity?, device_user?, payload } }`
///     (fields at the top level also accepted)
///   - `{ type:"ack", ack:{ command_id, status, result? } }` (idem)
///   - `{ type:"state", locked, frozen_users, enforcing, gaps, agent_version, active_users }`
///     (also accepted nested under "state")
///   - `{ type:"heartbeat", usage?, state? }` / `{ type:"pong" }`
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

            // SECURITY: bound the WS path the same way the HTTP path is bound
            // (see push_events). Without this, a rooted managed device — which
            // already has a valid device_token (the threat model acknowledged
            // at agent.rs:547-549) — can flood oversized/unbounded events or
            // arbitrary severity strings through the WS, bypassing the
            // MAX_EVENTS/MAX_PAYLOAD_BYTES/severity-whitelist checks the HTTP
            // handler enforces. The DB CHECK on `severity` would silently
            // reject invalid severities, but each invalid event still costs a
            // Postgres roundtrip. Drop the frame early instead.
            const MAX_PAYLOAD_BYTES: usize = 8 * 1024;
            if !matches!(severity, "info" | "warn" | "critical") {
                tracing::warn!(agent_device_id = %agent.device_id, severity,
                    "dropping WS event with invalid severity");
                return;
            }
            if serde_json::to_string(&payload)
                .map(|s| s.len())
                .unwrap_or(usize::MAX)
                > MAX_PAYLOAD_BYTES
            {
                tracing::warn!(agent_device_id = %agent.device_id,
                    "dropping WS event with oversize payload");
                return;
            }

            if let Ok(device_user_id) =
                resolve_device_user(&st.db, agent.device_id, device_user).await
            {
                if etype == "vpn_profile" {
                    crate::vpn::apply_agent_report(&st.db, agent.device_id, &payload).await;
                }
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
        Some("state") => {
            let frame = v.get("state").unwrap_or(&v);
            apply_state(&st.db, agent.device_id, frame).await;
        }
        Some("heartbeat") => {
            let _ = sqlx::query(
                "UPDATE devices SET last_seen = now(), status = 'online' WHERE id = $1",
            )
            .bind(agent.device_id)
            .execute(&st.db)
            .await;
            if let Some(state) = v.get("state") {
                apply_state(&st.db, agent.device_id, state).await;
            }
            // A WS-connected agent has no HTTP heartbeat; persist its usage here so
            // the ledger stays current in the normal (non-poll) path.
            if let Some(usage) = v.get("usage") {
                if let Ok(entries) = serde_json::from_value::<Vec<UsageEntry>>(usage.clone()) {
                    let _ = upsert_usage(&st.db, agent.tenant_id, agent.device_id, &entries).await;
                }
            }
        }
        _ => {}
    }
}
