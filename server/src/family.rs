//! `GET /api/family` — the entire home screen in one request.
//!
//! The console used to assemble this in the browser: list devices, list
//! profiles, then one `GET /api/devices/{id}/users` **per device**, then the
//! pending earn requests. A three-device family cost about a dozen round trips,
//! and because both the navigation rail and the page mounted the same hook
//! independently, it paid that bill twice on every navigation.
//!
//! This endpoint answers the same question in a fixed five queries, no matter
//! how many devices a family has, and does the grouping server-side so the rail
//! and the page cannot disagree about who is over their limit.
//!
//! A "child" here is a person, not a row: the same OS username on two machines
//! is one person whose day is the sum of both.

use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use crate::devices::{device_to_json, DeviceRow, DEVICE_COLS};
use crate::error::AppResult;
use crate::state::{AppState, AuthAdmin};

/// One device_user joined to its profile and today's ledger row.
type FamilyUserRow = (
    Uuid,           // du.id
    Uuid,           // du.device_id
    String,         // du.os_username
    Option<String>, // du.display_name
    Option<Uuid>,   // du.profile_id
    Option<String>, // p.name
    Option<String>, // p.kind
    Option<Value>,  // p.policy
    i32,            // used_seconds today  (ledger columns are int4)
    i32,            // earned_seconds today
);

/// A person, assembled across every device they use.
struct Child {
    key: String,
    name: String,
    used_minutes: i64,
    earned_minutes: i64,
    limit_minutes: Option<i64>,
    profile_id: Option<Uuid>,
    profile_name: Option<String>,
    devices: Vec<Value>,
    pending_requests: usize,
}

/// The daily limit that actually applies, or None for "no limit".
///
/// A disabled or zero limit is *no limit*, never "0 left of 0" — the console
/// renders that difference and getting it wrong tells a parent their child is
/// out of time when no limit was ever set.
fn limit_from_policy(policy: Option<&Value>) -> Option<i64> {
    let st = policy?.get("screen_time")?;
    if st.get("enabled")?.as_bool() != Some(true) {
        return None;
    }
    match st.get("daily_limit_minutes").and_then(Value::as_i64) {
        Some(m) if m > 0 => Some(m),
        _ => None,
    }
}

pub async fn get_family(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    // 1. Devices.
    let device_rows: Vec<DeviceRow> = sqlx::query_as(&format!(
        "SELECT {DEVICE_COLS} FROM devices WHERE tenant_id = $1 ORDER BY created_at DESC"
    ))
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;

    // 2. Every device_user in the tenant, with its profile and today's usage.
    //    The old code issued this once per device.
    let user_rows: Vec<FamilyUserRow> = sqlx::query_as(
        "SELECT du.id, du.device_id, du.os_username, du.display_name, du.profile_id,
                p.name, p.kind, p.policy,
                COALESCE(l.used_seconds, 0), COALESCE(l.earned_seconds, 0)
           FROM device_users du
           JOIN devices d ON d.id = du.device_id
           LEFT JOIN profiles p ON p.id = du.profile_id
           LEFT JOIN screen_time_ledger l
                  ON l.device_user_id = du.id AND l.day = CURRENT_DATE
          WHERE d.tenant_id = $1
          ORDER BY du.os_username",
    )
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;

    // 3. Pending commands for the whole tenant, grouped per device.
    let cmd_rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT c.device_id, c.type FROM commands c
           JOIN devices d ON d.id = c.device_id
          WHERE d.tenant_id = $1 AND c.status IN ('queued','sent')
          ORDER BY c.created_at",
    )
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;
    let mut pending: HashMap<Uuid, Vec<String>> = HashMap::new();
    for (device_id, ctype) in cmd_rows {
        pending.entry(device_id).or_default().push(ctype);
    }

    // 4. Profiles (the rules editor needs the full list).
    let profiles = crate::profiles::list_for_tenant(&st.db, admin.tenant_id).await?;

    // 5. Earn requests still waiting on a parent.
    let requests = crate::earn::list_for_tenant(&st.db, admin.tenant_id, Some("pending".into()))
        .await?
        .get("requests")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let mut asks_by_user: HashMap<String, usize> = HashMap::new();
    if let Some(list) = requests.as_array() {
        for r in list {
            if let Some(u) = r.get("os_username").and_then(Value::as_str) {
                *asks_by_user.entry(u.to_string()).or_default() += 1;
            }
        }
    }

    // Devices, with liveness and pending chips folded in.
    let mut devices_json = Vec::with_capacity(device_rows.len());
    let mut device_meta: HashMap<Uuid, (String, String)> = HashMap::new();
    for r in &device_rows {
        let mut d = device_to_json(r);
        d["online"] = json!(st.hub.is_online(r.0).await);
        d["pending_commands"] = json!(pending.get(&r.0).cloned().unwrap_or_default());
        device_meta.insert(r.0, (r.2.clone(), r.6.clone()));
        devices_json.push(d);
    }

    // Group users into people. Insertion order follows the query's ORDER BY,
    // so the result is already sorted by username; display name only reorders
    // it at the end.
    let mut by_key: Vec<Child> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for u in user_rows {
        let (id, device_id, os_username, display_name, profile_id, pname, _pkind, policy, used, earned) =
            u;
        let name = display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&os_username)
            .to_string();
        let (dev_name, dev_status) = device_meta
            .get(&device_id)
            .cloned()
            .unwrap_or_else(|| ("unknown".into(), "offline".into()));
        let entry = json!({
            "device_user_id": id,
            "id": device_id,
            "name": dev_name,
            "status": dev_status,
        });

        match index.get(&os_username) {
            // Same person, second machine: their day is the sum of both.
            Some(&i) => {
                let c = &mut by_key[i];
                c.used_minutes += i64::from(used) / 60;
                c.earned_minutes += i64::from(earned) / 60;
                c.devices.push(entry);
                // A limit set on any of their profiles wins over none at all.
                if c.limit_minutes.is_none() {
                    c.limit_minutes = limit_from_policy(policy.as_ref());
                }
            }
            None => {
                index.insert(os_username.clone(), by_key.len());
                by_key.push(Child {
                    pending_requests: asks_by_user.get(&os_username).copied().unwrap_or(0),
                    key: os_username,
                    name,
                    used_minutes: i64::from(used) / 60,
                    earned_minutes: i64::from(earned) / 60,
                    limit_minutes: limit_from_policy(policy.as_ref()),
                    profile_id,
                    profile_name: pname,
                    devices: vec![entry],
                });
            }
        }
    }

    by_key.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let children: Vec<Value> = by_key
        .into_iter()
        .map(|c| {
            json!({
                "key": c.key,
                "name": c.name,
                "used_minutes": c.used_minutes,
                "earned_minutes": c.earned_minutes,
                "limit_minutes": c.limit_minutes,
                "profile_id": c.profile_id,
                "profile_name": c.profile_name,
                "devices": c.devices,
                "pending_requests": c.pending_requests,
            })
        })
        .collect();

    Ok(Json(json!({
        "children": children,
        "devices": devices_json,
        "profiles": profiles,
        "requests": requests,
        "server_time": Utc::now(),
    })))
}

