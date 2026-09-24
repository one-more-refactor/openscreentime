//! Passkeys (WebAuthn via `webauthn-rs`), session cookies, token hashing, and
//! the first-run household.
//!
//! Three passkey ceremonies live here:
//!
//! * **Create your household** — first run only: your name, then a passkey.
//!   While no account exists it needs the one-time setup code that
//!   deploy/setup.sh printed as a link (`#setup=…`), so a fresh
//!   internet-facing install doesn't belong to whoever finds it first.
//! * **Sign in with a passkey** — one tap, no name first. Passkeys are
//!   created as discoverable credentials, so the passkey says whose it is.
//! * **Add a passkey** to the signed-in account (Settings).
//!
//! The other door — your name, then a code shown on your own computer — is
//! `login_code.rs`.

use axum::{
    extract::{Path, State},
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use webauthn_rs::prelude::{
    CreationChallengeResponse, CredentialID, DiscoverableKey, Passkey, PublicKeyCredential,
    RegisterPublicKeyCredential,
};

use crate::error::{AppError, AppResult};
use crate::presets;
use crate::state::{
    AppState, AuthAdmin, AuthChallenge, PasskeyCeremony, RegChallenge, AUTH_COOKIE, CHALLENGE_TTL,
    REG_COOKIE, SESSION_COOKIE,
};

/// Sessions live in Postgres and expire after 30 days (not sliding).
const SESSION_TTL_DAYS: i32 = 30;

/// How long "confirm it's you" stays confirmed — after a confirm, and after a
/// fresh sign-in (which is the same proof, just made at the door).
pub const CONFIRM_MINUTES: i32 = 15;

// ---------------------------------------------------------------------------
// Token helpers
// ---------------------------------------------------------------------------

/// Sha256-hex of a token. Every bearer secret is stored hashed at rest.
pub fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

/// A fresh random 256-bit token, hex-encoded.
pub fn gen_token() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Length-safe constant-time byte comparison (no timing oracle on a setup
/// code, a sign-in code hash, or anything else compared with it).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

pub fn session_cookie(value: String, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, value))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .build()
}

/// Insert a session row and return the raw cookie value (the DB keeps only
/// its sha256). Every caller is a completed sign-in — a passkey, a code from
/// your own computer, SSO, a device voucher or a recovery link — and a fresh
/// sign-in is proof made just now, so the confirm window opens with it: the
/// unlock codes are readable right after signing in without asking again.
pub async fn insert_session(
    db: &sqlx::PgPool,
    admin_id: Uuid,
    tenant_id: Uuid,
    ttl_days: i32,
    via_voucher: bool,
) -> AppResult<String> {
    let token = gen_token();
    sqlx::query(
        "INSERT INTO admin_sessions (token_hash, admin_id, tenant_id, expires_at, via_voucher,
                                     stepup_until)
         VALUES ($1, $2, $3, now() + make_interval(days => $4), $5,
                 now() + make_interval(mins => $6))",
    )
    .bind(hash_token(&token))
    .bind(admin_id)
    .bind(tenant_id)
    .bind(ttl_days)
    .bind(via_voucher)
    .bind(CONFIRM_MINUTES)
    .execute(db)
    .await?;
    // Opportunistic lazy cleanup of expired sessions.
    sqlx::query("DELETE FROM admin_sessions WHERE expires_at < now()")
        .execute(db)
        .await?;
    Ok(token)
}

/// A normal 30-day session (see [`insert_session`]).
pub async fn create_session(
    db: &sqlx::PgPool,
    admin_id: Uuid,
    tenant_id: Uuid,
) -> AppResult<String> {
    insert_session(db, admin_id, tenant_id, SESSION_TTL_DAYS, false).await
}

