//! Admin device CRUD + lock/unlock + device-user profile assignment.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::{enqueue_command, enqueue_command_delivered, pending_command};
use crate::auth::gen_token;
use crate::error::{AppError, AppResult};
use crate::events;
use crate::state::{AppState, AuthAdmin};

pub const DEVICE_COLS: &str = "id, tenant_id, name, hostname, os, agent_version, status, \
    tamper_level, public_ip::text, last_seen, created_at, vpn_updated_at, \
    offline_allowed_until, locked, last_state, owner_account_id";

pub type DeviceRow = (
    Uuid,
    Uuid,
    String,
    String,
    String,
    String,
    String,
    i32,
    Option<String>,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    bool,
    Option<Value>,
    Option<Uuid>,
);

pub fn device_to_json(r: &DeviceRow) -> Value {
    json!({
        "id": r.0,
        "tenant_id": r.1,
        "name": r.2,
        "hostname": r.3,
        "os": r.4,
        "agent_version": r.5,
        // pending | online | offline — presence only. Whether the screens are
        // frozen is `locked`, below.
        "status": r.6,
        "tamper_level": r.7,
        "public_ip": r.8,
        "last_seen": r.9,
        "created_at": r.10,
        // Named VPN profiles live in device_vpn_profiles (GET /devices/:id/vpn);
        // this stamp only says "something VPN-ish changed" for cache-busting.
        "vpn_updated_at": r.11,
        // Set by PUT /devices/:id/offline-window: a parent said this machine
        // may be away, so being offline is expected rather than trouble.
        "offline_allowed_until": r.12,
        // What the agent last reported (state frame / lock ack) — the truth,
        // never what a parent merely asked for. `lock_pending` is folded in by
        // the list/detail/family handlers from the command queue.
        "locked": r.13,
        "last_state": r.14,
        "owner_account_id": r.15,
    })
}

/// `lock_pending`: a lock or unlock is queued/sent and not yet confirmed, OR
/// the agent's own intent (`last_state.lock_intent`) disagrees with what the
/// kernel says (`last_state.locked`) — it is mid-way through applying.
pub fn lock_pending(pending_types: &[String], last_state: Option<&Value>) -> bool {
    if pending_types.iter().any(|t| t == "lock" || t == "unlock") {
        return true;
    }
    match last_state {
        Some(st) => match (
            st.get("lock_intent").and_then(Value::as_bool),
            st.get("locked").and_then(Value::as_bool),
        ) {
            (Some(intent), Some(locked)) => intent != locked,
            _ => false,
        },
        None => false,
    }
}

// --- Unlock code (per-device TOTP) + recovery codes ------------------------
//
// The device secret is shared by exactly two parties: this server and the
// agent. A parent never sees it — they read the *current* 6-digit code from
// the console (after a step-up), and the agent verifies it offline. No QR, no
// third-party authenticator, nothing to scan or lose.

/// The device's unlock-code secret, minting one if the device predates 0.4.
/// Base32, 20 bytes. Only ever handed to the agent.
pub async fn ensure_parent_code(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<String> {
    let existing: Option<Option<String>> =
        sqlx::query_scalar("SELECT parent_totp_secret FROM devices WHERE id = $1")
            .bind(device_id)
            .fetch_optional(db)
            .await?;
    let existing = existing.ok_or_else(|| AppError::NotFound("device not found".into()))?;
    if let Some(s) = existing {
        return Ok(s);
    }
    let fresh = crate::stepup::gen_totp_secret();
    // Race-safe: whoever lands first wins, everybody reads the winner back.
    let secret: String = sqlx::query_scalar(
        "UPDATE devices SET parent_totp_secret = COALESCE(parent_totp_secret, $2)
          WHERE id = $1 RETURNING parent_totp_secret",
    )
    .bind(device_id)
    .bind(&fresh)
    .fetch_one(db)
    .await?;
    Ok(secret)
}

/// The live 6-digit unlock code for a device, as the parent reads it off the
/// console. `seconds_left` lets the UI draw the countdown and refetch on the
/// step boundary instead of polling.
fn unlock_code_json(device_name: &str, secret: &str) -> AppResult<Value> {
    let (code, seconds_left) = crate::stepup::current_totp(secret)
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("device secret is not valid base32")))?;
    Ok(json!({
        "code": code,
        "seconds_left": seconds_left,
        "period": crate::stepup::TOTP_STEP,
        "device_name": device_name,
    }))
}

