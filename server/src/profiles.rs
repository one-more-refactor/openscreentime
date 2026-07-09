//! Admin profile CRUD. Presets are editable in place; custom profiles are
//! freely created/deleted. Policy is validated by round-tripping through the
//! shared `Policy` type.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::enqueue_command;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, AuthAdmin};
use sentinel_policy::Policy;

type ProfileRow = (
    Uuid,
    Uuid,
    String,
    String,
    bool,
    Value,
    DateTime<Utc>,
    DateTime<Utc>,
);

const PROFILE_COLS: &str = "id, tenant_id, name, kind, is_preset, policy, created_at, updated_at";

fn profile_to_json(r: ProfileRow) -> Value {
    json!({
        "id": r.0,
        "tenant_id": r.1,
        "name": r.2,
        "kind": r.3,
        "is_preset": r.4,
        "policy": r.5,
        "created_at": r.6,
        "updated_at": r.7,
    })
}

/// Validate a raw policy value by deserializing into the shared `Policy` type
/// (forward-compat: unknown fields are tolerated), then re-serialize so what we
/// store is canonical.
fn normalize_policy(v: Value) -> AppResult<Value> {
    let p: Policy = serde_json::from_value(v)
        .map_err(|e| AppError::BadRequest(format!("invalid policy: {e}")))?;
    Ok(serde_json::to_value(p).unwrap())
}

pub async fn list_profiles(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let rows: Vec<ProfileRow> = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM profiles WHERE tenant_id = $1 \
         ORDER BY is_preset DESC, name"
    ))
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(json!({
        "profiles": rows.into_iter().map(profile_to_json).collect::<Vec<_>>()
    })))
}

pub async fn get_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row: Option<ProfileRow> = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM profiles WHERE id = $1 AND tenant_id = $2"
    ))
    .bind(id)
    .bind(admin.tenant_id)
    .fetch_optional(&st.db)
    .await?;
    let row = row.ok_or_else(|| AppError::NotFound("profile not found".into()))?;
    Ok(Json(json!({ "profile": profile_to_json(row) })))
}

#[derive(Deserialize)]
pub struct CreateProfileReq {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    pub policy: Value,
}

pub async fn create_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Json(req): Json<CreateProfileReq>,
) -> AppResult<Json<Value>> {
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    // New profiles are always `custom` (presets are seeded only at tenant
    // creation). We accept the field but pin it.
    let kind = req.kind.unwrap_or_else(|| "custom".into());
    if kind != "custom" {
        return Err(AppError::BadRequest(
            "only custom profiles may be created".into(),
        ));
    }
    let policy = normalize_policy(req.policy)?;

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO profiles (tenant_id, name, kind, is_preset, policy)
         VALUES ($1, $2, 'custom', false, $3) RETURNING id",
    )
    .bind(admin.tenant_id)
    .bind(&req.name)
    .bind(&policy)
    .fetch_one(&st.db)
    .await?;

    let row: ProfileRow = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM profiles WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    Ok(Json(json!({ "profile": profile_to_json(row) })))
}

#[derive(Deserialize)]
pub struct UpdateProfileReq {
    pub name: Option<String>,
    pub policy: Option<Value>,
}

pub async fn update_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateProfileReq>,
) -> AppResult<Json<Value>> {
    // Presets are editable in place (edit mutates policy, preset row stays).
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM profiles WHERE id = $1 AND tenant_id = $2")
            .bind(id)
            .bind(admin.tenant_id)
            .fetch_optional(&st.db)
            .await?;
    existing.ok_or_else(|| AppError::NotFound("profile not found".into()))?;

    if let Some(name) = &req.name {
        sqlx::query("UPDATE profiles SET name = $1, updated_at = now() WHERE id = $2")
            .bind(name)
            .bind(id)
            .execute(&st.db)
            .await?;
    }
    if let Some(policy) = req.policy {
        let policy = normalize_policy(policy)?;
        sqlx::query("UPDATE profiles SET policy = $1, updated_at = now() WHERE id = $2")
            .bind(&policy)
            .bind(id)
            .execute(&st.db)
            .await?;

        // Policy changed: tell every device with a user on this profile to
        // re-pull (WS agents get it pushed; poll agents also catch up via the
        // heartbeat policy_version).
        let device_ids: Vec<(Uuid,)> =
            sqlx::query_as("SELECT DISTINCT device_id FROM device_users WHERE profile_id = $1")
                .bind(id)
                .fetch_all(&st.db)
                .await?;
        for (device_id,) in device_ids {
            enqueue_command(&st, device_id, "apply_policy", json!({})).await?;
        }
    }

    let row: ProfileRow = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM profiles WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    Ok(Json(json!({ "profile": profile_to_json(row) })))
}

pub async fn delete_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT is_preset FROM profiles WHERE id = $1 AND tenant_id = $2")
            .bind(id)
            .bind(admin.tenant_id)
            .fetch_optional(&st.db)
            .await?;
    let is_preset = row
        .ok_or_else(|| AppError::NotFound("profile not found".into()))?
        .0;
    if is_preset {
        return Err(AppError::BadRequest(
            "preset profiles cannot be deleted".into(),
        ));
    }

    // Guard against deleting a profile still in use.
    let in_use: i64 = sqlx::query_scalar("SELECT count(*) FROM device_users WHERE profile_id = $1")
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    if in_use > 0 {
        return Err(AppError::Conflict(
            "profile is assigned to device users".into(),
        ));
    }

    sqlx::query("DELETE FROM profiles WHERE id = $1 AND tenant_id = $2")
        .bind(id)
        .bind(admin.tenant_id)
        .execute(&st.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}
