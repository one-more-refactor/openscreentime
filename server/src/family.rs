//! `GET /api/family` — the entire home screen in one request.
//!
//! A "child" here is a **person** (a member account), not an OS row: the same
//! person on two machines is one child whose day is the sum of both. Since 0.4
//! every OS login is linked to an account (`members::link_os_user`), so the
//! grouping is by `device_users.account_id` and the key is the account id.
//! Members with no device yet still appear — a child you just added is part of
//! the family before their laptop is.
//!
//! Fixed number of queries no matter how many devices a family has.

use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use crate::devices::{device_to_json, lock_pending, DeviceRow, DEVICE_COLS};
use crate::error::AppResult;
use crate::members::{self, AccountRow, ACCOUNT_COLS};
use crate::state::{AppState, AuthAdmin};
use openscreentime_policy::{catalog, Policy};

/// One device_user joined to today's ledger row.
type FamilyUserRow = (
    Uuid,         // du.id
    Uuid,         // du.device_id
    String,       // du.os_username
    Option<Uuid>, // du.account_id
    Option<Uuid>, // du.profile_id
    i32,          // used_seconds today (ledger columns are int4)
    i32,          // earned_seconds today
);

struct Child {
    account: AccountRow,
    used_minutes: i64,
    earned_minutes: i64,
    devices: Vec<Value>,
    pending_requests: usize,
    locked: bool,
}

