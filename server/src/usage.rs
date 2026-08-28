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
    // OS logins the agent may attribute app-seconds to on ITS OWN device —
    // so a compromised agent can't misattribute a sibling's usage to another
    // account by inventing an os_username (the where-queries join on it).
    let known_users: std::collections::HashSet<String> = sqlx::query_scalar::<_, String>(
        "SELECT os_username FROM device_users WHERE device_id = $1",
    )
    .bind(agent.device_id)
    .fetch_all(&st.db)
    .await?
    .into_iter()
    .collect();

    // Collect the valid rows, then upsert them in ONE statement via UNNEST —
    // the old per-slice loop held one of only ~10 pool connections for up to
    // 500 serial round-trips, which a few reconnecting long-offline devices
    // could saturate.
    let mut os = Vec::new();
    let mut hours = Vec::new();
    let mut kinds = Vec::new();
    let mut keys = Vec::new();
    let mut amounts = Vec::new();
    for s in &req.slices {
        if !matches!(s.kind.as_str(), "app" | "site") || s.amount <= 0 || s.amount > 86_400 {
            continue;
        }
        // Site slices are device-wide (os_username ""); app slices must name a
        // real OS login of this device.
        if s.kind == "app" && (s.os_username.is_empty() || !known_users.contains(&s.os_username)) {
            continue;
        }
        if s.kind == "site" && !s.os_username.is_empty() {
            continue;
        }
        let key: String = s.key.chars().take(120).collect();
        if key.is_empty() {
            continue;
        }
        os.push(s.os_username.clone());
        hours.push(s.hour);
        kinds.push(s.kind.clone());
        keys.push(key);
        amounts.push(s.amount);
    }
    if !keys.is_empty() {
        sqlx::query(
            "INSERT INTO usage_slices (device_id, tenant_id, os_username, hour, kind, key, amount)
             SELECT $1, $2, u.os, date_trunc('hour', u.hour), u.kind, u.key, u.amount
               FROM UNNEST($3::text[], $4::timestamptz[], $5::text[], $6::text[], $7::bigint[])
                 AS u(os, hour, kind, key, amount)
             ON CONFLICT (device_id, os_username, hour, kind, key)
             DO UPDATE SET amount = usage_slices.amount + EXCLUDED.amount",
        )
        .bind(agent.device_id)
        .bind(agent.tenant_id)
        .bind(&os)
        .bind(&hours)
        .bind(&kinds)
        .bind(&keys)
        .bind(&amounts)
        .execute(&st.db)
        .await?;
    }
    Ok(Json(json!({ "ok": true })))
}

/// Today's attribution for one account: top apps (their own seconds), top
/// sites (their devices' activity), and a 24-slot activity curve.
///
/// KNOWN LIMITATION: "today" here is the **UTC** calendar day, while the
/// screen-time TimeBar comes from the ledger's device-local day. In a
/// non-UTC timezone the two can disagree near local midnight (late-night use
/// straddles the boundary differently). Fixing it properly needs a stored
/// per-device UTC offset; deferred deliberately — this is a soft attribution
/// signal, not the enforced budget. The web still buckets the hour curve into
/// the viewer's local time, so the strip reads correctly within the window.
pub async fn where_for_account(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    account_id: Uuid,
    // When the viewer IS this person (their own /me), device-wide site activity
    // on a SHARED computer leaks siblings'/parents' browsing to them — so it's
    // suppressed there. A parent viewing a child is authorized to see the
    // device's activity, so they pass `false`.
    hide_shared_sites: bool,
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
    // Is any device this person uses shared with a DIFFERENT account?
    let shared: bool = sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM device_users other
            WHERE other.account_id <> $1
              AND other.device_id IN (SELECT device_id FROM device_users WHERE account_id = $1))",
    )
    .bind(account_id)
    .fetch_one(db)
    .await?;
    let sites_hidden = hide_shared_sites && shared;
    let sites: Vec<(String, i64)> = if sites_hidden {
        Vec::new()
    } else {
        sqlx::query_as(
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
        .await?
    };

    // The day's curve: the person's own app-SECONDS per hour (only — mixing in
    // site lookup COUNTS made the intensity unit-soup, where 200 DNS hits
    // dwarfed an hour of real use). Site activity has its own list above.
    let hours: Vec<(DateTime<Utc>, i64)> = sqlx::query_as(
        "SELECT us.hour, SUM(us.amount)::bigint
              FROM usage_slices us
              JOIN device_users du ON du.device_id = us.device_id
                                  AND du.os_username = us.os_username
             WHERE du.account_id = $1 AND us.tenant_id = $2 AND us.kind = 'app'
               AND us.hour >= date_trunc('day', now())
             GROUP BY us.hour ORDER BY us.hour",
    )
    .bind(account_id)
    .bind(tenant_id)
    .fetch_all(db)
    .await?;

    Ok(json!({
        "apps": apps.into_iter().map(|(k, s)| json!({ "key": k, "seconds": s })).collect::<Vec<_>>(),
        "sites": sites.into_iter().map(|(k, n)| json!({ "key": k, "hits": n })).collect::<Vec<_>>(),
        "hours": hours.into_iter().map(|(h, a)| json!({ "hour": h, "amount": a })).collect::<Vec<_>>(),
        // The web shows a "sites hidden — shared computer" note instead of a leak.
        "sites_hidden_shared": sites_hidden,
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
        where_for_account(&st.db, admin.tenant_id, q.account_id, false).await?,
    ))
}

/// `GET /api/me/where` — the person's own view (member-allowed).
pub async fn me_where(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    Ok(Json(
        where_for_account(&st.db, admin.tenant_id, admin.admin_id, true).await?,
    ))
}