pub(crate) fn temp_cookie(name: &'static str, value: String, secure: bool) -> Cookie<'static> {
    // Short-lived challenge cookie. It is a session cookie (cleared on browser
    // close) and is explicitly removed once the challenge is consumed. Carries the
    // same Secure policy as the session cookie so it isn't sent over plain HTTP in
    // production.
    Cookie::build((name, value))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .build()
}

// ---------------------------------------------------------------------------
// Tenant bootstrap
// ---------------------------------------------------------------------------

/// Advisory-lock key that serializes tenant/admin bootstraps so two concurrent
/// first-boot registrations can't each create an org (a TOCTOU on the
/// "registration closes after the first admin" invariant).
const ADMIN_BOOTSTRAP_LOCK: i64 = 0x5E17_0001;

/// Creates a tenant, seeds the bracket presets verbatim, and creates the
/// first admin (role `owner`) — with `admin_id` when the caller already
/// promised that id to a passkey as its user handle. Returns
/// (tenant_id, admin_id).
///
/// `require_first` enforces the single-household invariant: when set, the call
/// refuses (under a serializing advisory lock) if any account already exists.
pub async fn create_tenant_with_admin(
    db: &sqlx::PgPool,
    admin_id: Option<Uuid>,
    username: &str,
    display_name: &str,
    require_first: bool,
) -> AppResult<(Uuid, Uuid)> {
    let mut tx = db.begin().await.map_err(AppError::from)?;

    // Serialize bootstraps: a first-boot race would otherwise let two callers
    // that both saw zero admins each insert an org.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_BOOTSTRAP_LOCK)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    if require_first {
        let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM admins")
            .fetch_one(&mut *tx)
            .await
            .map_err(AppError::from)?;
        if admins > 0 {
            return Err(AppError::RegistrationClosed(
                "this server already has a household — sign in instead".into(),
            ));
        }
    }

    let tenant_id: Uuid = sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
        .bind(format!("{display_name}'s household"))
        .fetch_one(&mut *tx)
        .await
        .map_err(AppError::from)?;

    for p in presets::all_presets() {
        sqlx::query(
            "INSERT INTO profiles (tenant_id, name, kind, is_preset, policy)
             VALUES ($1, $2, $3, true, $4)",
        )
        .bind(tenant_id)
        .bind(p.name)
        .bind(p.kind)
        .bind(&p.policy)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    }

    let admin_id: Uuid = sqlx::query_scalar(
        "INSERT INTO admins (id, tenant_id, username, display_name, role, age_bracket)
         VALUES (COALESCE($1, gen_random_uuid()), $2, $3, $4, 'owner', 'adult') RETURNING id",
    )
    .bind(admin_id)
    .bind(tenant_id)
    .bind(username)
    .bind(display_name)
    .fetch_one(&mut *tx)
    .await
    .map_err(AppError::from)?;

    tx.commit().await.map_err(AppError::from)?;
    Ok((tenant_id, admin_id))
}

/// Normalize + validate an account username: 3–32 chars of `a–z 0–9 . _ -`,
/// lower-cased. (The SSO first run still lets people pick one.)
pub(crate) fn normalize_username(raw: &str) -> AppResult<String> {
    let u = raw.trim().to_ascii_lowercase();
    if u.chars().count() < 3 || u.chars().count() > 32 {
        return Err(AppError::BadRequest(
            "username must be 3–32 characters".into(),
        ));
    }
    if !u
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(AppError::BadRequest(
            "username may use only a–z, 0–9, dot, underscore, hyphen".into(),
        ));
    }
    Ok(u)
}

/// A login name made from a person's name, for the places that still want
/// one (the passkey's label in a password manager, signing in by name):
/// "Philip Ludwig" → "philip-ludwig". Nobody has to type it — the name they
/// gave works for signing in too.
pub(crate) fn username_from_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_matches('-').chars().take(28).collect();
    out = out.trim_matches('-').to_string();
    if out.len() < 3 {
        out = format!("{out}{}", if out.is_empty() { "parent" } else { "-me" });
    }
    out
}

/// `username_from_name`, made unique (usernames are unique across the server).
async fn unique_username(db: &sqlx::PgPool, name: &str) -> AppResult<String> {
    let base = username_from_name(name);
    for n in 1..100 {
        let candidate = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let taken: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM admins WHERE lower(username) = lower($1)")
                .bind(&candidate)
                .fetch_optional(db)
                .await?;
        if taken.is_none() {
            return Ok(candidate);
        }
    }
    Ok(format!("{base}-{}", &gen_token()[..6]))
}

/// A passkey that can't be found without a name first is useless to "Sign in
/// with a passkey". webauthn-rs asks for a non-resident key by default; ask
/// for a discoverable one. (The registration state doesn't check this, and
/// every platform passkey — phone, laptop, password manager — is one anyway.)
fn discoverable(ccr: CreationChallengeResponse) -> Value {
    let mut v = serde_json::to_value(ccr).unwrap();
    if let Some(sel) = v.pointer_mut("/publicKey/authenticatorSelection") {
        sel["residentKey"] = json!("required");
        sel["requireResidentKey"] = json!(true);
    }
    v
}