/// `GET /api/devices/{id}/unlock-code` — step-up gated (sensitive read): the
/// code that opens this computer right now.
pub async fn get_unlock_code(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row = get_device_row(&st.db, id, admin.tenant_id).await?;
    let secret = ensure_parent_code(&st.db, id).await?;
    Ok(Json(unlock_code_json(&row.2, &secret)?))
}

/// `POST /api/devices/{id}/unlock-code/rotate` — a new secret; the codes the
/// console shows change on the spot, and the device follows once it pulls
/// policy. Recovery codes are keyed by the secret, so they die with it.
pub async fn rotate_unlock_code(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row = get_device_row(&st.db, id, admin.tenant_id).await?;
    let fresh = crate::stepup::gen_totp_secret();
    let mut tx = st.db.begin().await?;
    sqlx::query("UPDATE devices SET parent_totp_secret = $2 WHERE id = $1")
        .bind(id)
        .bind(&fresh)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM device_recovery_codes WHERE device_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    enqueue_command(&st, id, "apply_policy", json!({})).await?;
    events::insert(
        &st.db,
        admin.tenant_id,
        Some(id),
        None,
        "parent_code_ok",
        "info",
        json!({ "action": "rotated", "by": admin.admin_id }),
    )
    .await?;
    let mut out = unlock_code_json(&row.2, &fresh)?;
    out["recovery_codes_cleared"] = json!(true);
    Ok(Json(out))
}

/// How many recovery codes a device gets per set.
pub const RECOVERY_CODES_PER_SET: usize = 8;

/// An 8-digit recovery code. Digits only and fixed length because it gets read
/// off a printout and typed on a locked machine's overlay — not pasted. The
/// keyspace is fine: it is verified offline against a keyed MAC by someone
/// already at the keyboard, single-use, behind the agent's lockout.
fn gen_recovery_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| char::from(b'0' + rng.gen_range(0..10u8)))
        .collect()
}

/// "1234 5678" — how a code is shown and printed. The agent strips the space.
fn format_recovery_code(code: &str) -> String {
    format!("{} {}", &code[..4], &code[4..])
}

/// `POST /api/devices/{id}/recovery-codes` — replace the set with eight fresh
/// one-time codes. Returned in plaintext exactly once; the server keeps only
/// the keyed MACs, the agent gets the same MACs on its next policy pull.
pub async fn generate_recovery_codes(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    get_device_row(&st.db, id, admin.tenant_id).await?;
    let secret = ensure_parent_code(&st.db, id).await?;
    let codes: Vec<String> = (0..RECOVERY_CODES_PER_SET)
        .map(|_| gen_recovery_code())
        .collect();
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM device_recovery_codes WHERE device_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for (i, code) in codes.iter().enumerate() {
        let mac = crate::stepup::recovery_mac(&secret, code).ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!("device secret is not valid base32"))
        })?;
        sqlx::query("INSERT INTO device_recovery_codes (device_id, idx, mac) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(i as i16)
            .bind(&mac)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    enqueue_command(&st, id, "apply_policy", json!({})).await?;
    events::insert(
        &st.db,
        admin.tenant_id,
        Some(id),
        None,
        "parent_code_ok",
        "info",
        json!({ "action": "recovery_codes_generated", "by": admin.admin_id }),
    )
    .await?;
    Ok(Json(json!({
        "codes": codes.iter().map(|c| format_recovery_code(c)).collect::<Vec<_>>(),
        "generated_at": Utc::now(),
    })))
}

