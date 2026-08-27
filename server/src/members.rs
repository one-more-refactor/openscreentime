//! Accounts — everyone has one (docs/CONTRACT-0.4.md §2, §6).
//!
//! The `admins` table is the account table: owners and parents are the hub,
//! **members** are the people the hub manages (children) or adults who only
//! track themselves. A member usually has no email and no passkey; they reach
//! the console through a device voucher on a machine they use and land on
//! their own page.
//!
//! This module owns: the member CRUD (`/api/members`), the extended `/api/me`,
//! the member's own `/api/me/today` + `/api/me/ask`, the OS-user → account
//! linking used by enrollment, the member guard layer (a member session can
//! reach a short allow-list and nothing else — fails closed), and
//! `/api/catalog`.

use axum::{
    extract::{Path, Request, State},
    middleware::Next,
    response::Response,
    Json,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::enqueue_command;
use crate::auth::hash_token;
use crate::error::{AppError, AppResult};
use crate::events;
use crate::presets;
use crate::state::{AppState, AuthAdmin, SESSION_COOKIE};
use openscreentime_policy::{catalog, AgeBracket, Policy, Theme};

// ── the account row ─────────────────────────────────────────────────────────

pub const ACCOUNT_COLS: &str = "id, tenant_id, display_name, email, role, age_bracket, birthdate, \
    theme, self_managed, profile_id, created_at, avatar";

pub type AccountRow = (
    Uuid,              // id
    Uuid,              // tenant_id
    String,            // display_name
    Option<String>,    // email
    String,            // role
    String,            // age_bracket
    Option<NaiveDate>, // birthdate
    Option<String>,    // theme (NULL = auto)
    bool,              // self_managed
    Option<Uuid>,      // profile_id
    DateTime<Utc>,     // created_at
    Option<String>,    // avatar (emoji; NULL = monogram) — appended last so
                       //   the positional accesses above it never renumber
);

pub fn bracket_of(r: &AccountRow) -> AgeBracket {
    AgeBracket::parse(&r.5).unwrap_or(AgeBracket::Adult)
}

/// The theme a person's page actually renders: their explicit pick, else the
/// bracket default.
pub fn effective_theme(bracket: AgeBracket, theme: Option<&str>) -> Theme {
    theme
        .and_then(Theme::parse)
        .unwrap_or_else(|| bracket.default_theme())
}

pub fn account_json(r: &AccountRow) -> Value {
    let bracket = bracket_of(r);
    json!({
        "id": r.0,
        "household_id": r.1,
        "tenant_id": r.1,
        "display_name": r.2,
        "email": r.3,
        "role": r.4,
        "age_bracket": r.5,
        "birthdate": r.6,
        "theme": r.7,
        "effective_theme": effective_theme(bracket, r.7.as_deref()).id(),
        "self_managed": r.8,
        "profile_id": r.9,
        "created_at": r.10,
        "avatar": r.11,
    })
}

pub async fn get_account(db: &sqlx::PgPool, id: Uuid, tenant_id: Uuid) -> AppResult<AccountRow> {
    let row: Option<AccountRow> = sqlx::query_as(&format!(
        "SELECT {ACCOUNT_COLS} FROM admins WHERE id = $1 AND tenant_id = $2"
    ))
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(db)
    .await?;
    row.ok_or_else(|| AppError::NotFound("account not found".into()))
}

fn require_hub(admin: &AuthAdmin) -> AppResult<()> {
    if admin.is_hub() {
        Ok(())
    } else {
        Err(AppError::ForbiddenForMember(
            "only a parent can do that".into(),
        ))
    }
}

/// The daily limit that actually applies, or None for "no limit". A disabled
/// or zero limit is *no limit*, never "0 left of 0".
pub fn limit_minutes(policy: &Policy) -> Option<i64> {
    if !policy.screen_time.enabled {
        return None;
    }
    match policy.screen_time.daily_limit_minutes {
        0 => None,
        m => Some(i64::from(m)),
    }
}

// ── rules for a person ──────────────────────────────────────────────────────

/// A fresh, editable copy of the bracket preset, owned by one person.
pub async fn create_profile_for(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    bracket: AgeBracket,
    person_name: &str,
) -> AppResult<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO profiles (tenant_id, name, kind, is_preset, policy)
         VALUES ($1, $2, $3, false, $4) RETURNING id",
    )
    .bind(tenant_id)
    .bind(format!("{person_name}'s rules"))
    .bind(bracket.id())
    .bind(presets::policy_for(bracket))
    .fetch_one(db)
    .await?;
    Ok(id)
}

