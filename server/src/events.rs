//! Event insert helpers + the admin events/audit list endpoint.

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use openscreentime_policy::AgeBracket;
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

/// The event types the database accepts — mirrors `events_type_check`
/// (migration 0026). Keep the two in step.
const KNOWN_TYPES: &[&str] = &[
    "heartbeat",
    "tamper",
    "lock",
    "unlock",
    "policy_applied",
    "screen_time_exceeded",
    "screen_time_earned",
    "enrolled",
    "ssh",
    "earn_request",
    "evasion",
    "enforcement_degraded",
    "vpn_profile",
    "parent_code_ok",
    "parent_code_failed",
    "parent_code_backup_used",
    "app_blocked",
    "member",
    "account_login",
    "login_approval",
    "other",
];

/// A type this server doesn't know (a newer agent's) becomes `other`, with
/// the original kept in the payload — instead of failing the CHECK and, with
/// it, the whole batch the agent will then retry forever.
pub fn normalize_type(etype: &str, payload: Value) -> (String, Value) {
    if KNOWN_TYPES.contains(&etype) {
        return (etype.to_string(), payload);
    }
    let original: String = etype.chars().take(64).collect();
    let payload = match payload {
        Value::Object(mut map) => {
            map.insert("original_type".into(), Value::String(original));
            Value::Object(map)
        }
        other => json!({ "original_type": original, "payload": other }),
    };
    ("other".to_string(), payload)
}

/// Insert one event from an agent. `client_id` — minted on the device and
/// stable across its retries — makes this idempotent: a batch whose response
/// was lost can be re-sent without duplicating a single row (or re-alerting a
/// critical one). Returns whether the row is new.
#[allow(clippy::too_many_arguments)]
pub async fn insert_from_agent<'e, E>(
    exec: E,
    tenant_id: Uuid,
    device_id: Uuid,
    device_user_id: Option<Uuid>,
    client_id: Option<Uuid>,
    etype: &str,
    severity: &str,
    payload: &Value,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO events
             (tenant_id, device_id, device_user_id, type, severity, payload, client_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (device_id, client_id) WHERE client_id IS NOT NULL DO NOTHING
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(device_id)
    .bind(device_user_id)
    .bind(etype)
    .bind(severity)
    .bind(payload)
    .bind(client_id)
    .fetch_optional(exec)
    .await?;
    Ok(id.is_some())
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

/// The logins whose events `viewer` doesn't get to read: those of people the
/// hub sees only the minutes of (`usage::hub_exposure` — adults, co-parents,
/// anyone who manages themselves). Their moments are theirs, like their apps
/// and sites. A viewer is never hidden from their own events, and events
/// with no login (the computer's own: a tamper, a pause) stay visible — they
/// are about the machine, not the person.
pub async fn private_logins(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    viewer: Uuid,
) -> AppResult<Vec<Uuid>> {
    let rows: Vec<(Uuid, String, bool)> = sqlx::query_as(
        "SELECT du.id, a.age_bracket, a.self_managed
           FROM device_users du
           JOIN devices d ON d.id = du.device_id
           JOIN admins a ON a.id = du.account_id
          WHERE d.tenant_id = $1 AND a.id <> $2",
    )
    .bind(tenant_id)
    .bind(viewer)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(_, bracket, self_managed)| {
            let bracket = AgeBracket::parse(bracket).unwrap_or(AgeBracket::Adult);
            crate::usage::hub_exposure(bracket, *self_managed).is_none()
        })
        .map(|(id, _, _)| id)
        .collect())
}

/// A computer's recent events, as `viewer` may read them (see
/// [`private_logins`]).
pub async fn recent_for_device(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    viewer: Uuid,
    device_id: Uuid,
    limit: i64,
) -> AppResult<Value> {
    let hidden = private_logins(db, tenant_id, viewer).await?;
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, tenant_id, device_id, device_user_id, type, severity, payload, created_at
         FROM events WHERE tenant_id = $1 AND device_id = $2
           AND (device_user_id IS NULL OR NOT (device_user_id = ANY($4)))
         ORDER BY created_at DESC LIMIT $3",
    )
    .bind(tenant_id)
    .bind(device_id)
    .bind(limit)
    .bind(&hidden)
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
    // An adult's moments are theirs: the hub sees their minutes, not this.
    let hidden = private_logins(&st.db, admin.tenant_id, admin.admin_id).await?;
    // Dynamic filters via COALESCE-style optional binds.
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, tenant_id, device_id, device_user_id, type, severity, payload, created_at
         FROM events
         WHERE tenant_id = $1
           AND ($2::uuid IS NULL OR device_id = $2)
           AND ($3::text IS NULL OR type = $3)
           AND ($4::text IS NULL OR severity = $4)
           AND (device_user_id IS NULL OR NOT (device_user_id = ANY($6)))
         ORDER BY created_at DESC
         LIMIT $5",
    )
    .bind(admin.tenant_id)
    .bind(q.device_id)
    .bind(q.r#type)
    .bind(q.severity)
    .bind(limit)
    .bind(&hidden)
    .fetch_all(&st.db)
    .await?;

    Ok(Json(json!({
        "events": rows.into_iter().map(event_to_json).collect::<Vec<_>>()
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_types_become_other_and_keep_their_name() {
        let (t, p) = normalize_type("tamper", json!({"message": "x"}));
        assert_eq!(t, "tamper");
        assert_eq!(p, json!({"message": "x"}));

        let (t, p) = normalize_type("brand_new_thing", json!({"message": "x"}));
        assert_eq!(t, "other");
        assert_eq!(p["original_type"], "brand_new_thing");
        assert_eq!(p["message"], "x");

        let (t, p) = normalize_type("weird", json!(42));
        assert_eq!(t, "other");
        assert_eq!(p, json!({"original_type": "weird", "payload": 42}));
    }

    /// The list above and the CHECK in the migration must agree, or a "known"
    /// type still fails the insert.
    #[test]
    fn known_types_match_the_database_check() {
        let sql = include_str!("../migrations/0026_appliance.sql");
        let start = sql.find("CHECK (type IN").expect("check in migration");
        let end = start + sql[start..].find("));").expect("end of check");
        let in_db: std::collections::BTreeSet<&str> = sql[start..end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .flat_map(|l| l.split('\'').skip(1).step_by(2))
            .collect();
        let here: std::collections::BTreeSet<&str> = KNOWN_TYPES.iter().copied().collect();
        assert_eq!(in_db, here);
    }
}