/// `GET /api/devices/{id}/recovery-codes` — step-up gated: how many are left
/// and when the set was made. Never the codes themselves.
pub async fn recovery_codes_status(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    get_device_row(&st.db, id, admin.tenant_id).await?;
    let row: (i64, i64, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE used_at IS NULL), count(*), max(created_at)
           FROM device_recovery_codes WHERE device_id = $1",
    )
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    Ok(Json(json!({
        "unused": row.0,
        "total": row.1,
        "generated_at": row.2,
    })))
}

/// How many one-time recovery codes each of a tenant's devices still has
/// unused — folded into device JSON as `recovery_codes_unused` (0 = none were
/// ever generated, or all are spent). The codes themselves are shown once, at
/// generation, and never again.
pub async fn recovery_unused_by_device(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
) -> AppResult<std::collections::HashMap<Uuid, i64>> {
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT rc.device_id, count(*) FROM device_recovery_codes rc
           JOIN devices d ON d.id = rc.device_id
          WHERE d.tenant_id = $1 AND rc.used_at IS NULL GROUP BY rc.device_id",
    )
    .bind(tenant_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Same, for one device.
pub async fn recovery_unused_one(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM device_recovery_codes WHERE device_id = $1 AND used_at IS NULL",
    )
    .bind(device_id)
    .fetch_one(db)
    .await?)
}

/// The unused recovery codes of a device, for the agent's policy bundle.
pub async fn recovery_codes_for_agent(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<Vec<Value>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, mac FROM device_recovery_codes
          WHERE device_id = $1 AND used_at IS NULL ORDER BY idx",
    )
    .bind(device_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, mac)| json!({ "id": id, "mac": mac }))
        .collect())
}

/// The agent reports a recovery code as spent (`parent_code_backup_used`
/// with a `recovery_id`): mark it so it is neither shown as unused nor sent
/// to the device again. Unknown or foreign ids are ignored — a rooted device
/// can only ever burn its own codes.
pub async fn mark_recovery_code_used(db: &sqlx::PgPool, device_id: Uuid, payload: &Value) {
    let Some(id) = payload
        .get("recovery_id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
    else {
        return;
    };
    let _ = sqlx::query(
        "UPDATE device_recovery_codes SET used_at = now()
          WHERE id = $1 AND device_id = $2 AND used_at IS NULL",
    )
    .bind(id)
    .bind(device_id)
    .execute(db)
    .await;
}

/// Types of commands still pending (queued|sent) for a device — drives the
/// server-backed PENDING chips in the UI (replacing lost-on-reload React state).
async fn pending_command_types(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT type FROM commands
         WHERE device_id = $1 AND status IN ('queued','sent')
         ORDER BY created_at",
    )
    .bind(device_id)
    .fetch_all(db)
    .await?)
}

/// Fetch a device scoped to the tenant, or 404.
async fn get_device_row(db: &sqlx::PgPool, id: Uuid, tenant_id: Uuid) -> AppResult<DeviceRow> {
    let row: Option<DeviceRow> = sqlx::query_as(&format!(
        "SELECT {DEVICE_COLS} FROM devices WHERE id = $1 AND tenant_id = $2"
    ))
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(db)
    .await?;
    row.ok_or_else(|| AppError::NotFound("device not found".into()))
}

pub async fn list_devices(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let rows: Vec<DeviceRow> = sqlx::query_as(&format!(
        "SELECT {DEVICE_COLS} FROM devices WHERE tenant_id = $1 ORDER BY created_at DESC"
    ))
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;

    let recovery = recovery_unused_by_device(&st.db, admin.tenant_id).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let mut d = device_to_json(r);
        d["users"] = device_users_json(&st.db, r.0).await?;
        d["online"] = json!(d["status"] == "online");
        d["recovery_codes_unused"] = json!(recovery.get(&r.0).copied().unwrap_or(0));
        let pending = pending_command_types(&st.db, r.0).await?;
        d["lock_pending"] = json!(lock_pending(&pending, r.14.as_ref()));
        d["pending_commands"] = json!(pending);
        out.push(d);
    }
    Ok(Json(json!({ "devices": out })))
}