pub async fn get_family(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    // 1. Devices.
    let device_rows: Vec<DeviceRow> = sqlx::query_as(&format!(
        "SELECT {DEVICE_COLS} FROM devices WHERE tenant_id = $1 ORDER BY created_at DESC"
    ))
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;

    // 2. Members (the children, plus adults who only self-track).
    let member_rows: Vec<AccountRow> = sqlx::query_as(&format!(
        "SELECT {ACCOUNT_COLS} FROM admins WHERE tenant_id = $1 AND role = 'member'
          ORDER BY created_at"
    ))
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;

    // 3. Every device_user in the tenant with today's usage.
    let user_rows: Vec<FamilyUserRow> = sqlx::query_as(
        "SELECT du.id, du.device_id, du.os_username, du.account_id, du.profile_id,
                COALESCE(l.used_seconds, 0), COALESCE(l.earned_seconds, 0)
           FROM device_users du
           JOIN devices d ON d.id = du.device_id
           LEFT JOIN screen_time_ledger l
                  ON l.device_user_id = du.id AND l.day = CURRENT_DATE
          WHERE d.tenant_id = $1
          ORDER BY du.os_username",
    )
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;

    // 4. Pending commands for the whole tenant, grouped per device.
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

    // 5. Profiles (the rules editor needs the full list), as JSON and as a
    //    policy map for limits/blocks.
    let profiles = crate::profiles::list_for_tenant(&st.db, admin.tenant_id).await?;
    let mut policies: HashMap<Uuid, Policy> = HashMap::new();
    if let Some(list) = profiles.as_array() {
        for p in list {
            if let (Some(id), Some(pol)) = (
                p.get("id")
                    .and_then(Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok()),
                p.get("policy").cloned(),
            ) {
                if let Ok(parsed) = serde_json::from_value::<Policy>(pol) {
                    policies.insert(id, parsed);
                }
            }
        }
    }

    // 6. Earn requests still waiting on a parent.
    let requests = crate::earn::list_for_tenant(&st.db, admin.tenant_id, Some("pending".into()))
        .await?
        .get("requests")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let mut asks_by_du: HashMap<Uuid, usize> = HashMap::new();
    if let Some(list) = requests.as_array() {
        for r in list {
            if let Some(du) = r
                .get("device_user_id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                *asks_by_du.entry(du).or_default() += 1;
            }
        }
    }

    // Devices, with liveness, pending chips and spare keys folded in.
    let recovery = crate::devices::recovery_unused_by_device(&st.db, admin.tenant_id).await?;
    let mut devices_json = Vec::with_capacity(device_rows.len());
    let mut device_meta: HashMap<Uuid, (String, String, bool, bool)> = HashMap::new();
    for r in &device_rows {
        let mut d = device_to_json(r);
        let p = pending.get(&r.0).cloned().unwrap_or_default();
        d["online"] = json!(d["status"] == "online");
        d["recovery_codes_unused"] = json!(recovery.get(&r.0).copied().unwrap_or(0));
        let lp = lock_pending(&p, r.14.as_ref());
        d["lock_pending"] = json!(lp);
        d["pending_commands"] = json!(p);
        device_meta.insert(r.0, (r.2.clone(), r.6.clone(), r.13, lp));
        devices_json.push(d);
    }

    // People. Members first (even with no device); OS logins linked to parents
    // are not "children" and stay out of this list.
    let mut children: Vec<Child> = member_rows
        .into_iter()
        .map(|account| Child {
            account,
            used_minutes: 0,
            earned_minutes: 0,
            devices: Vec::new(),
            pending_requests: 0,
            locked: false,
        })
        .collect();
    let index: HashMap<Uuid, usize> = children
        .iter()
        .enumerate()
        .map(|(i, c)| (c.account.0, i))
        .collect();

    for (du_id, device_id, os_username, account_id, _profile_id, used, earned) in user_rows {
        let Some(i) = account_id.and_then(|a| index.get(&a).copied()) else {
            continue;
        };
        let (dev_name, dev_status, dev_locked, dev_lock_pending) = device_meta
            .get(&device_id)
            .cloned()
            .unwrap_or_else(|| ("unknown".into(), "offline".into(), false, false));
        let c = &mut children[i];
        c.used_minutes += i64::from(used) / 60;
        c.earned_minutes += i64::from(earned) / 60;
        c.pending_requests += asks_by_du.get(&du_id).copied().unwrap_or(0);
        c.locked |= dev_locked;
        c.devices.push(json!({
            "device_user_id": du_id,
            "id": device_id,
            "name": dev_name,
            "status": dev_status,
            "locked": dev_locked,
            "lock_pending": dev_lock_pending,
            "os_username": os_username,
        }));
    }

    children.sort_by_key(|c| c.account.2.to_lowercase());
    // Blocked members (Danger-Zone action) — surfaced so the console shows the
    // state. Kept out of the shared ACCOUNT_COLS tuple to avoid churning arity.
    let blocked_ids: std::collections::HashSet<Uuid> = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM admins WHERE tenant_id = $1 AND blocked_at IS NOT NULL",
    )
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?
    .into_iter()
    .collect();

    let children: Vec<Value> = children
        .into_iter()
        .map(|c| {
            let bracket = members::bracket_of(&c.account);
            let policy = c.account.9.and_then(|p| policies.get(&p));
            let limit = policy.and_then(members::limit_minutes);
            let profile_name = c
                .account
                .9
                .and_then(|pid| {
                    profiles.as_array().and_then(|l| {
                        l.iter().find(|p| {
                            p.get("id").and_then(Value::as_str) == Some(pid.to_string().as_str())
                        })
                    })
                })
                .and_then(|p| p.get("name").cloned())
                .unwrap_or(Value::Null);
            let blocked_apps = policy
                .map(|p| catalog::expand(&p.blocks).apps)
                .unwrap_or_default();
            let mut v = members::account_json(&c.account);
            v["key"] = json!(c.account.0);
            v["name"] = json!(c.account.2);
            v["used_minutes"] = json!(c.used_minutes);
            v["earned_minutes"] = json!(c.earned_minutes);
            v["limit_minutes"] = json!(limit);
            v["profile_name"] = profile_name;
            v["devices"] = json!(c.devices);
            v["pending_requests"] = json!(c.pending_requests);
            v["locked"] = json!(c.locked);
            v["blocks"] = json!(policy.map(|p| p.blocks.clone()).unwrap_or_default());
            v["blocked_apps"] = json!(blocked_apps);
            v["can_ask"] = json!(bracket.can_request_time());
            v["managed"] = json!(bracket.is_managed());
            v["blocked"] = json!(blocked_ids.contains(&c.account.0));
            v
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
    d["online"] = json!(d["status"] == "online");
    d["recovery_codes_unused"] = json!(crate::devices::recovery_unused_one(&st.db, id).await?);
    Ok(Json(json!({ "device": d })))
}