// ---------------------------------------------------------------------------
// First run: create your household
// ---------------------------------------------------------------------------

/// Whether `OST_OPEN_REGISTRATION=1` lets more households be created after
/// the first (API only — the console shows first-run on a fresh server only).
fn open_registration() -> bool {
    std::env::var("OST_OPEN_REGISTRATION").map(|v| v == "1") == Ok(true)
}

/// First run is open while no account exists — with the setup code, when the
/// server has one. After that it is closed (unless open registration is on).
async fn ensure_first_run(st: &AppState, setup_token: Option<&str>) -> AppResult<()> {
    let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM admins")
        .fetch_one(&st.db)
        .await?;
    if admins > 0 {
        if open_registration() {
            return Ok(());
        }
        return Err(AppError::RegistrationClosed(
            "this server already has a household — sign in instead".into(),
        ));
    }
    match st.bootstrap_token.as_deref() {
        Some(expected) => {
            let given = setup_token.unwrap_or("").trim();
            if !constant_time_eq(given.as_bytes(), expected.trim().as_bytes()) {
                return Err(AppError::Unauthorized(
                    "open the setup link the installer printed (the code is also in .env as \
                     OST_BOOTSTRAP_TOKEN)"
                        .into(),
                ));
            }
        }
        None => tracing::warn!(
            "first-run registration is OPEN: OST_BOOTSTRAP_TOKEN is not set — fine for a \
             local checkout, not for anything reachable from the internet"
        ),
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct RegisterStartReq {
    /// Your name, as the household will see it. Nothing else to pick.
    #[serde(default)]
    pub name: String,
    /// The one-time setup code (from the `#setup=` link).
    #[serde(default)]
    pub setup_token: Option<String>,
}

/// `POST /api/auth/register/start` — name in, passkey creation options out.
pub async fn register_start(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<RegisterStartReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let name = req.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(AppError::BadRequest("what's your name?".into()));
    }
    ensure_first_run(&st, req.setup_token.as_deref()).await?;

    let user_id = Uuid::new_v4();
    let username = unique_username(&st.db, &name).await?;
    let (ccr, reg) = st
        .webauthn
        .start_passkey_registration(user_id, &username, &name, None)
        .map_err(|e| AppError::BadRequest(format!("webauthn: {e}")))?;

    let key = gen_token();
    {
        let mut states = st.reg_states.write().await;
        states.retain(|_, c| c.created.elapsed() < CHALLENGE_TTL);
        states.insert(
            key.clone(),
            RegChallenge {
                user_id,
                username,
                display_name: name,
                existing_tenant: None,
                reg,
                created: std::time::Instant::now(),
            },
        );
    }
    let jar = jar.add(temp_cookie(REG_COOKIE, key, st.cookie_secure));
    Ok((jar, Json(discoverable(ccr))))
}

#[derive(Deserialize)]
pub struct RegisterFinishReq {
    pub credential: RegisterPublicKeyCredential,
    #[serde(default)]
    pub setup_token: Option<String>,
}

/// `POST /api/auth/register/finish` — the passkey in, the household and a
/// session out.
pub async fn register_finish(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<RegisterFinishReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    // Re-checked here (not just in start): the two calls aren't atomic, and the
    // finish must never mint a session after the first account appeared.
    ensure_first_run(&st, req.setup_token.as_deref()).await?;

    let challenge = take_reg_challenge(&st, &jar).await?;
    if challenge.existing_tenant.is_some() {
        return Err(AppError::BadRequest(
            "no household setup in progress".into(),
        ));
    }
    let passkey: Passkey = st
        .webauthn
        .finish_passkey_registration(&req.credential, &challenge.reg)
        .map_err(|e| AppError::BadRequest(format!("webauthn: {e}")))?;

    let (tenant_id, admin_id) = create_tenant_with_admin(
        &st.db,
        Some(challenge.user_id),
        &challenge.username,
        &challenge.display_name,
        !open_registration(),
    )
    .await?;
    store_passkey(&st.db, admin_id, &passkey, "passkey").await?;

    let sid = create_session(&st.db, admin_id, tenant_id).await?;
    let jar = jar
        .remove(Cookie::from(REG_COOKIE))
        .add(session_cookie(sid, st.cookie_secure));
    let admin = admin_json(&st.db, admin_id).await?;
    Ok((jar, Json(json!({ "admin": admin }))))
}

async fn take_reg_challenge(st: &AppState, jar: &CookieJar) -> AppResult<RegChallenge> {
    let key = jar
        .get(REG_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or_else(|| AppError::BadRequest("no passkey setup in progress".into()))?;
    st.reg_states
        .write()
        .await
        .remove(&key)
        .filter(|c| c.created.elapsed() < CHALLENGE_TTL)
        .ok_or_else(|| AppError::BadRequest("that took too long — try again".into()))
}

async fn store_passkey(
    db: &sqlx::PgPool,
    admin_id: Uuid,
    passkey: &Passkey,
    nickname: &str,
) -> AppResult<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO webauthn_credentials (admin_id, credential_id, passkey, nickname)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(admin_id)
    .bind(passkey.cred_id().as_ref())
    .bind(serde_json::to_value(passkey).unwrap())
    .bind(nickname)
    .fetch_one(db)
    .await?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Sign in with a passkey (no name first)
// ---------------------------------------------------------------------------

/// `POST /api/auth/login/start` — options for a discoverable-credential
/// assertion. There is no name to look up, so there is nothing to enumerate.
pub async fn login_start(
    State(st): State<AppState>,
    jar: CookieJar,
) -> AppResult<(CookieJar, Json<Value>)> {
    let (mut rcr, auth) = st
        .webauthn
        .start_discoverable_authentication()
        .map_err(|e| AppError::BadRequest(format!("webauthn: {e}")))?;
    // webauthn-rs marks this for autofill ("conditional") UI; this is a button.
    rcr.mediation = None;
    let jar = stash_auth_challenge(&st, jar, PasskeyCeremony::SignIn(auth)).await;
    Ok((jar, Json(serde_json::to_value(rcr).unwrap())))
}

#[derive(Deserialize)]
pub struct LoginFinishReq {
    pub credential: PublicKeyCredential,
}

/// `POST /api/auth/login/finish` — the passkey names its credential; the
/// stored credential names the account.
pub async fn login_finish(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginFinishReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let PasskeyCeremony::SignIn(state) = take_auth_challenge(&st, &jar).await? else {
        return Err(AppError::BadRequest("no sign-in in progress".into()));
    };
    let unknown = || AppError::Unauthorized("that passkey isn't registered on this server".into());
    let (_, cred_id) = st
        .webauthn
        .identify_discoverable_authentication(&req.credential)
        .map_err(|_| unknown())?;

    let row: Option<(Uuid, Uuid, Value)> = sqlx::query_as(
        "SELECT wc.admin_id, a.tenant_id, wc.passkey
           FROM webauthn_credentials wc JOIN admins a ON a.id = wc.admin_id
          WHERE wc.credential_id = $1",
    )
    .bind(cred_id)
    .fetch_optional(&st.db)
    .await?;
    let (admin_id, tenant_id, stored) = row.ok_or_else(unknown)?;
    let passkey: Passkey = serde_json::from_value(stored).map_err(|_| unknown())?;

    let result = st
        .webauthn
        .finish_discoverable_authentication(
            &req.credential,
            state,
            &[DiscoverableKey::from(&passkey)],
        )
        .map_err(|e| AppError::Unauthorized(format!("webauthn: {e}")))?;
    note_passkey_used(&st.db, admin_id, &result).await?;

    let sid = create_session(&st.db, admin_id, tenant_id).await?;
    let jar = jar
        .remove(Cookie::from(AUTH_COOKIE))
        .add(session_cookie(sid, st.cookie_secure));
    let admin = admin_json(&st.db, admin_id).await?;
    Ok((jar, Json(json!({ "admin": admin }))))
}

/// Park a passkey assertion challenge under a temp cookie.
pub(crate) async fn stash_auth_challenge(
    st: &AppState,
    jar: CookieJar,
    ceremony: PasskeyCeremony,
) -> CookieJar {
    let key = gen_token();
    {
        let mut states = st.auth_states.write().await;
        states.retain(|_, c| c.created.elapsed() < CHALLENGE_TTL);
        states.insert(
            key.clone(),
            AuthChallenge {
                ceremony,
                created: std::time::Instant::now(),
            },
        );
    }
    jar.add(temp_cookie(AUTH_COOKIE, key, st.cookie_secure))
}

/// Take (single use) the passkey challenge this browser started.
pub(crate) async fn take_auth_challenge(
    st: &AppState,
    jar: &CookieJar,
) -> AppResult<PasskeyCeremony> {
    let key = jar
        .get(AUTH_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or_else(|| AppError::BadRequest("no passkey check in progress".into()))?;
    st.auth_states
        .write()
        .await
        .remove(&key)
        .filter(|c| c.created.elapsed() < CHALLENGE_TTL)
        .map(|c| c.ceremony)
        .ok_or_else(|| AppError::BadRequest("that took too long — try again".into()))
}

pub async fn logout(
    State(st): State<AppState>,
    jar: CookieJar,
) -> AppResult<(CookieJar, Json<Value>)> {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        sqlx::query("DELETE FROM admin_sessions WHERE token_hash = $1")
            .bind(hash_token(c.value()))
            .execute(&st.db)
            .await?;
    }
    let jar = jar.remove(Cookie::from(SESSION_COOKIE));
    Ok((jar, Json(json!({ "ok": true }))))
}

// ---------------------------------------------------------------------------
// Your passkeys (Settings — the sensitive corner)
// ---------------------------------------------------------------------------

/// The admin's registered passkeys (metadata only — never the credential itself).
pub async fn list_passkeys(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    type PasskeyRow = (Uuid, String, DateTime<Utc>, Option<DateTime<Utc>>);
    let rows: Vec<PasskeyRow> = sqlx::query_as(
        "SELECT id, nickname, created_at, last_used_at FROM webauthn_credentials
         WHERE admin_id = $1 ORDER BY created_at",
    )
    .bind(admin.admin_id)
    .fetch_all(&st.db)
    .await?;
    let passkeys: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.0,
                "nickname": r.1,
                "created_at": r.2,
                "last_used_at": r.3,
            })
        })
        .collect();
    Ok(Json(json!({ "passkeys": passkeys })))
}