pub async fn get_device(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row = get_device_row(&st.db, id, admin.tenant_id).await?;
    let users = device_users_json(&st.db, id).await?;
    let recent = events::recent_for_device(&st.db, admin.tenant_id, id, 25).await?;

    let mut d = device_to_json(&row);
    d["online"] = json!(d["status"] == "online");
    d["recovery_codes_unused"] = json!(recovery_unused_one(&st.db, id).await?);
    let pending = pending_command_types(&st.db, id).await?;
    d["lock_pending"] = json!(lock_pending(&pending, row.14.as_ref()));
    d["pending_commands"] = json!(pending);
    Ok(Json(json!({
        "device": d,
        "users": users,
        "recent_events": recent,
    })))
}

#[derive(Deserialize)]
pub struct CreateDeviceReq {
    pub name: String,
    /// "This is <person>'s computer": OS logins that enroll without a name
    /// match link to this account instead of spawning a new member.
    #[serde(default, alias = "member_id")]
    pub account_id: Option<Uuid>,
}

pub async fn create_device(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Json(req): Json<CreateDeviceReq>,
) -> AppResult<Json<Value>> {
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    if let Some(acct) = req.account_id {
        crate::members::get_account(&st.db, acct, admin.tenant_id).await?;
    }
    let enroll_token = gen_token();
    // The unlock-code secret is born with the device; only the agent ever
    // receives it (on its first policy pull). The parent reads codes off the
    // console.
    let secret = crate::stepup::gen_totp_secret();
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO devices (tenant_id, name, enroll_token, enroll_token_expires_at, status,
                              parent_totp_secret, owner_account_id)
         VALUES ($1, $2, $3, now() + interval '24 hours', 'pending', $4, $5) RETURNING id",
    )
    .bind(admin.tenant_id)
    .bind(req.name.trim())
    .bind(&enroll_token)
    .bind(&secret)
    .bind(req.account_id)
    .fetch_one(&st.db)
    .await?;

    let row = get_device_row(&st.db, id, admin.tenant_id).await?;
    Ok(Json(json!({
        "device": device_to_json(&row),
        "enroll_token": enroll_token,
    })))
}

/// POST /api/devices/:id/enroll-token — regenerate the one-time enroll token
/// (fresh 24 h TTL). Only valid while the device is still `pending`: an
/// enrolled device already holds a bearer token and must be re-created to
/// re-enroll.
pub async fn regen_enroll_token(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row = get_device_row(&st.db, id, admin.tenant_id).await?;
    if row.6 != "pending" {
        return Err(AppError::Conflict(
            "device is already enrolled — enroll tokens exist only while pending".into(),
        ));
    }

    let enroll_token = gen_token();
    sqlx::query(
        "UPDATE devices SET enroll_token = $1, enroll_token_expires_at = now() + interval '24 hours'
         WHERE id = $2 AND tenant_id = $3",
    )
    .bind(&enroll_token)
    .bind(id)
    .bind(admin.tenant_id)
    .execute(&st.db)
    .await?;

    Ok(Json(json!({
        "device": device_to_json(&row),
        "enroll_token": enroll_token,
    })))
}

#[derive(Deserialize)]
pub struct PatchDeviceReq {
    pub name: Option<String>,
    pub tamper_level: Option<i32>,
}