/// The account's rules, creating them from the bracket preset if the account
/// predates 0.4 and has none yet.
async fn ensure_profile(db: &sqlx::PgPool, acct: &AccountRow) -> AppResult<Uuid> {
    if let Some(p) = acct.9 {
        return Ok(p);
    }
    let bracket = bracket_of(acct);
    let pid = create_profile_for(db, acct.1, bracket, &acct.2).await?;
    sqlx::query("UPDATE admins SET profile_id = $2 WHERE id = $1 AND profile_id IS NULL")
        .bind(acct.0)
        .bind(pid)
        .execute(db)
        .await?;
    Ok(pid)
}

/// Point every OS login of an account at the account's rules and tell their
/// devices to re-pull.
async fn sync_device_users(st: &AppState, account_id: Uuid, profile_id: Uuid) -> AppResult<()> {
    let devices: Vec<(Uuid,)> = sqlx::query_as(
        "UPDATE device_users SET profile_id = $2 WHERE account_id = $1 RETURNING device_id",
    )
    .bind(account_id)
    .bind(profile_id)
    .fetch_all(&st.db)
    .await?;
    let mut seen = std::collections::HashSet::new();
    for (d,) in devices {
        if seen.insert(d) {
            enqueue_command(st, d, "apply_policy", json!({})).await?;
        }
    }
    Ok(())
}

// ── OS user → person linking (enrollment, heartbeat, startup backfill) ───────

/// Link one OS login on a device to a person, creating the person if nobody
/// matches. Order: an existing link wins; else an account in the tenant whose
/// display name equals the OS display name or the username
/// (case-insensitive); else the device's `owner_account_id` (the "this is
/// Mia's laptop" enroll intent); else a brand-new member (bracket `kid`).
/// Always leaves `device_users.profile_id` equal to the person's rules.
pub async fn link_os_user(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
    os_username: &str,
    os_display_name: Option<&str>,
) -> AppResult<Uuid> {
    let os_username = os_username.trim();
    if os_username.is_empty() {
        return Err(AppError::BadRequest("empty os username".into()));
    }
    let display = os_display_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(os_username);

    // 1. Already linked?
    let existing: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT account_id FROM device_users WHERE device_id = $1 AND os_username = $2",
    )
    .bind(device_id)
    .bind(os_username)
    .fetch_optional(db)
    .await?;
    let linked = existing.flatten();

    let account: AccountRow = match linked {
        Some(id) => get_account(db, id, tenant_id).await?,
        None => {
            // 2. Name match.
            let by_name: Option<AccountRow> = sqlx::query_as(&format!(
                "SELECT {ACCOUNT_COLS} FROM admins
                  WHERE tenant_id = $1
                    AND (lower(display_name) = lower($2) OR lower(display_name) = lower($3))
                  ORDER BY (role = 'member') DESC, created_at LIMIT 1"
            ))
            .bind(tenant_id)
            .bind(display)
            .bind(os_username)
            .fetch_optional(db)
            .await?;
            match by_name {
                Some(a) => a,
                None => {
                    // 3. The device's declared owner.
                    let owner: Option<Option<Uuid>> =
                        sqlx::query_scalar("SELECT owner_account_id FROM devices WHERE id = $1")
                            .bind(device_id)
                            .fetch_optional(db)
                            .await?;
                    match owner.flatten() {
                        Some(id) => get_account(db, id, tenant_id).await?,
                        // 4. A new member.
                        None => {
                            let bracket = AgeBracket::Kid;
                            let pid = create_profile_for(db, tenant_id, bracket, display).await?;
                            let id: Uuid = sqlx::query_scalar(
                                "INSERT INTO admins (tenant_id, display_name, role, age_bracket, profile_id)
                                 VALUES ($1, $2, 'member', $3, $4) RETURNING id",
                            )
                            .bind(tenant_id)
                            .bind(display)
                            .bind(bracket.id())
                            .bind(pid)
                            .fetch_one(db)
                            .await?;
                            let _ = events::insert(
                                db,
                                tenant_id,
                                Some(device_id),
                                None,
                                "member",
                                "info",
                                json!({ "action": "auto_created", "account_id": id,
                                        "display_name": display, "os_username": os_username }),
                            )
                            .await;
                            get_account(db, id, tenant_id).await?
                        }
                    }
                }
            }
        }
    };

    let profile_id = ensure_profile(db, &account).await?;
    sqlx::query(
        "INSERT INTO device_users (device_id, os_username, display_name, profile_id, account_id)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (device_id, os_username)
         DO UPDATE SET account_id = $5,
                       profile_id = $4,
                       display_name = COALESCE(device_users.display_name, EXCLUDED.display_name)",
    )
    .bind(device_id)
    .bind(os_username)
    .bind(os_display_name)
    .bind(profile_id)
    .bind(account.0)
    .execute(db)
    .await?;
    Ok(account.0)
}

