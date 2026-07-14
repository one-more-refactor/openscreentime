//! Earn-time approval flow: the agent files an `earn_request` when a user
//! picks an earn offer on the lockout screen; a parent approves/denies it in
//! the web UI. Approval credits `screen_time_ledger.earned_seconds` and pushes
//! a `credit_time` command to the device. Every step is audited via
//! `earn_request` events.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::enqueue_command;
use crate::error::{AppError, AppResult};
use crate::events;
use crate::state::{AgentAuth, AppState, AuthAdmin};

const MAX_MINUTES: i32 = 240;

/// Full earn-request row joined with device + user names for the admin UI.
type RequestRow = (
    Uuid,                  // id
    Uuid,                  // device_id
    String,                // device name
    Uuid,                  // device_user_id
    String,                // os_username
    Option<String>,        // user display_name
    String,                // task_id
    String,                // task_label
    i32,                   // minutes
    String,                // status
    DateTime<Utc>,         // created_at
    Option<DateTime<Utc>>, // decided_at
);

const REQUEST_COLS: &str = "er.id, er.device_id, d.name, er.device_user_id, du.os_username, \
    du.display_name, er.task_id, er.task_label, er.minutes, er.status, er.created_at, \
    er.decided_at";

fn request_to_json(r: RequestRow) -> Value {
    json!({
        "id": r.0,
        "device_id": r.1,
        "device_name": r.2,
        "device_user_id": r.3,
        "os_username": r.4,
        "user_display_name": r.5,
        "task_id": r.6,
        "task_label": r.7,
        "minutes": r.8,
        "status": r.9,
        "created_at": r.10,
        "decided_at": r.11,
    })
}

async fn fetch_request(db: &sqlx::PgPool, id: Uuid, tenant_id: Uuid) -> AppResult<RequestRow> {
    let row: Option<RequestRow> = sqlx::query_as(&format!(
        "SELECT {REQUEST_COLS} FROM earn_requests er
         JOIN devices d ON d.id = er.device_id
         JOIN device_users du ON du.id = er.device_user_id
         WHERE er.id = $1 AND er.tenant_id = $2"
    ))
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(db)
    .await?;
    row.ok_or_else(|| AppError::NotFound("earn request not found".into()))
}

// ---------------------------------------------------------------------------
// Agent side
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct EarnRequestReq {
    pub os_username: String,
    pub task_id: String,
    pub task_label: String,
    pub minutes: i32,
}

/// POST /agent/earn-request — file (or re-fetch) today's pending request for
/// this (user, task). One open request per (user, task) per day: a duplicate
/// returns the existing pending row instead of creating another.
pub async fn create_request(
    State(st): State<AppState>,
    agent: AgentAuth,
    Json(req): Json<EarnRequestReq>,
) -> AppResult<Json<Value>> {
    if req.minutes <= 0 || req.minutes > MAX_MINUTES {
        return Err(AppError::BadRequest(format!(
            "minutes must be between 1 and {MAX_MINUTES}"
        )));
    }
    let device_user_id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM device_users WHERE device_id = $1 AND os_username = $2")
            .bind(agent.device_id)
            .bind(&req.os_username)
            .fetch_optional(&st.db)
            .await?;
    let device_user_id =
        device_user_id.ok_or_else(|| AppError::NotFound("unknown os user".into()))?;

    // Dedupe: return today's existing pending row for this (user, task).
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM earn_requests
         WHERE device_user_id = $1 AND task_id = $2 AND status = 'pending'
           AND created_at::date = CURRENT_DATE",
    )
    .bind(device_user_id)
    .bind(&req.task_id)
    .fetch_optional(&st.db)
    .await?;

    let id = match existing {
        Some(id) => id,
        None => {
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO earn_requests
                     (tenant_id, device_id, device_user_id, task_id, task_label, minutes)
                 VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
            )
            .bind(agent.tenant_id)
            .bind(agent.device_id)
            .bind(device_user_id)
            .bind(&req.task_id)
            .bind(&req.task_label)
            .bind(req.minutes)
            .fetch_one(&st.db)
            .await?;

            events::insert(
                &st.db,
                agent.tenant_id,
                Some(agent.device_id),
                Some(device_user_id),
                "earn_request",
                "info",
                json!({
                    "action": "requested",
                    "request_id": id,
                    "task_id": req.task_id,
                    "task_label": req.task_label,
                    "minutes": req.minutes,
                }),
            )
            .await?;
            id
        }
    };

    let row = fetch_request(&st.db, id, agent.tenant_id).await?;
    Ok(Json(json!({ "request": request_to_json(row) })))
}

// ---------------------------------------------------------------------------
// Admin side
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreditTimeReq {
    pub minutes: i32,
}

