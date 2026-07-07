//! LAN discovery: an admin asks an enrolled agent to scan its subnet; results
//! come back as `discovery_result` events (see TAMPER.md → Device discovery).

use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::enqueue_command;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, AuthAdmin};

#[derive(Deserialize)]
pub struct ScanReq {
    pub device_id: Uuid,
}

pub async fn scan(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Json(req): Json<ScanReq>,
) -> AppResult<Json<Value>> {
    // Verify the device belongs to the tenant.
    let owner: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM devices WHERE id = $1 AND tenant_id = $2")
            .bind(req.device_id)
            .bind(admin.tenant_id)
            .fetch_optional(&st.db)
            .await?;
    owner.ok_or_else(|| AppError::NotFound("device not found".into()))?;

    // Only on explicit admin trigger — no unsolicited scanning.
    let cmd_id = enqueue_command(&st, req.device_id, "discover", json!({})).await?;
    Ok(Json(json!({ "command_id": cmd_id })))
}

pub async fn results(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, Option<Uuid>, Value, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, device_id, payload, created_at FROM events
         WHERE tenant_id = $1 AND type = 'discovery_result'
         ORDER BY created_at DESC LIMIT 100",
    )
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;

    let results: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.0,
                "device_id": r.1,
                "payload": r.2,
                "created_at": r.3,
            })
        })
        .collect();
    Ok(Json(json!({ "results": results })))
}