pub async fn patch_device(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchDeviceReq>,
) -> AppResult<Json<Value>> {
    // Ensure ownership.
    get_device_row(&st.db, id, admin.tenant_id).await?;

    if let Some(name) = &req.name {
        sqlx::query("UPDATE devices SET name = $1 WHERE id = $2 AND tenant_id = $3")
            .bind(name)
            .bind(id)
            .bind(admin.tenant_id)
            .execute(&st.db)
            .await?;
    }
    if let Some(level) = req.tamper_level {
        if level != 1 && level != 3 {
            return Err(AppError::BadRequest("tamper_level must be 1 or 3".into()));
        }
        sqlx::query("UPDATE devices SET tamper_level = $1 WHERE id = $2 AND tenant_id = $3")
            .bind(level)
            .bind(id)
            .bind(admin.tenant_id)
            .execute(&st.db)
            .await?;
        enqueue_command(&st, id, "set_tamper_level", json!({ "level": level })).await?;
    }

    let row = get_device_row(&st.db, id, admin.tenant_id).await?;
    Ok(Json(json!({ "device": device_to_json(&row) })))
}

// --- VPN profile -------------------------------------------------------------

pub async fn delete_device(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let res = sqlx::query("DELETE FROM devices WHERE id = $1 AND tenant_id = $2")
        .bind(id)
        .bind(admin.tenant_id)
        .execute(&st.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound("device not found".into()));
    }
    st.hub.force_unregister(id).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn lock_device(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    get_device_row(&st.db, id, admin.tenant_id).await?;
    // One pending lock is enough — a second click is a 409, and the UI shows
    // the server-backed pending state instead of stacking commands.
    if pending_command(&st.db, id, "lock").await?.is_some() {
        return Err(AppError::Conflict("a lock is already pending".into()));
    }
    // Truthful lock state: the device shows `lock_pending` until the agent
    // acks (or its next `state` frame says locked). Nothing is flipped here.
    let (cmd_id, delivered) = enqueue_command_delivered(&st, id, "lock", json!({})).await?;
    events::insert(
        &st.db,
        admin.tenant_id,
        Some(id),
        None,
        "lock",
        "warn",
        json!({ "by": admin.admin_id, "delivered": delivered }),
    )
    .await?;
    Ok(Json(
        json!({ "command_id": cmd_id, "queued": true, "delivered": delivered }),
    ))
}

pub async fn unlock_device(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    get_device_row(&st.db, id, admin.tenant_id).await?;
    if pending_command(&st.db, id, "unlock").await?.is_some() {
        return Err(AppError::Conflict("an unlock is already pending".into()));
    }
    // Mirror of lock: pending until the agent confirms.
    let (cmd_id, delivered) = enqueue_command_delivered(&st, id, "unlock", json!({})).await?;
    events::insert(
        &st.db,
        admin.tenant_id,
        Some(id),
        None,
        "unlock",
        "info",
        json!({ "by": admin.admin_id, "delivered": delivered }),
    )
    .await?;
    Ok(Json(
        json!({ "command_id": cmd_id, "queued": true, "delivered": delivered }),
    ))
}

// --- Device users -----------------------------------------------------------

type DeviceUserRow = (
    Uuid,
    Uuid,
    String,
    Option<String>,
    Uuid,
    String,
    String,
    i32,
    i32,
    Option<Uuid>,
);

pub async fn device_users_json(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<Value> {
    let rows: Vec<DeviceUserRow> = sqlx::query_as(
        "SELECT du.id, du.device_id, du.os_username, du.display_name, du.profile_id, \
                p.name, p.kind, \
                COALESCE(l.used_seconds, 0), COALESCE(l.earned_seconds, 0), du.account_id \
         FROM device_users du JOIN profiles p ON p.id = du.profile_id \
         LEFT JOIN screen_time_ledger l \
                ON l.device_user_id = du.id AND l.day = CURRENT_DATE \
         WHERE du.device_id = $1 ORDER BY du.os_username",
    )
    .bind(device_id)
    .fetch_all(db)
    .await?;
    let users: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.0,
                "device_id": r.1,
                "os_username": r.2,
                "display_name": r.3,
                "profile_id": r.4,
                "profile_name": r.5,
                "profile_kind": r.6,
                "used_minutes_today": r.7 / 60,
                "earned_minutes_today": r.8 / 60,
                "account_id": r.9,
            })
        })
        .collect();
    Ok(json!(users))
}

