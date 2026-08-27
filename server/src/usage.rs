//! Where the time goes (CONTRACT-0.6 §3).
//!
//! The agent reports per-hour attribution slices: seconds an app was open
//! (per OS user, from catalog process sampling) and DNS-query activity per
//! site/app domain (per device — resolver traffic has no user, and the UI
//! says so). This module ingests them and answers the console's one
//! question: *where did today go?*
//!
//! Honesty notes baked into the shapes:
//! - app seconds mean "open while the session was in use", not foreground
//!   focus — the agent has no compositor-independent way to know focus;
//! - site numbers are query activity, not a stopwatch;
//! - site slices are device-wide, so a shared computer shows the household's
//!   traffic on that machine, labeled as the device's.

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::{AgentAuth, AppState, AuthAdmin};

const MAX_SLICES_PER_POST: usize = 500;

#[derive(Deserialize)]
pub struct SliceIn {
    #[serde(default)]
    pub os_username: String,
    pub hour: DateTime<Utc>,
    pub kind: String,
    pub key: String,
    pub amount: i64,
}

#[derive(Deserialize)]
pub struct IngestReq {
    #[serde(default)]
    pub slices: Vec<SliceIn>,
}

/// `POST /agent/usage` — upsert-sum a batch of slices. The agent is a
/// semi-trusted origin: shapes are validated, sizes bounded, keys clamped.
pub async fn ingest(
    State(st): State<AppState>,
    agent: AgentAuth,
    Json(req): Json<IngestReq>,
) -> AppResult<Json<Value>> {
    if req.slices.len() > MAX_SLICES_PER_POST {
        return Err(AppError::BadRequest("too many slices in one post".into()));
    }
    for s in &req.slices {
        if !matches!(s.kind.as_str(), "app" | "site") || s.amount <= 0 || s.amount > 86_400 {
            continue;
        }
        let key: String = s.key.chars().take(120).collect();
        if key.is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT INTO usage_slices (device_id, tenant_id, os_username, hour, kind, key, amount)
             VALUES ($1, $2, $3, date_trunc('hour', $4::timestamptz), $5, $6, $7)
             ON CONFLICT (device_id, os_username, hour, kind, key)
             DO UPDATE SET amount = usage_slices.amount + EXCLUDED.amount",
        )
        .bind(agent.device_id)
        .bind(agent.tenant_id)
        .bind(&s.os_username)
        .bind(s.hour)
        .bind(&s.kind)
        .bind(&key)
        .bind(s.amount)
        .execute(&st.db)
        .await?;
    }
    Ok(Json(json!({ "ok": true })))
}

/// Today's attribution for one account: top apps (their own seconds), top
/// sites (their devices' activity), and a 24-slot activity curve.
pub async fn where_for_account(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    account_id: Uuid,
) -> AppResult<Value> {
    // Apps: the person's own OS logins, summed across their devices.
    let apps: Vec<(String, i64)> = sqlx::query_as(
        "SELECT us.key, SUM(us.amount)::bigint
           FROM usage_slices us
           JOIN device_users du ON du.device_id = us.device_id
                               AND du.os_username = us.os_username
          WHERE du.account_id = $1 AND us.tenant_id = $2
            AND us.kind = 'app' AND us.hour >= date_trunc('day', now())
          GROUP BY us.key ORDER BY 2 DESC LIMIT 12",
    )
    .bind(account_id)
    .bind(tenant_id)
    .fetch_all(db)
    .await?;

    // Sites: device-wide on the machines this person uses (resolver traffic
    // has no user — the UI labels it as the computer's).
    let sites: Vec<(String, i64)> = sqlx::query_as(
        "SELECT us.key, SUM(us.amount)::bigint
           FROM usage_slices us
          WHERE us.tenant_id = $2 AND us.kind = 'site' AND us.os_username = ''
            AND us.hour >= date_trunc('day', now())
            AND us.device_id IN (SELECT device_id FROM device_users WHERE account_id = $1)
          GROUP BY us.key ORDER BY 2 DESC LIMIT 12",
    )
    .bind(account_id)
    .bind(tenant_id)
    .fetch_all(db)
    .await?;

    // The day's curve: everything attributable to them or their machines,
    // as raw hourly totals the web buckets into local time.
    let hours: Vec<(DateTime<Utc>, i64)> = sqlx::query_as(
        "SELECT h, SUM(a)::bigint FROM (
            SELECT us.hour AS h, us.amount AS a
              FROM usage_slices us
              JOIN device_users du ON du.device_id = us.device_id
                                  AND du.os_username = us.os_username
             WHERE du.account_id = $1 AND us.tenant_id = $2 AND us.kind = 'app'
               AND us.hour >= date_trunc('day', now())
            UNION ALL
            SELECT us.hour, us.amount
              FROM usage_slices us
             WHERE us.tenant_id = $2 AND us.kind = 'site' AND us.os_username = ''
               AND us.hour >= date_trunc('day', now())
               AND us.device_id IN (SELECT device_id FROM device_users WHERE account_id = $1)
         ) t GROUP BY h ORDER BY h",
    )
    .bind(account_id)
    .bind(tenant_id)
    .fetch_all(db)
    .await?;

    Ok(json!({
        "apps": apps.into_iter().map(|(k, s)| json!({ "key": k, "seconds": s })).collect::<Vec<_>>(),
        "sites": sites.into_iter().map(|(k, n)| json!({ "key": k, "hits": n })).collect::<Vec<_>>(),
        "hours": hours.into_iter().map(|(h, a)| json!({ "hour": h, "amount": a })).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
pub struct WhereQuery {
    pub account_id: Uuid,
}

/// `GET /api/usage/where?account_id=` — the parent's view of a person's day.
pub async fn where_api(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Query(q): Query<WhereQuery>,
) -> AppResult<Json<Value>> {
    // Scope: the account must be of this tenant.
    let owned: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM admins WHERE id = $1 AND tenant_id = $2")
            .bind(q.account_id)
            .bind(admin.tenant_id)
            .fetch_optional(&st.db)
            .await?;
    owned.ok_or_else(|| AppError::NotFound("no such person".into()))?;
    Ok(Json(
        where_for_account(&st.db, admin.tenant_id, q.account_id).await?,
    ))
}

/// `GET /api/me/where` — the person's own view (member-allowed).
pub async fn me_where(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    Ok(Json(
        where_for_account(&st.db, admin.tenant_id, admin.admin_id).await?,
    ))
}