/// A device inside an allowed-offline window is not trouble — a parent said it
/// may be away. Without this the home page flags a laptop that is simply
/// switched off for the weekend.
#[derive(serde::Deserialize)]
pub struct OfflineWindowReq {
    /// Minutes from now, or null to end the window immediately.
    pub minutes: Option<i64>,
}

/// `PUT /api/devices/{id}/offline-window`
pub async fn set_offline_window(
    State(st): State<AppState>,
    admin: AuthAdmin,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Json(req): Json<OfflineWindowReq>,
) -> AppResult<Json<Value>> {
    let until: Option<DateTime<Utc>> = match req.minutes {
        None => None,
        Some(m) if m <= 0 => None,
        // A window is a reassurance, not a permanent exemption: cap it at a
        // month so a device cannot be silently excused from monitoring forever.
        Some(m) => Some(Utc::now() + chrono::Duration::minutes(m.min(60 * 24 * 31))),
    };

    let row: Option<DeviceRow> = sqlx::query_as(&format!(
        "UPDATE devices SET offline_allowed_until = $1
          WHERE id = $2 AND tenant_id = $3
      RETURNING {DEVICE_COLS}"
    ))
    .bind(until)
    .bind(id)
    .bind(admin.tenant_id)
    .fetch_optional(&st.db)
    .await?;

    let row = row.ok_or_else(|| crate::error::AppError::NotFound("device not found".into()))?;
    let mut d = device_to_json(&row);
    d["online"] = json!(st.hub.is_online(id).await);
    Ok(Json(json!({ "device": d })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_or_zero_limit_is_no_limit_not_zero_left() {
        // The distinction the console renders: "no limit set" vs "time is up".
        let off = json!({ "screen_time": { "enabled": false, "daily_limit_minutes": 60 } });
        assert_eq!(limit_from_policy(Some(&off)), None);

        let zero = json!({ "screen_time": { "enabled": true, "daily_limit_minutes": 0 } });
        assert_eq!(limit_from_policy(Some(&zero)), None);

        let real = json!({ "screen_time": { "enabled": true, "daily_limit_minutes": 60 } });
        assert_eq!(limit_from_policy(Some(&real)), Some(60));

        // A user with no profile at all must not panic or invent a limit.
        assert_eq!(limit_from_policy(None), None);
        assert_eq!(limit_from_policy(Some(&json!({}))), None);
    }
}