/// Startup backfill: every OS login that predates 0.4 gets a person.
pub async fn backfill_links(db: &sqlx::PgPool) -> AppResult<()> {
    let rows: Vec<(Uuid, Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT d.tenant_id, du.device_id, du.os_username, du.display_name
           FROM device_users du JOIN devices d ON d.id = du.device_id
          WHERE du.account_id IS NULL",
    )
    .fetch_all(db)
    .await?;
    for (tenant_id, device_id, user, display) in rows {
        if let Err(e) = link_os_user(db, tenant_id, device_id, &user, display.as_deref()).await {
            tracing::warn!(error = %e, %device_id, %user, "could not link OS user to an account");
        }
    }
    Ok(())
}

// ── member CRUD (hub only) ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateMemberReq {
    pub display_name: String,
    #[serde(default)]
    pub birthdate: Option<NaiveDate>,
    #[serde(default)]
    pub age_bracket: Option<String>,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

fn parse_bracket(s: &str) -> AppResult<AgeBracket> {
    AgeBracket::parse(s).ok_or_else(|| AppError::BadRequest(format!("unknown age bracket {s:?}")))
}

fn parse_theme(s: &str) -> AppResult<Option<&'static str>> {
    match s {
        "" | "auto" => Ok(None),
        other => Theme::parse(other)
            .map(|t| Some(t.id()))
            .ok_or_else(|| AppError::BadRequest(format!("unknown theme {other:?}"))),
    }
}

/// `POST /api/members` → a member with rules copied from their bracket preset.
pub async fn create_member(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Json(req): Json<CreateMemberReq>,
) -> AppResult<Json<Value>> {
    require_hub(&admin)?;
    let name = req.display_name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("display_name required".into()));
    }
    let bracket = match (&req.birthdate, &req.age_bracket) {
        (_, Some(b)) if !b.is_empty() => parse_bracket(b)?,
        (Some(bd), _) => AgeBracket::from_birthdate(*bd, Utc::now().date_naive()),
        _ => AgeBracket::Kid,
    };
    let theme = match &req.theme {
        Some(t) => parse_theme(t)?,
        None => None,
    };
    let email = req
        .email
        .map(|e| e.trim().to_lowercase())
        .filter(|e| !e.is_empty());

    let pid = create_profile_for(&st.db, admin.tenant_id, bracket, &name).await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO admins (tenant_id, display_name, email, role, age_bracket, birthdate, theme,
                             self_managed, profile_id)
         VALUES ($1, $2, $3, 'member', $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(admin.tenant_id)
    .bind(&name)
    .bind(&email)
    .bind(bracket.id())
    .bind(req.birthdate)
    .bind(theme)
    .bind(!bracket.is_managed())
    .bind(pid)
    .fetch_one(&st.db)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            AppError::Conflict("an account with this email already exists".into())
        }
        _ => AppError::from(e),
    })?;

    events::insert(
        &st.db,
        admin.tenant_id,
        None,
        None,
        "member",
        "info",
        json!({ "action": "created", "account_id": id, "display_name": name,
                "age_bracket": bracket.id(), "by": admin.admin_id }),
    )
    .await?;

    let row = get_account(&st.db, id, admin.tenant_id).await?;
    Ok(Json(json!({ "member": account_json(&row) })))
}