/// `POST /api/me/passkeys/new/start` — creation options for one more passkey
/// on the signed-in account. Inside the sensitive corner: a new permanent
/// credential is the most durable foothold there is.
pub async fn passkey_add_start(
    State(st): State<AppState>,
    admin: AuthAdmin,
    jar: CookieJar,
) -> AppResult<(CookieJar, Json<Value>)> {
    let (username, display): (Option<String>, String) =
        sqlx::query_as("SELECT username, display_name FROM admins WHERE id = $1")
            .bind(admin.admin_id)
            .fetch_one(&st.db)
            .await?;
    let existing: Vec<CredentialID> = load_passkeys(&st.db, admin.admin_id)
        .await?
        .iter()
        .map(|p| p.cred_id().clone())
        .collect();
    let (ccr, reg) = st
        .webauthn
        .start_passkey_registration(
            admin.admin_id,
            username.as_deref().unwrap_or(&display),
            &display,
            Some(existing),
        )
        .map_err(|e| AppError::BadRequest(format!("webauthn: {e}")))?;

    let key = gen_token();
    {
        let mut states = st.reg_states.write().await;
        states.retain(|_, c| c.created.elapsed() < CHALLENGE_TTL);
        states.insert(
            key.clone(),
            RegChallenge {
                user_id: admin.admin_id,
                username: username.unwrap_or_default(),
                display_name: display,
                existing_tenant: Some(admin.tenant_id),
                reg,
                created: std::time::Instant::now(),
            },
        );
    }
    let jar = jar.add(temp_cookie(REG_COOKIE, key, st.cookie_secure));
    Ok((jar, Json(discoverable(ccr))))
}