/// POST /api/device-users/:id/credit-time — a parent grants extra screen time
/// today, no earn request required. Same mechanics as an approved request:
/// upsert today's ledger row + enqueue a `credit_time` command for the agent
/// (`request_id: null` — there is no earn request to resolve). Audited as an
/// `earn_request` event with `action: "granted"`.
pub async fn credit_time(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(device_user_id): Path<Uuid>,
    Json(req): Json<CreditTimeReq>,
) -> AppResult<Json<Value>> {
    if req.minutes <= 0 || req.minutes > MAX_MINUTES {
        return Err(AppError::BadRequest(format!(
            "minutes must be between 1 and {MAX_MINUTES}"
        )));
    }

    // Verify the device_user belongs to a device in this tenant.
    let owner: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT d.id, du.os_username FROM device_users du
         JOIN devices d ON d.id = du.device_id
         WHERE du.id = $1 AND d.tenant_id = $2",
    )
    .bind(device_user_id)
    .bind(admin.tenant_id)
    .fetch_optional(&st.db)
    .await?;
    let (device_id, os_username) =
        owner.ok_or_else(|| AppError::NotFound("device user not found".into()))?;

    // Credit the ledger for today (upsert on (device_user_id, day)).
    sqlx::query(
        "INSERT INTO screen_time_ledger (device_user_id, day, earned_seconds)
         VALUES ($1, CURRENT_DATE, $2)
         ON CONFLICT (device_user_id, day)
         DO UPDATE SET earned_seconds = screen_time_ledger.earned_seconds
                       + EXCLUDED.earned_seconds",
    )
    .bind(device_user_id)
    .bind(req.minutes * 60)
    .execute(&st.db)
    .await?;

    enqueue_command(
        &st,
        device_id,
        "credit_time",
        json!({ "os_username": os_username, "minutes": req.minutes, "request_id": null }),
    )
    .await?;

    events::insert(
        &st.db,
        admin.tenant_id,
        Some(device_id),
        Some(device_user_id),
        "earn_request",
        "info",
        json!({ "action": "granted", "minutes": req.minutes, "by": admin.admin_id }),
    )
    .await?;

    Ok(Json(json!({ "ok": true, "minutes": req.minutes })))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
}

/// GET /api/earn-requests?status= — newest first, joined with device + user.
pub async fn list_requests(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    let rows: Vec<RequestRow> = sqlx::query_as(&format!(
        "SELECT {REQUEST_COLS} FROM earn_requests er
         JOIN devices d ON d.id = er.device_id
         JOIN device_users du ON du.id = er.device_user_id
         WHERE er.tenant_id = $1 AND ($2::text IS NULL OR er.status = $2)
         ORDER BY er.created_at DESC LIMIT 200"
    ))
    .bind(admin.tenant_id)
    .bind(q.status)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(json!({
        "requests": rows.into_iter().map(request_to_json).collect::<Vec<_>>()
    })))
}

/// POST /api/earn-requests/:id/approve
pub async fn approve_request(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    decide(st, admin, id, true).await
}

/// POST /api/earn-requests/:id/deny
pub async fn deny_request(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    decide(st, admin, id, false).await
}

async fn decide(st: AppState, admin: AuthAdmin, id: Uuid, approve: bool) -> AppResult<Json<Value>> {
    let status = if approve { "approved" } else { "denied" };

    let updated: Option<(Uuid, Uuid, i32)> = sqlx::query_as(
        "UPDATE earn_requests SET status = $1, decided_at = now()
         WHERE id = $2 AND tenant_id = $3 AND status = 'pending'
         RETURNING device_id, device_user_id, minutes",
    )
    .bind(status)
    .bind(id)
    .bind(admin.tenant_id)
    .fetch_optional(&st.db)
    .await?;

    let Some((device_id, device_user_id, minutes)) = updated else {
        // Distinguish "gone" from "already decided".
        fetch_request(&st.db, id, admin.tenant_id).await?;
        return Err(AppError::Conflict("earn request already decided".into()));
    };

    if approve {
        // Credit the ledger for today (upsert on (device_user_id, day)).
        sqlx::query(
            "INSERT INTO screen_time_ledger (device_user_id, day, earned_seconds)
             VALUES ($1, CURRENT_DATE, $2)
             ON CONFLICT (device_user_id, day)
             DO UPDATE SET earned_seconds = screen_time_ledger.earned_seconds
                           + EXCLUDED.earned_seconds",
        )
        .bind(device_user_id)
        .bind(minutes * 60)
        .execute(&st.db)
        .await?;

        let os_username: String =
            sqlx::query_scalar("SELECT os_username FROM device_users WHERE id = $1")
                .bind(device_user_id)
                .fetch_one(&st.db)
                .await?;
        enqueue_command(
            &st,
            device_id,
            "credit_time",
            json!({ "os_username": os_username, "minutes": minutes, "request_id": id }),
        )
        .await?;
    } else {
        // Mirror of the approve path: tell the agent about the denial so it
        // can clear its once-per-day dedupe (the teen may re-ask) and replace
        // the stale "WAITING FOR APPROVAL" copy with an honest answer.
        let (os_username, task_id): (String, String) = sqlx::query_as(
            "SELECT du.os_username, er.task_id
             FROM earn_requests er JOIN device_users du ON du.id = er.device_user_id
             WHERE er.id = $1",
        )
        .bind(id)
        .fetch_one(&st.db)
        .await?;
        enqueue_command(
            &st,
            device_id,
            "deny_earn",
            json!({ "os_username": os_username, "task_id": task_id, "request_id": id }),
        )
        .await?;
    }

    events::insert(
        &st.db,
        admin.tenant_id,
        Some(device_id),
        Some(device_user_id),
        "earn_request",
        "info",
        json!({ "action": status, "request_id": id, "minutes": minutes, "by": admin.admin_id }),
    )
    .await?;

    let row = fetch_request(&st.db, id, admin.tenant_id).await?;
    Ok(Json(json!({ "request": request_to_json(row) })))
}
