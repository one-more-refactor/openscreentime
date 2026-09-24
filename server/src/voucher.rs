//! One-time sign-in tokens that arrive in a URL fragment (never sent to a
//! server, so never in an access log):
//!
//! * **device vouchers** — `ost login` on an enrolled computer: the agent
//!   (holding the device token) mints one for the person behind the OS login
//!   that asked, and opens `https://host/#v=…`;
//! * **recovery links** — `openscreentime-server recover <name>` (root inside
//!   the container) mints one for an existing account that lost its passkeys:
//!   `https://host/#signin=…`.

use axum::{extract::State, Json};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{create_session, gen_token, hash_token, insert_session, session_cookie};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Voucher sessions are shorter than a passkey's: a computer vouches for
/// whoever is at it, which is the proof, but a week is plenty.
const VOUCHER_SESSION_DAYS: i32 = 7;

#[derive(Deserialize)]
pub struct MintVoucherReq {
    /// The OS login on the device that wants to open the console. The session
    /// is issued for the person that login is linked to — a child's laptop
    /// opens the child's page, never the parent's.
    #[serde(default)]
    pub os_username: String,
}

/// `POST /agent/voucher` — the enrolled client mints a one-time voucher for a
/// local surface on its own machine to exchange.
///
/// The voucher is bound to the **account** behind `os_username`
/// (`device_users.account_id`). A parent is only vouched for from a computer
/// declared as theirs, and there only for **the owner's login**
/// (`devices.owner_os_username`) — root on a shared kid laptop must not mint a
/// parent session, and neither may a child's login that an older server
/// linked to the parent on the parent's own computer.
pub async fn mint(
    State(st): State<AppState>,
    agent: crate::state::AgentAuth,
    body: Option<Json<MintVoucherReq>>,
) -> AppResult<Json<Value>> {
    let os_username = body
        .map(|Json(b)| b.os_username.trim().to_string())
        .unwrap_or_default();
    if os_username.is_empty() {
        return Err(AppError::BadRequest("os_username required".into()));
    }
    let account_id: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT account_id FROM device_users WHERE device_id = $1 AND os_username = $2",
    )
    .bind(agent.device_id)
    .bind(&os_username)
    .fetch_optional(&st.db)
    .await?;
    let Some(account_id) = account_id.flatten() else {
        return Err(AppError::NoAccount(format!(
            "{os_username} on this computer isn't linked to anyone on the household yet"
        )));
    };
    let (role, own_device): (String, bool) = sqlx::query_as(
        "SELECT a.role,
                EXISTS (SELECT 1 FROM devices d
                         WHERE d.id = $2 AND d.owner_account_id = a.id
                           AND lower(d.owner_os_username) = lower($3))
           FROM admins a WHERE a.id = $1",
    )
    .bind(account_id)
    .bind(agent.device_id)
    .bind(&os_username)
    .fetch_one(&st.db)
    .await?;
    if role != "member" && !own_device {
        return Err(AppError::NoAccount(format!(
            "{os_username} is a parent — only their own computer can sign them in this way"
        )));
    }

    let voucher = gen_token();
    sqlx::query(
        "INSERT INTO device_vouchers (device_id, tenant_id, account_id, voucher_hash, expires_at)
         VALUES ($1, $2, $3, $4, now() + interval '2 minutes')",
    )
    .bind(agent.device_id)
    .bind(agent.tenant_id)
    .bind(account_id)
    .bind(hash_token(&voucher))
    .execute(&st.db)
    .await?;
    let _ = sqlx::query("DELETE FROM device_vouchers WHERE expires_at < now() - interval '1 hour'")
        .execute(&st.db)
        .await;

    Ok(Json(
        json!({ "voucher": voucher, "expires_in_secs": 120, "account_id": account_id }),
    ))
}

#[derive(Deserialize)]
pub struct TokenReq {
    #[serde(default)]
    pub voucher: String,
    #[serde(default)]
    pub token: String,
}

