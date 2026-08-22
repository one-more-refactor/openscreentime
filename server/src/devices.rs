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

// --- Parent code (per-device TOTP) -----------------------------------------

/// The device's parent authenticator secret, minting one if the device
/// predates 0.4. Base32, 20 bytes.
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

fn parent_code_json(device_name: &str, secret: &str) -> Value {
    json!({
        "secret": secret,
        "otpauth_uri": crate::stepup::otpauth_uri(device_name, secret),
    })
}

/// `GET /api/devices/{id}/parent-code` — step-up gated (sensitive read).
pub async fn get_parent_code(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row = get_device_row(&st.db, id, admin.tenant_id).await?;
    let secret = ensure_parent_code(&st.db, id).await?;
    Ok(Json(
        json!({ "parent_code": parent_code_json(&row.2, &secret) }),
    ))
}

/// `POST /api/devices/{id}/parent-code/rotate` — a new secret; the old
/// authenticator entry stops working once the agent pulls policy.
pub async fn rotate_parent_code(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row = get_device_row(&st.db, id, admin.tenant_id).await?;
    let fresh = crate::stepup::gen_totp_secret();
    sqlx::query("UPDATE devices SET parent_totp_secret = $2 WHERE id = $1")
        .bind(id)
        .bind(&fresh)
        .execute(&st.db)
        .await?;
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
    Ok(Json(
        json!({ "parent_code": parent_code_json(&row.2, &fresh) }),
    ))
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

    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let mut d = device_to_json(r);
        d["users"] = device_users_json(&st.db, r.0).await?;
        d["online"] = json!(d["status"] == "online");
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
    // The parent code is born with the device so the QR can sit next to the
    // install command; the agent receives the same secret on its first pull.
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
        "parent_code": parent_code_json(&row.2, &secret),
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
    st.hub.unregister_agent(id).await;
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
}