/// `GET /api/members` — every account in the household (hub only).
pub async fn list_members(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    require_hub(&admin)?;
    let rows: Vec<AccountRow> = sqlx::query_as(&format!(
        "SELECT {ACCOUNT_COLS} FROM admins WHERE tenant_id = $1
          ORDER BY (role = 'member'), created_at"
    ))
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(json!({
        "members": rows.iter().map(account_json).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
pub struct PatchMemberReq {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub birthdate: Option<Option<NaiveDate>>,
    #[serde(default)]
    pub age_bracket: Option<String>,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub profile_id: Option<Uuid>,
    /// An emoji face; empty string clears back to the monogram.
    #[serde(default)]
    pub avatar: Option<String>,
}

/// `PATCH /api/members/{id}`. Changing the bracket does not rewrite the
/// person's rules — those are the parent's to edit; it changes what the
/// person may do (ask for time, …) and their default look.
pub async fn patch_member(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchMemberReq>,
) -> AppResult<Json<Value>> {
    require_hub(&admin)?;
    let before = get_account(&st.db, id, admin.tenant_id).await?;

    if let Some(name) = &req.display_name {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest("display_name cannot be empty".into()));
        }
        sqlx::query("UPDATE admins SET display_name = $2 WHERE id = $1")
            .bind(id)
            .bind(name)
            .execute(&st.db)
            .await?;
    }
    // Birthdate first: an explicit bracket below still wins.
    if let Some(bd) = req.birthdate {
        let bracket = bd.map(|d| AgeBracket::from_birthdate(d, Utc::now().date_naive()));
        sqlx::query(
            "UPDATE admins SET birthdate = $2,
                    age_bracket = COALESCE($3, age_bracket),
                    self_managed = COALESCE($4, self_managed)
              WHERE id = $1",
        )
        .bind(id)
        .bind(bd)
        .bind(bracket.map(|b| b.id()))
        .bind(bracket.map(|b| !b.is_managed()))
        .execute(&st.db)
        .await?;
    }
    if let Some(b) = &req.age_bracket {
        let bracket = parse_bracket(b)?;
        if before.4 != "member" && bracket.is_managed() {
            return Err(AppError::BadRequest(
                "a parent account cannot be put in a managed bracket".into(),
            ));
        }
        sqlx::query("UPDATE admins SET age_bracket = $2, self_managed = $3 WHERE id = $1")
            .bind(id)
            .bind(bracket.id())
            .bind(!bracket.is_managed())
            .execute(&st.db)
            .await?;
    }
    if let Some(t) = &req.theme {
        let theme = parse_theme(t)?;
        sqlx::query("UPDATE admins SET theme = $2 WHERE id = $1")
            .bind(id)
            .bind(theme)
            .execute(&st.db)
            .await?;
    }
    if let Some(av) = &req.avatar {
        let av = av.trim();
        // A face is one emoji, not an essay; over-long input is a bug or abuse.
        if av.chars().count() > 4 {
            return Err(AppError::BadRequest("avatar must be a single emoji".into()));
        }
        sqlx::query("UPDATE admins SET avatar = $2 WHERE id = $1")
            .bind(id)
            .bind(if av.is_empty() { None } else { Some(av) })
            .execute(&st.db)
            .await?;
    }
    if let Some(pid) = req.profile_id {
        let owned: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM profiles WHERE id = $1 AND tenant_id = $2")
                .bind(pid)
                .bind(admin.tenant_id)
                .fetch_optional(&st.db)
                .await?;
        owned.ok_or_else(|| AppError::NotFound("profile not found".into()))?;
        sqlx::query("UPDATE admins SET profile_id = $2 WHERE id = $1")
            .bind(id)
            .bind(pid)
            .execute(&st.db)
            .await?;
        sync_device_users(&st, id, pid).await?;
    }

    let row = get_account(&st.db, id, admin.tenant_id).await?;
    Ok(Json(json!({ "member": account_json(&row) })))
}

/// `DELETE /api/members/{id}` — members only; the person's OS logins stay on
/// their devices (unlinked) and their rules are removed if nothing else uses
/// them.
pub async fn delete_member(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    require_hub(&admin)?;
    let row = get_account(&st.db, id, admin.tenant_id).await?;
    if row.4 != "member" {
        return Err(AppError::BadRequest(
            "only members can be removed here; parents leave through Settings".into(),
        ));
    }
    sqlx::query("DELETE FROM admins WHERE id = $1 AND tenant_id = $2")
        .bind(id)
        .bind(admin.tenant_id)
        .execute(&st.db)
        .await?;
    if let Some(pid) = row.9 {
        let _ = sqlx::query(
            "DELETE FROM profiles WHERE id = $1 AND NOT is_preset
               AND NOT EXISTS (SELECT 1 FROM device_users WHERE profile_id = $1)
               AND NOT EXISTS (SELECT 1 FROM admins WHERE profile_id = $1)",
        )
        .bind(pid)
        .execute(&st.db)
        .await;
    }
    events::insert(
        &st.db,
        admin.tenant_id,
        None,
        None,
        "member",
        "info",
        json!({ "action": "deleted", "account_id": id, "display_name": row.2, "by": admin.admin_id }),
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

// ── /api/me ─────────────────────────────────────────────────────────────────

/// `GET /api/me` → account + household, plus the deprecated `admin`/`tenant`
/// aliases the console still types.
pub async fn me(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let row = get_account(&st.db, admin.admin_id, admin.tenant_id).await?;
    let tenant: (Uuid, String, DateTime<Utc>) =
        sqlx::query_as("SELECT id, name, created_at FROM tenants WHERE id = $1")
            .bind(admin.tenant_id)
            .fetch_one(&st.db)
            .await?;
    let account = account_json(&row);
    Ok(Json(json!({
        "account": account,
        "household": { "id": tenant.0, "name": tenant.1, "created_at": tenant.2 },
        "admin": {
            "id": row.0, "tenant_id": row.1, "email": row.3, "display_name": row.2,
            "role": row.4, "age_bracket": row.5,
        },
        "tenant": { "id": tenant.0, "name": tenant.1 },
    })))
}

/// The rules that apply to a person: their own profile, else the first of
/// their OS logins' profiles, else an empty policy.
async fn policy_for_account(db: &sqlx::PgPool, acct: &AccountRow) -> AppResult<Policy> {
    let raw: Option<Value> = match acct.9 {
        Some(pid) => {
            sqlx::query_scalar("SELECT policy FROM profiles WHERE id = $1")
                .bind(pid)
                .fetch_optional(db)
                .await?
        }
        None => {
            sqlx::query_scalar(
                "SELECT p.policy FROM device_users du JOIN profiles p ON p.id = du.profile_id
                  WHERE du.account_id = $1 ORDER BY du.os_username LIMIT 1",
            )
            .bind(acct.0)
            .fetch_optional(db)
            .await?
        }
    };
    Ok(raw
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

type TodayRow = (Uuid, Uuid, String, String, bool, i32, i32);

/// `GET /api/me/today` — the person's own day, across every device they use.
pub async fn today(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let acct = get_account(&st.db, admin.admin_id, admin.tenant_id).await?;
    let bracket = bracket_of(&acct);
    let policy = policy_for_account(&st.db, &acct).await?;

    let rows: Vec<TodayRow> = sqlx::query_as(
        "SELECT du.id, d.id, d.name, d.status, d.locked,
                COALESCE(l.used_seconds, 0), COALESCE(l.earned_seconds, 0)
           FROM device_users du
           JOIN devices d ON d.id = du.device_id
           LEFT JOIN screen_time_ledger l ON l.device_user_id = du.id AND l.day = CURRENT_DATE
          WHERE du.account_id = $1 AND d.tenant_id = $2
          ORDER BY d.name",
    )
    .bind(acct.0)
    .bind(acct.1)
    .fetch_all(&st.db)
    .await?;

    let used: i64 = rows.iter().map(|r| i64::from(r.5)).sum::<i64>() / 60;
    let earned: i64 = rows.iter().map(|r| i64::from(r.6)).sum::<i64>() / 60;
    let limit = limit_minutes(&policy);
    let left = limit.map(|l| (l + earned - used).max(0));
    let locked = rows.iter().any(|r| r.4);
    let du_ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let pending: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM earn_requests WHERE device_user_id = ANY($1) AND status = 'pending' LIMIT 1",
    )
    .bind(&du_ids)
    .fetch_optional(&st.db)
    .await?;

    // Dedupe devices (one person can have two logins on one machine).
    let mut devices = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in &rows {
        if seen.insert(r.1) {
            devices.push(json!({ "id": r.1, "name": r.2, "status": r.3, "locked": r.4 }));
        }
    }

    Ok(Json(json!({
        "used_minutes": used,
        "earned_minutes": earned,
        "limit_minutes": limit,
        "left_minutes": left,
        "locked": locked,
        "devices": devices,
        "blocks": policy.blocks,
        "blocked_apps": catalog::expand(&policy.blocks).apps,
        "bracket": bracket.id(),
        "theme": effective_theme(bracket, acct.7.as_deref()).id(),
        "can_ask": bracket.can_request_time(),
        "pending_request": pending.is_some(),
        "bedtime": policy.screen_time.bedtime,
        "windows": policy.screen_time.schedule,
        "display_name": acct.2,
    })))
}

/// `GET /api/me/history` — the last 14 days summed across the person's
/// devices, plus where today's minutes went, device by device. The /me page
/// draws its week from this: knowing what you actually did is the floor of
/// any motivation.
pub async fn history(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let acct = get_account(&st.db, admin.admin_id, admin.tenant_id).await?;

    let days: Vec<(chrono::NaiveDate, i64, i64)> = sqlx::query_as(
        "SELECT l.day, SUM(l.used_seconds)::bigint, SUM(l.earned_seconds)::bigint
           FROM screen_time_ledger l
           JOIN device_users du ON du.id = l.device_user_id
           JOIN devices d ON d.id = du.device_id
          WHERE du.account_id = $1 AND d.tenant_id = $2
            AND l.day > CURRENT_DATE - 14
          GROUP BY l.day ORDER BY l.day",
    )
    .bind(acct.0)
    .bind(acct.1)
    .fetch_all(&st.db)
    .await?;

    let today_by_device: Vec<(String, i64)> = sqlx::query_as(
        "SELECT d.name, SUM(l.used_seconds)::bigint
           FROM screen_time_ledger l
           JOIN device_users du ON du.id = l.device_user_id
           JOIN devices d ON d.id = du.device_id
          WHERE du.account_id = $1 AND d.tenant_id = $2 AND l.day = CURRENT_DATE
          GROUP BY d.name HAVING SUM(l.used_seconds) > 0
          ORDER BY SUM(l.used_seconds) DESC",
    )
    .bind(acct.0)
    .bind(acct.1)
    .fetch_all(&st.db)
    .await?;

    Ok(Json(json!({
        "days": days
            .into_iter()
            .map(|(day, used, earned)| json!({
                "day": day,
                "used_minutes": used / 60,
                "earned_minutes": earned / 60,
            }))
            .collect::<Vec<_>>(),
        "today_by_device": today_by_device
            .into_iter()
            .map(|(name, used)| json!({ "name": name, "used_minutes": used / 60 }))
            .collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct AskReq {
    pub minutes: i32,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `POST /api/me/ask` — "can I have more time?" Lands as a pending request the
/// hub answers like any earn request. One open ask per day; asking again
/// returns the same request.
pub async fn ask(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Json(req): Json<AskReq>,
) -> AppResult<Json<Value>> {
    let acct = get_account(&st.db, admin.admin_id, admin.tenant_id).await?;
    let bracket = bracket_of(&acct);
    if !bracket.can_request_time() {
        return Err(AppError::BadRequest(
            "this account can't ask for time".into(),
        ));
    }
    if req.minutes <= 0 || req.minutes > 240 {
        return Err(AppError::BadRequest(
            "minutes must be between 1 and 240".into(),
        ));
    }
    // The device to file it against: an online one if there is one.
    let target: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT du.id, d.id FROM device_users du JOIN devices d ON d.id = du.device_id
          WHERE du.account_id = $1 AND d.tenant_id = $2
          ORDER BY (d.status = 'online') DESC, d.last_seen DESC NULLS LAST LIMIT 1",
    )
    .bind(acct.0)
    .bind(acct.1)
    .fetch_optional(&st.db)
    .await?;
    let (device_user_id, device_id) = target.ok_or_else(|| {
        AppError::BadRequest("you don't use any device on this household yet".into())
    })?;

    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM earn_requests
          WHERE device_user_id = $1 AND task_id = 'ask' AND status = 'pending'
            AND created_at::date = CURRENT_DATE",
    )
    .bind(device_user_id)
    .fetch_optional(&st.db)
    .await?;
    let label = req
        .reason
        .map(|r| r.trim().chars().take(120).collect::<String>())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| "Asked for more time".into());

    let id = match existing {
        Some(id) => id,
        None => {
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO earn_requests
                     (tenant_id, device_id, device_user_id, task_id, task_label, minutes)
                 VALUES ($1, $2, $3, 'ask', $4, $5) RETURNING id",
            )
            .bind(acct.1)
            .bind(device_id)
            .bind(device_user_id)
            .bind(&label)
            .bind(req.minutes)
            .fetch_one(&st.db)
            .await?;
            events::insert(
                &st.db,
                acct.1,
                Some(device_id),
                Some(device_user_id),
                "earn_request",
                "info",
                json!({ "action": "requested", "request_id": id, "task_id": "ask",
                        "task_label": label, "minutes": req.minutes, "account_id": acct.0 }),
            )
            .await?;
            id
        }
    };
    let request = crate::earn::request_json(&st.db, id, acct.1).await?;
    Ok(Json(json!({ "request": request })))
}

// ── the member guard ────────────────────────────────────────────────────────

/// What a member session may reach. Everything else under `/api/` is the
/// hub's, and a new route is a hub route until it is added here.
pub fn member_allowed(path: &str) -> bool {
    path == "/api/me"
        || path == "/api/me/today"
        || path == "/api/me/history"
        || path == "/api/me/ask"
        || path == "/api/catalog"
        || path.starts_with("/api/me/2fa")
        || path.starts_with("/api/auth/")
}

/// Layer over `/api`: a member session is confined to [`member_allowed`].
/// Requests without a session pass through (the handler's extractor answers
/// 401); non-`/api` paths are untouched.
pub async fn guard_member(
    State(st): State<AppState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let path = req.uri().path().to_string();
    if !path.starts_with("/api/") || member_allowed(&path) {
        return Ok(next.run(req).await);
    }
    let Some(cookie) = jar.get(SESSION_COOKIE) else {
        return Ok(next.run(req).await);
    };
    let role: Option<String> = sqlx::query_scalar(
        "SELECT a.role FROM admin_sessions s JOIN admins a ON a.id = s.admin_id
          WHERE (s.token_hash = $1
                 OR (s.prev_token_hash = $1 AND s.prev_valid_until > now()))
            AND s.expires_at > now()",
    )
    .bind(hash_token(cookie.value()))
    .fetch_optional(&st.db)
    .await?;
    match role.as_deref() {
        Some("member") => Err(AppError::ForbiddenForMember(
            "this is the parent's side of the house".into(),
        )),
        _ => Ok(next.run(req).await),
    }
}

// ── catalog ─────────────────────────────────────────────────────────────────

/// `GET /api/catalog` — the one-click app & category list (any session).
pub async fn catalog_json(_admin: AuthAdmin) -> Json<Value> {
    Json(catalog::as_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_see_only_their_own_page() {
        assert!(member_allowed("/api/me"));
        assert!(member_allowed("/api/me/today"));
        assert!(member_allowed("/api/me/ask"));
        assert!(member_allowed("/api/catalog"));
        assert!(member_allowed("/api/auth/logout"));
        assert!(member_allowed("/api/auth/stepup/verify"));
        assert!(member_allowed("/api/me/2fa/totp/start"));
        // The hub's side, including routes nobody has written yet.
        assert!(!member_allowed("/api/family"));
        assert!(!member_allowed("/api/devices"));
        assert!(!member_allowed("/api/members"));
        assert!(!member_allowed("/api/me/passkeys"));
        assert!(!member_allowed("/api/something/new"));
    }

    #[test]
    fn effective_theme_is_pick_else_bracket_default() {
        assert_eq!(effective_theme(AgeBracket::Kid, None), Theme::Playful);
        assert_eq!(
            effective_theme(AgeBracket::Kid, Some("plain")),
            Theme::Plain
        );
        assert_eq!(
            effective_theme(AgeBracket::Adult, Some("bogus")),
            Theme::Plain
        );
        assert_eq!(effective_theme(AgeBracket::YoungerTeen, None), Theme::Calm);
    }

    #[test]
    fn a_disabled_or_zero_limit_is_no_limit_not_zero_left() {
        let mut p = Policy::default();
        assert_eq!(limit_minutes(&p), None);
        p.screen_time.enabled = true;
        assert_eq!(limit_minutes(&p), None);
        p.screen_time.daily_limit_minutes = 60;
        assert_eq!(limit_minutes(&p), Some(60));
        p.screen_time.enabled = false;
        assert_eq!(limit_minutes(&p), None);
    }
}
