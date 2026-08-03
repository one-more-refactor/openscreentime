//! Admin device CRUD + lock/unlock + device-user profile assignment.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::{enqueue_command, enqueue_command_delivered};
use crate::auth::gen_token;
use crate::error::{AppError, AppResult};
use crate::events;
use crate::state::{AppState, AuthAdmin};

const DEVICE_COLS: &str = "id, tenant_id, name, hostname, os, agent_version, status, \
    tamper_level, public_ip::text, last_seen, created_at, vpn_kind, vpn_updated_at";

type DeviceRow = (
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
    Option<String>,
    Option<DateTime<Utc>>,
);

pub fn device_to_json(r: &DeviceRow) -> Value {
    json!({
        "id": r.0,
        "tenant_id": r.1,
        "name": r.2,
        "hostname": r.3,
        "os": r.4,
        "agent_version": r.5,
        "status": r.6,
        "tamper_level": r.7,
        "public_ip": r.8,
        "last_seen": r.9,
        "created_at": r.10,
        // Presence only — the config body holds private keys and is served
        // exclusively to the enrolled agent via the policy pull.
        "vpn": r.11.as_ref().map(|kind| json!({ "kind": kind, "updated_at": r.12 })),
    })
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
        d["online"] = json!(st.hub.is_online(r.0).await);
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
    d["online"] = json!(st.hub.is_online(id).await);
    Ok(Json(json!({
        "device": d,
        "users": users,
        "recent_events": recent,
    })))
}

#[derive(Deserialize)]
pub struct CreateDeviceReq {
    pub name: String,
}

pub async fn create_device(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Json(req): Json<CreateDeviceReq>,
) -> AppResult<Json<Value>> {
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    let enroll_token = gen_token();
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO devices (tenant_id, name, enroll_token, enroll_token_expires_at, status)
         VALUES ($1, $2, $3, now() + interval '24 hours', 'pending') RETURNING id",
    )
    .bind(admin.tenant_id)
    .bind(&req.name)
    .bind(&enroll_token)
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

/// Hard cap on an uploaded VPN config. Real wg/ovpn client configs are a few
/// KB; anything bigger is a mistake (or an attempt to stuff the DB/policy pull).
const MAX_VPN_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
pub struct SetVpnReq {
    pub kind: String,
    pub config: String,
}

/// PUT /api/devices/:id/vpn — store an admin-uploaded WireGuard/OpenVPN client
/// config for this device. The agent picks it up on the next policy apply
/// (an `apply_policy` command is enqueued here; poll agents see the bumped
/// `policy_version`).
pub async fn set_vpn(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
    Json(req): Json<SetVpnReq>,
) -> AppResult<Json<Value>> {
    get_device_row(&st.db, id, admin.tenant_id).await?;

    if req.config.len() > MAX_VPN_CONFIG_BYTES {
        return Err(AppError::BadRequest(format!(
            "VPN config too large (max {} KB)",
            MAX_VPN_CONFIG_BYTES / 1024
        )));
    }
    // Cheap shape check so a wrong-file upload fails here with a clear message
    // instead of on the device with a dead tunnel.
    let ok = match req.kind.as_str() {
        "wireguard" => req.config.contains("[Interface]") && req.config.contains("PrivateKey"),
        "openvpn" => req.config.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with("remote ") || l == "client"
        }),
        _ => {
            return Err(AppError::BadRequest(
                "kind must be 'wireguard' or 'openvpn'".into(),
            ))
        }
    };
    if !ok {
        return Err(AppError::BadRequest(format!(
            "this does not look like a {} client config",
            req.kind
        )));
    }

    sqlx::query(
        "UPDATE devices SET vpn_kind = $1, vpn_config = $2, vpn_updated_at = now()
         WHERE id = $3 AND tenant_id = $4",
    )
    .bind(&req.kind)
    .bind(&req.config)
    .bind(id)
    .bind(admin.tenant_id)
    .execute(&st.db)
    .await?;

    enqueue_command(&st, id, "apply_policy", json!({})).await?;
    events::insert(
        &st.db,
        admin.tenant_id,
        Some(id),
        None,
        "vpn_profile",
        "info",
        json!({ "action": "set", "kind": req.kind, "by": admin.admin_id }),
    )
    .await?;

    let row = get_device_row(&st.db, id, admin.tenant_id).await?;
    Ok(Json(json!({ "device": device_to_json(&row) })))
}

/// DELETE /api/devices/:id/vpn — remove the profile; the agent tears the
/// tunnel down on its next policy apply. `vpn_updated_at` is bumped (not
/// nulled) so the removal propagates to poll-mode agents too.
pub async fn remove_vpn(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    get_device_row(&st.db, id, admin.tenant_id).await?;
    sqlx::query(
        "UPDATE devices SET vpn_kind = NULL, vpn_config = NULL, vpn_updated_at = now()
         WHERE id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(admin.tenant_id)
    .execute(&st.db)
    .await?;

    enqueue_command(&st, id, "apply_policy", json!({})).await?;
    events::insert(
        &st.db,
        admin.tenant_id,
        Some(id),
        None,
        "vpn_profile",
        "info",
        json!({ "action": "removed", "by": admin.admin_id }),
    )
    .await?;

    let row = get_device_row(&st.db, id, admin.tenant_id).await?;
    Ok(Json(json!({ "device": device_to_json(&row) })))
}

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
    // Truthful lock state: only flip status when the command actually reached
    // a live agent. Otherwise it stays queued; the ack path in agent.rs flips
    // the status once the device reconnects and applies the lock.
    let (cmd_id, delivered) = enqueue_command_delivered(&st, id, "lock", json!({})).await?;
    if delivered {
        sqlx::query("UPDATE devices SET status = 'locked' WHERE id = $1")
            .bind(id)
            .execute(&st.db)
            .await?;
    }
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
    // Mirror of lock: status flips immediately only on live delivery, else on ack.
    let (cmd_id, delivered) = enqueue_command_delivered(&st, id, "unlock", json!({})).await?;
    if delivered {
        sqlx::query("UPDATE devices SET status = 'online' WHERE id = $1")
            .bind(id)
            .execute(&st.db)
            .await?;
    }
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
);

pub async fn device_users_json(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<Value> {
    let rows: Vec<DeviceUserRow> = sqlx::query_as(
        "SELECT du.id, du.device_id, du.os_username, du.display_name, du.profile_id, \
                p.name, p.kind, \
                COALESCE(l.used_seconds, 0), COALESCE(l.earned_seconds, 0) \
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