#[derive(Deserialize)]
pub struct PasskeyAddFinishReq {
    pub credential: RegisterPublicKeyCredential,
}

/// `POST /api/me/passkeys/new/finish`.
pub async fn passkey_add_finish(
    State(st): State<AppState>,
    admin: AuthAdmin,
    jar: CookieJar,
    Json(req): Json<PasskeyAddFinishReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let challenge = take_reg_challenge(&st, &jar).await?;
    if challenge.user_id != admin.admin_id || challenge.existing_tenant != Some(admin.tenant_id) {
        return Err(AppError::BadRequest("no passkey setup in progress".into()));
    }
    let passkey = st
        .webauthn
        .finish_passkey_registration(&req.credential, &challenge.reg)
        .map_err(|e| AppError::BadRequest(format!("webauthn: {e}")))?;
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webauthn_credentials WHERE admin_id = $1")
            .bind(admin.admin_id)
            .fetch_one(&st.db)
            .await?;
    let id = store_passkey(
        &st.db,
        admin.admin_id,
        &passkey,
        &format!("passkey {}", n + 1),
    )
    .await?;
    Ok((
        jar.remove(Cookie::from(REG_COOKIE)),
        Json(json!({ "ok": true, "id": id })),
    ))
}

/// Delete one of the admin's passkeys. Refuses to delete the last credential
/// unless OIDC is enabled (the admin would lock themselves out).
pub async fn delete_passkey(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM webauthn_credentials WHERE id = $1 AND admin_id = $2")
            .bind(id)
            .bind(admin.admin_id)
            .fetch_optional(&st.db)
            .await?;
    exists.ok_or_else(|| AppError::NotFound("passkey not found".into()))?;

    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webauthn_credentials WHERE admin_id = $1")
            .bind(admin.admin_id)
            .fetch_one(&st.db)
            .await?;
    if total <= 1 && st.oidc.is_none() {
        return Err(AppError::Conflict(
            "cannot delete the last passkey while SSO is disabled".into(),
        ));
    }

    sqlx::query("DELETE FROM webauthn_credentials WHERE id = $1 AND admin_id = $2")
        .bind(id)
        .bind(admin.admin_id)
        .execute(&st.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn admin_json(db: &sqlx::PgPool, admin_id: Uuid) -> AppResult<Value> {
    let row: (Uuid, Uuid, Option<String>, String, String, String) = sqlx::query_as(
        "SELECT id, tenant_id, username, display_name, role, age_bracket FROM admins WHERE id = $1",
    )
    .bind(admin_id)
    .fetch_one(db)
    .await?;
    Ok(json!({
        "id": row.0,
        "tenant_id": row.1,
        "username": row.2,
        "display_name": row.3,
        "role": row.4,
        "age_bracket": row.5,
    }))
}

pub(crate) async fn load_passkeys(db: &sqlx::PgPool, admin_id: Uuid) -> AppResult<Vec<Passkey>> {
    let rows: Vec<(Value,)> =
        sqlx::query_as("SELECT passkey FROM webauthn_credentials WHERE admin_id = $1")
            .bind(admin_id)
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(v,)| serde_json::from_value::<Passkey>(v).ok())
        .collect())
}

