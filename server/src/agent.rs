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
use sentinel_policy::Policy;

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
const COALESCE_TYPES: &[&str] = &[
    "lock",
    "unlock",
    "apply_policy",
    "discover",
    "set_tamper_level",
];

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
                 AND type IN ('lock','unlock','apply_policy','discover','set_tamper_level')
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

/// An 8-digit device recovery PIN.
///
/// Digits only and fixed length because it gets read aloud down a phone, typed
/// on a locked machine's overlay, and written on a sticker — not pasted. 10^8
/// keyspace is fine: verification is argon2 against a local hash, offline, by
/// someone already at the keyboard, and there is no remote guessing surface.
fn gen_recovery_pin() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| char::from(b'0' + rng.gen_range(0..10u8)))
        .collect()
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

    // Every device gets its own recovery PIN, whether anyone asked for one or
    // not. It is the ONLY offline way back into a device that has locked itself
    // out — no server, no network, no SSH. Leaving it optional meant devices
    // shipped unrecoverable, with the lockout screen promising a PIN that did
    // not exist. Shown once, here, and never stored in plaintext.
    let recovery_pin = gen_recovery_pin();
    let recovery_hash = crate::profiles::hash_pin(recovery_pin.clone()).await?;

    sqlx::query(
        "UPDATE devices SET device_token = $1, enroll_token = NULL,
             enroll_token_expires_at = NULL, status = 'online',
             hostname = $2, os = $3, agent_version = $4, last_seen = now(),
             recovery_pin_hash = $6, recovery_pin_set_at = now()
         WHERE id = $5",
    )
    .bind(&token_hash)
    .bind(&req.hostname)
    .bind(&req.os)
    .bind(&req.agent_version)
    .bind(device_id)
    .bind(&recovery_hash)
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
        // Plaintext, exactly once. The agent prints it during enrollment so
        // whoever set the device up can write it down; after this response it
        // exists only as an argon2 hash.
        "recovery_pin": recovery_pin,
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

    // The device's own recovery PIN, used wherever the profile does not set one.
    // Without this the agent never caches a parent_pin_hash, and `sentinel-agent
    // unlock --pin` — the only offline way out of a self-inflicted lockout —
    // refuses before it checks anything.
    let device_pin_hash: Option<String> =
        sqlx::query_scalar("SELECT recovery_pin_hash FROM devices WHERE id = $1")
            .bind(agent.device_id)
            .fetch_optional(&st.db)
            .await?
            .flatten();

    let users: Vec<Value> = rows
        .into_iter()
        .map(|(os_username, kind, policy)| {
            // Round-trip through the shared type for forward-compat normalization.
            let mut normalized: Policy = serde_json::from_value(policy).unwrap_or_default();
            // A profile-level PIN wins — an admin who set one deliberately
            // should not be silently overridden by the generated fallback.
            if normalized.parent_pin_hash.is_none() {
                normalized.parent_pin_hash = device_pin_hash.clone();
            }
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
        Some("heartbeat") => {
            let _ = sqlx::query("UPDATE devices SET last_seen = now() WHERE id = $1")
                .bind(agent.device_id)
                .execute(&st.db)
                .await;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The PIN is read aloud, typed on a locked machine, and written on a
    /// sticker. Anything but fixed-length digits breaks one of those.
    #[test]
    fn recovery_pin_is_eight_digits() {
        for _ in 0..200 {
            let pin = gen_recovery_pin();
            assert_eq!(pin.len(), 8, "wrong length: {pin}");
            assert!(pin.chars().all(|c| c.is_ascii_digit()), "non-digit: {pin}");
        }
    }

    /// Not a uniqueness guarantee — just a check that it is actually random and
    /// not, say, a constant or a counter.
    #[test]
    fn recovery_pins_differ() {
        let a: std::collections::HashSet<String> = (0..50).map(|_| gen_recovery_pin()).collect();
        assert!(
            a.len() > 45,
            "generator looks degenerate: {} unique of 50",
            a.len()
        );
    }

    /// The whole point: a policy served to an agent must carry a
    /// parent_pin_hash, or `sentinel-agent unlock --pin` refuses and the device
    /// has no offline way back in. A profile-set PIN must win over the
    /// device-level fallback.
    #[test]
    fn device_pin_fills_in_only_when_the_profile_has_none() {
        let device_hash = Some("$argon2id$device".to_string());

        let mut without: Policy = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(without.parent_pin_hash.is_none());
        if without.parent_pin_hash.is_none() {
            without.parent_pin_hash = device_hash.clone();
        }
        assert_eq!(without.parent_pin_hash.as_deref(), Some("$argon2id$device"));

        let mut with_profile_pin: Policy =
            serde_json::from_value(serde_json::json!({"parent_pin_hash": "$argon2id$profile"}))
                .unwrap();
        if with_profile_pin.parent_pin_hash.is_none() {
            with_profile_pin.parent_pin_hash = device_hash.clone();
        }
        assert_eq!(
            with_profile_pin.parent_pin_hash.as_deref(),
            Some("$argon2id$profile"),
            "a deliberately set profile PIN must not be overridden by the fallback"
        );
    }
}
