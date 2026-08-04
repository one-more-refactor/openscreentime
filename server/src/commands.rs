//! Admin visibility + control over the device command queue: list what is
//! pending/recent per device, cancel a command that hasn't been acked yet,
//! and a janitor that keeps the table from growing forever.
//!
//! The queue semantics themselves (enqueue, coalescing, redelivery) live in
//! `agent.rs` next to the delivery paths.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::{AppState, AuthAdmin};

#[derive(Deserialize)]
pub struct ListQuery {
    /// Max rows (default 50, cap 200). Pending rows always sort first.
    pub limit: Option<i64>,
}

type CommandRow = (
    Uuid,
    String,
    Value,
    String,
    Option<Value>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
);

/// GET /api/devices/:id/commands — the device's queue, pending first, then
/// recent history (newest first inside each group).
pub async fn list_for_device(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    ensure_device(&st, id, admin.tenant_id).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let rows: Vec<CommandRow> = sqlx::query_as(
        "SELECT id, type, payload, status, result, created_at, sent_at, acked_at
         FROM commands
         WHERE device_id = $1
         ORDER BY (status IN ('queued','sent')) DESC, created_at DESC
         LIMIT $2",
    )
    .bind(id)
    .bind(limit)
    .fetch_all(&st.db)
    .await?;

    let commands: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.0,
                "type": r.1,
                "payload": r.2,
                "status": r.3,
                "result": r.4,
                "created_at": r.5,
                "sent_at": r.6,
                "acked_at": r.7,
            })
        })
        .collect();
    Ok(Json(json!({ "commands": commands })))
}

/// POST /api/commands/:id/cancel — withdraw a command that hasn't been acked.
/// A `sent` command is cancelled best-effort: if the agent already executed it
/// the ack still lands and overwrites the status, which is the honest record.
pub async fn cancel(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "UPDATE commands SET status = 'cancelled'
         WHERE id = $1
           AND status IN ('queued','sent')
           AND device_id IN (SELECT id FROM devices WHERE tenant_id = $2)
         RETURNING device_id, type",
    )
    .bind(id)
    .bind(admin.tenant_id)
    .fetch_optional(&st.db)
    .await?;

    let (device_id, ctype) =
        row.ok_or_else(|| AppError::NotFound("no pending command to cancel".into()))?;
    Ok(Json(
        json!({ "cancelled": true, "device_id": device_id, "type": ctype }),
    ))
}

/// Daily janitor: settled commands older than 30 days serve no purpose — the
/// event log is the durable audit trail, the queue is operational state.
pub fn spawn_janitor(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        loop {
            tick.tick().await;
            let _ = sqlx::query(
                "DELETE FROM commands
                 WHERE status IN ('acked','failed','cancelled')
                   AND created_at < now() - interval '30 days'",
            )
            .execute(&st.db)
            .await;
        }
    });
}

async fn ensure_device(st: &AppState, id: Uuid, tenant_id: Uuid) -> AppResult<()> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM devices WHERE id = $1 AND tenant_id = $2")
            .bind(id)
            .bind(tenant_id)
            .fetch_optional(&st.db)
            .await?;
    found.ok_or_else(|| AppError::NotFound("device not found".into()))?;
    Ok(())
}