/// Record a successful assertion: the signature counter (clone detection) and
/// when the passkey was last used.
pub(crate) async fn note_passkey_used(
    db: &sqlx::PgPool,
    admin_id: Uuid,
    result: &webauthn_rs::prelude::AuthenticationResult,
) -> AppResult<()> {
    let rows: Vec<(Vec<u8>, Value)> = sqlx::query_as(
        "SELECT credential_id, passkey FROM webauthn_credentials WHERE admin_id = $1",
    )
    .bind(admin_id)
    .fetch_all(db)
    .await?;
    for (cred_id, v) in rows {
        if cred_id.as_slice() != result.cred_id().as_ref() {
            continue;
        }
        let mut changed = None;
        if let Ok(mut pk) = serde_json::from_value::<Passkey>(v) {
            if pk.update_credential(result) == Some(true) {
                changed = Some(serde_json::to_value(&pk).unwrap());
            }
        }
        sqlx::query(
            "UPDATE webauthn_credentials SET passkey = COALESCE($1, passkey), last_used_at = now()
             WHERE admin_id = $2 AND credential_id = $3",
        )
        .bind(changed)
        .bind(admin_id)
        .bind(&cred_id)
        .execute(db)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_becomes_a_login_name() {
        assert_eq!(username_from_name("Philip"), "philip");
        assert_eq!(username_from_name("  Philip Ludwig "), "philip-ludwig");
        assert_eq!(username_from_name("Zoë & Mia"), "zo-mia");
        assert_eq!(username_from_name("Al"), "al-me");
        assert_eq!(username_from_name("😀"), "parent");
        let long = username_from_name(&"a".repeat(80));
        assert!(long.len() <= 32);
        // Whatever comes out is a valid username.
        for n in ["Philip", "  x ", "Ünal Öz", "a-b-c", "--"] {
            assert!(normalize_username(&username_from_name(n)).is_ok(), "{n}");
        }
    }

    #[test]
    fn constant_time_eq_is_plain_equality() {
        assert!(constant_time_eq(b"123456", b"123456"));
        assert!(!constant_time_eq(b"123456", b"123457"));
        assert!(!constant_time_eq(b"123456", b"1234567"));
        assert!(!constant_time_eq(b"", b"1"));
    }
}