pub async fn list_device_users(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    get_device_row(&st.db, id, admin.tenant_id).await?;
    let users = device_users_json(&st.db, id).await?;
    Ok(Json(json!({ "users": users })))
}

#[derive(Deserialize)]
pub struct AssignProfileReq {
    pub profile_id: Uuid,
}

pub async fn assign_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(device_user_id): Path<Uuid>,
    Json(req): Json<AssignProfileReq>,
) -> AppResult<Json<Value>> {
    // Verify the device_user belongs to a device in this tenant.
    let owner: Option<(Uuid,)> = sqlx::query_as(
        "SELECT d.id FROM device_users du JOIN devices d ON d.id = du.device_id \
         WHERE du.id = $1 AND d.tenant_id = $2",
    )
    .bind(device_user_id)
    .bind(admin.tenant_id)
    .fetch_optional(&st.db)
    .await?;
    let device_id = owner
        .ok_or_else(|| AppError::NotFound("device user not found".into()))?
        .0;

    // Verify the profile belongs to this tenant.
    let prof: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM profiles WHERE id = $1 AND tenant_id = $2")
            .bind(req.profile_id)
            .bind(admin.tenant_id)
            .fetch_optional(&st.db)
            .await?;
    prof.ok_or_else(|| AppError::NotFound("profile not found".into()))?;

    sqlx::query("UPDATE device_users SET profile_id = $1 WHERE id = $2")
        .bind(req.profile_id)
        .bind(device_user_id)
        .execute(&st.db)
        .await?;

    // Tell the agent to re-pull policy.
    enqueue_command(&st, device_id, "apply_policy", json!({})).await?;

    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct AssignAccountReq {
    pub account_id: Uuid,
}

/// `POST /api/device-users/{id}/assign-account` — relink an OS login to a
/// different person in the household. Enrollment links unmatched logins to the
/// device's owner; a second account on a child's laptop (a parent's, say) ends
/// up with the child's rules until it is moved here. The login takes the new
/// person's rules immediately (profile_id follows the account) and the agent
/// re-pulls.
pub async fn assign_account(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(device_user_id): Path<Uuid>,
    Json(req): Json<AssignAccountReq>,
) -> AppResult<Json<Value>> {
    let owner: Option<(Uuid,)> = sqlx::query_as(
        "SELECT d.id FROM device_users du JOIN devices d ON d.id = du.device_id \
         WHERE du.id = $1 AND d.tenant_id = $2",
    )
    .bind(device_user_id)
    .bind(admin.tenant_id)
    .fetch_optional(&st.db)
    .await?;
    let device_id = owner
        .ok_or_else(|| AppError::NotFound("device user not found".into()))?
        .0;

    let acct: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT profile_id FROM admins WHERE id = $1 AND tenant_id = $2")
            .bind(req.account_id)
            .bind(admin.tenant_id)
            .fetch_optional(&st.db)
            .await?;
    let profile_id = acct
        .ok_or_else(|| AppError::NotFound("person not found".into()))?
        .0;

    sqlx::query(
        "UPDATE device_users SET account_id = $1, profile_id = COALESCE($2, profile_id) WHERE id = $3",
    )
    .bind(req.account_id)
    .bind(profile_id)
    .bind(device_user_id)
    .execute(&st.db)
    .await?;

    enqueue_command(&st, device_id, "apply_policy", json!({})).await?;
    Ok(Json(json!({ "ok": true })))
}

// --- Screen-time history -----------------------------------------------------

#[derive(Deserialize)]
pub struct UsageQuery {
    /// Days of history (default 30, cap 90).
    pub days: Option<i64>,
}

/// GET /api/device-users/:id/usage — the per-day ledger for one device user,
/// newest last, plus the current streak (consecutive days with any usage,
/// counted back from today; an empty today doesn't break it until midnight).
/// `streak_days` in the ledger was never written by anything — the streak is
/// computed here from the rows instead of trusting a dead column.
pub async fn usage_history(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
    axum::extract::Query(q): axum::extract::Query<UsageQuery>,
) -> AppResult<Json<Value>> {
    let days = q.days.unwrap_or(30).clamp(1, 90);
    // Scope: the device user must belong to a device of this tenant.
    let owned: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM device_users du
         JOIN devices d ON d.id = du.device_id
         WHERE du.id = $1 AND d.tenant_id = $2",
    )
    .bind(id)
    .bind(admin.tenant_id)
    .fetch_optional(&st.db)
    .await?;
    owned.ok_or_else(|| AppError::NotFound("device user not found".into()))?;

    let rows: Vec<(chrono::NaiveDate, i32, i32)> = sqlx::query_as(
        "SELECT day, used_seconds, earned_seconds FROM screen_time_ledger
         WHERE device_user_id = $1 AND day > CURRENT_DATE - $2::int
         ORDER BY day",
    )
    .bind(id)
    .bind(days)
    .fetch_all(&st.db)
    .await?;

    // Streak: walk back from today over the fetched window; a missing or
    // zero-usage today is forgiven (the day isn't over), any earlier gap ends it.
    let today = chrono::Utc::now().date_naive();
    let by_day: std::collections::HashMap<chrono::NaiveDate, i32> =
        rows.iter().map(|r| (r.0, r.1)).collect();
    let mut streak: i64 = 0;
    let mut cursor = today;
    loop {
        let used = by_day.get(&cursor).copied().unwrap_or(0);
        if used > 0 {
            streak += 1;
        } else if cursor != today {
            break;
        }
        let Some(prev) = cursor.pred_opt() else { break };
        // Don't walk past the fetched window — the streak is then "N+ days".
        if today.signed_duration_since(prev).num_days() >= days {
            break;
        }
        cursor = prev;
    }

    let out: Vec<Value> = rows
        .into_iter()
        .map(|(day, used, earned)| {
            json!({
                "day": day,
                "used_minutes": used / 60,
                "earned_minutes": earned / 60,
            })
        })
        .collect();
    Ok(Json(json!({ "days": out, "streak_days": streak })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_pending_is_queue_or_intent_mismatch() {
        let none: Vec<String> = vec![];
        assert!(lock_pending(&["lock".to_string()], None));
        assert!(lock_pending(&["unlock".to_string()], None));
        assert!(!lock_pending(&["apply_policy".to_string()], None));
        assert!(!lock_pending(&none, None));
        let settled = json!({ "lock_intent": true, "locked": true });
        assert!(!lock_pending(&none, Some(&settled)));
        let applying = json!({ "lock_intent": true, "locked": false });
        assert!(lock_pending(&none, Some(&applying)));
        let old_agent = json!({ "locked": false });
        assert!(!lock_pending(&none, Some(&old_agent)));
    }

    /// A recovery code is read off a printout and typed on a locked machine:
    /// anything but fixed-length digits breaks that.
    #[test]
    fn recovery_codes_are_eight_digits_and_random() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let c = gen_recovery_code();
            assert_eq!(c.len(), 8, "wrong length: {c}");
            assert!(c.chars().all(|ch| ch.is_ascii_digit()), "non-digit: {c}");
            seen.insert(c);
        }
        assert!(
            seen.len() > 190,
            "generator looks degenerate: {}",
            seen.len()
        );
    }

    #[test]
    fn recovery_code_is_shown_in_two_halves() {
        assert_eq!(format_recovery_code("12345678"), "1234 5678");
    }
}