/// `POST /api/auth/voucher` — voucher in, session out, server-verified.
pub async fn redeem(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<TokenReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let row: Option<(Uuid, Uuid, Option<Uuid>)> = sqlx::query_as(
        "UPDATE device_vouchers SET consumed_at = now()
          WHERE voucher_hash = $1 AND consumed_at IS NULL AND expires_at > now()
        RETURNING device_id, tenant_id, account_id",
    )
    .bind(hash_token(&req.voucher))
    .fetch_optional(&st.db)
    .await?;
    let (device_id, tenant_id, account_id) =
        row.ok_or_else(|| AppError::Unauthorized("voucher not valid".into()))?;

    // The device must still be enrolled in that household, and the person
    // must still exist in it.
    let live: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM devices WHERE id = $1 AND tenant_id = $2")
            .bind(device_id)
            .bind(tenant_id)
            .fetch_optional(&st.db)
            .await?;
    if live.is_none() {
        return Err(AppError::Unauthorized("device is not enrolled".into()));
    }
    let account_id =
        account_id.ok_or_else(|| AppError::Unauthorized("voucher not valid".into()))?;
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM admins WHERE id = $1 AND tenant_id = $2")
            .bind(account_id)
            .bind(tenant_id)
            .fetch_optional(&st.db)
            .await?;
    let role = role.ok_or_else(|| AppError::Unauthorized("no account on this household".into()))?;

    let token = insert_session(&st.db, account_id, tenant_id, VOUCHER_SESSION_DAYS, true).await?;
    // A new session is security-relevant, like every other sign-in: a
    // parent's is phoned out (critical) — it opens with the confirm window
    // open, so a computer that shouldn't vouch for them is never silent. A
    // child's `ost login` on their own laptop is logged, not phoned.
    let display: String = sqlx::query_scalar("SELECT display_name FROM admins WHERE id = $1")
        .bind(account_id)
        .fetch_one(&st.db)
        .await?;
    let _ = crate::events::insert(
        &st.db,
        tenant_id,
        Some(device_id),
        None,
        "account_login",
        if role == "member" { "info" } else { "critical" },
        json!({
            "message": format!("New web sign-in as {display} from this computer (ost login)."),
            "account_id": account_id,
            "via": "device_voucher",
        }),
    )
    .await;
    Ok((
        jar.add(session_cookie(token, st.cookie_secure)),
        Json(json!({
            "ok": true, "via": "device_voucher", "account_id": account_id, "role": role
        })),
    ))
}

// ── recovery links ──────────────────────────────────────────────────────────

/// How long a recovery link works.
pub const LINK_MINUTES: i32 = 30;

/// Mint a one-time sign-in link token for an existing account.
pub async fn mint_link(db: &sqlx::PgPool, tenant_id: Uuid, account_id: Uuid) -> AppResult<String> {
    let token = gen_token();
    sqlx::query(
        "INSERT INTO signin_links (tenant_id, account_id, token_hash, expires_at)
         VALUES ($1, $2, $3, now() + make_interval(mins => $4))",
    )
    .bind(tenant_id)
    .bind(account_id)
    .bind(hash_token(&token))
    .bind(LINK_MINUTES)
    .execute(db)
    .await?;
    Ok(token)
}

/// `POST /api/auth/link` — a recovery link's token in, a session out. The
/// session is born with its confirm window open, so the first thing the
/// person can do is add a new passkey.
pub async fn redeem_link(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<TokenReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE signin_links SET consumed_at = now()
          WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > now()
        RETURNING tenant_id, account_id",
    )
    .bind(hash_token(req.token.trim()))
    .fetch_optional(&st.db)
    .await?;
    let (tenant_id, account_id) = row.ok_or_else(|| {
        AppError::Unauthorized("that sign-in link was already used or has run out".into())
    })?;
    let token = create_session(&st.db, account_id, tenant_id).await?;
    let _ = crate::events::insert(
        &st.db,
        tenant_id,
        None,
        None,
        "account_login",
        "critical",
        json!({
            "message": "Someone signed in with a recovery link from the server. \
                        If that wasn't you, the server itself is in the wrong hands.",
            "account_id": account_id,
            "via": "recovery_link",
        }),
    )
    .await;
    Ok((
        jar.add(session_cookie(token, st.cookie_secure)),
        Json(json!({ "ok": true, "via": "recovery_link" })),
    ))
}
