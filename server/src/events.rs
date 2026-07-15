//! Event insert helpers + the admin events/audit list endpoint.

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppResult;
use crate::state::{AppState, AuthAdmin};

/// Insert one event row.
pub async fn insert(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    device_id: Option<Uuid>,
    device_user_id: Option<Uuid>,
    etype: &str,
    severity: &str,
    payload: Value,
) -> AppResult<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO events (tenant_id, device_id, device_user_id, type, severity, payload)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(tenant_id)
    .bind(device_id)
    .bind(device_user_id)
    .bind(etype)
    .bind(severity)
    .bind(&payload)
    .fetch_one(db)
    .await?;
    Ok(id)
}

type EventRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    String,
    String,
    Value,
    DateTime<Utc>,
);

fn event_to_json(r: EventRow) -> Value {
    json!({
        "id": r.0,
        "tenant_id": r.1,
        "device_id": r.2,
        "device_user_id": r.3,
        "type": r.4,
        "severity": r.5,
        "payload": r.6,
        "created_at": r.7,
    })
}

pub async fn recent_for_device(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
    limit: i64,
) -> AppResult<Value> {
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, tenant_id, device_id, device_user_id, type, severity, payload, created_at
         FROM events WHERE tenant_id = $1 AND device_id = $2
         ORDER BY created_at DESC LIMIT $3",
    )
    .bind(tenant_id)
    .bind(device_id)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(json!(rows
        .into_iter()
        .map(event_to_json)
        .collect::<Vec<_>>()))
}

/// Recent noteworthy events for a tenant — warnings and criticals only
/// (tamper, evasion, locks, screen-time exceeded, etc.), newest first. This is
/// what the parent companion polls for its alerts feed.
pub async fn recent_alerts(db: &sqlx::PgPool, tenant_id: Uuid, limit: i64) -> AppResult<Value> {
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, tenant_id, device_id, device_user_id, type, severity, payload, created_at
         FROM events
         WHERE tenant_id = $1 AND severity IN ('warn','critical')
         ORDER BY created_at DESC LIMIT $2",
    )
    .bind(tenant_id)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(json!(rows
        .into_iter()
        .map(event_to_json)
        .collect::<Vec<_>>()))
}

#[derive(Deserialize)]
pub struct EventsQuery {
    pub device_id: Option<Uuid>,
    pub r#type: Option<String>,
    pub severity: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list_events(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Query(q): Query<EventsQuery>,
) -> AppResult<Json<Value>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    // Dynamic filters via COALESCE-style optional binds.
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, tenant_id, device_id, device_user_id, type, severity, payload, created_at
         FROM events
         WHERE tenant_id = $1
           AND ($2::uuid IS NULL OR device_id = $2)
           AND ($3::text IS NULL OR type = $3)
           AND ($4::text IS NULL OR severity = $4)
         ORDER BY created_at DESC
         LIMIT $5",
    )
    .bind(admin.tenant_id)
    .bind(q.device_id)
    .bind(q.r#type)
    .bind(q.severity)
    .bind(limit)
    .fetch_all(&st.db)
    .await?;

    Ok(Json(json!({
        "events": rows.into_iter().map(event_to_json).collect::<Vec<_>>()
    })))
}
