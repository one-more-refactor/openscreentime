//! Trust at login — "prove it's you at the door, then just use it."
//!
//! A session born from a completed login ceremony — passkey, SSO, or an
//! enrolled device's voucher — is **trusted** and mutates freely. There is no
//! separate change-mode ceremony any more: the proof happened at sign-in.
//! A session that is *not* trusted (pre-migration cookies, or a future weak
//! login path) gets `428 step_up_required` on any mutation; passing one factor
//! makes it trusted for good, which is "verify once at login" in practice.
//!
//! What remains guarded for everyone is the **sensitive corner** — the handful
//! of routes that are themselves takeover surface (a child's unlock code and
//! recovery codes, the passkey inventory, standing pairing tokens). Touching
//! those needs a short-lived **confirm window** on the session: one factor —
//! a code, or a Telegram tap — opens it for a few minutes.
//!
//! Two design notes worth keeping:
//!
//! **It is a layer, not a per-handler extractor.** A layer over the whole
//! `/api` router with a small, explicit exempt list means a new mutating route
//! is guarded the moment it exists — failing closed instead of depending on
//! nobody ever forgetting a parameter. The exempt list is the auth flow
//! itself, which cannot require the thing it is producing.
//!
//! **The verifier reads the secret.** TOTP secrets are stored as base32, not
//! hashed: a one-way digest cannot generate the next code. That is inherent to
//! TOTP, not an oversight — the thing to protect is the database, which already
//! holds sessions and device tokens.

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
    Json,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use sha1::Sha1;
use uuid::Uuid;

use crate::auth::{gen_token, hash_token, session_cookie};
use crate::error::{AppError, AppResult};
use crate::state::{AppState, AuthAdmin, SESSION_COOKIE};

/// How long a confirm window lasts once a factor is passed — long enough to
/// read an unlock code to a child or rotate a device's keys, short enough
/// that a walked-away-from console is not a standing permission.
const GRANT_MINUTES: i64 = 15;
/// Emailed codes are single-use and short-lived.
const EMAIL_CODE_MINUTES: i64 = 10;
/// Wrong second factors before the account has to wait.
const MAX_FAILS: i32 = 5;
/// The first lockout; it doubles from there, capped.
const LOCKOUT_BASE_SECS: i64 = 30;
const LOCKOUT_MAX_SECS: i64 = 900;
/// TOTP: RFC 6238 defaults, and one step of drift either side.
pub const TOTP_STEP: u64 = 30;
const TOTP_SKEW: i64 = 1;
const TOTP_DIGITS: u32 = 6;

// ── TOTP ────────────────────────────────────────────────────────────────────

/// RFC 6238 over HMAC-SHA1, the shape every authenticator app implements.
pub fn totp_at(secret_b32: &str, counter: u64) -> Option<String> {
    let key = base32::decode(
        base32::Alphabet::Rfc4648 { padding: false },
        &secret_b32.to_uppercase(),
    )?;
    let mut mac = Hmac::<Sha1>::new_from_slice(&key).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let code = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]) % 10u32.pow(TOTP_DIGITS);
    Some(format!("{code:0width$}", width = TOTP_DIGITS as usize))
}

fn now_counter() -> u64 {
    (Utc::now().timestamp() as u64) / TOTP_STEP
}

/// The code that is valid *right now* for a secret, and how many seconds it
/// has left. This is what the console shows a parent as a device's unlock
/// code: the server is the authenticator, the parent just reads.
pub fn current_totp(secret_b32: &str) -> Option<(String, u64)> {
    let now = Utc::now().timestamp() as u64;
    let code = totp_at(secret_b32, now / TOTP_STEP)?;
    Some((code, TOTP_STEP - now % TOTP_STEP))
}

/// Recovery-code MAC: hex HMAC-SHA256 over the ASCII digits of the code,
/// keyed by the device's decoded TOTP secret. The agent computes the same
/// (client `parentcode::recovery_mac`) to verify offline; the test vector
/// below is shared with the client's tests so the two never drift.
pub fn recovery_mac(secret_b32: &str, code: &str) -> Option<String> {
    let key = base32::decode(
        base32::Alphabet::Rfc4648 { padding: false },
        &secret_b32.to_uppercase(),
    )?;
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&key).ok()?;
    mac.update(digits_only(code).as_bytes());
    Some(hex::encode(mac.finalize().into_bytes()))
}

/// A fresh 160-bit secret, base32 as the apps expect it.
pub(crate) fn gen_totp_secret() -> String {
    let bytes: [u8; 20] = rand::thread_rng().gen();
    base32::encode(base32::Alphabet::Rfc4648 { padding: false }, &bytes)
}

fn digits_only(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

// ── the guard ───────────────────────────────────────────────────────────────

/// Mutating `/api` paths that must NOT require a grant, because they are how a
/// grant is obtained (or how you sign in or out at all). Everything else that
/// mutates is guarded, including routes that do not exist yet.
fn exempt(path: &str) -> bool {
    path.starts_with("/api/auth/register/")
        || path.starts_with("/api/auth/login/")
        || path.starts_with("/api/auth/device/")
        || path == "/api/auth/logout"
        || path == "/api/auth/voucher"
        || path == "/api/auth/stepup/verify"
        || path == "/api/auth/stepup/email/start"
        || path == "/api/auth/stepup/telegram/start"
        // Locking change mode early must never itself need change mode.
        || path == "/api/auth/stepup/lock"
        || path == "/api/me/2fa/totp/start"
        || path == "/api/me/2fa/totp/confirm"
        // A child asking for more time is not a takeover surface: it only
        // creates a request the parent still has to answer, and a member
        // usually has no second factor to give.
        || path == "/api/me/ask"
}

/// The sensitive corner: routes that are themselves takeover surface, read
/// **or** write. A passkey list tells an attacker what to remove; a pairing
/// token is standing parent access; a device's unlock code is the key to a
/// child's machine and its recovery codes are the spares. Touching any of
/// these needs a live confirm window, whoever you are.
/// (`GET /api/me/2fa` stays free — the confirm dialog needs it to know which
/// factors to offer BEFORE any window exists.)
fn sensitive(path: &str) -> bool {
    path.starts_with("/api/me/passkeys")
        || path.starts_with("/api/me/telegram")
        || path.starts_with("/api/parent-tokens")
        // Re-pointing an OS login at another account re-keys the whole
        // device-voucher trust chain (a child's laptop could then mint parent
        // sessions), and an enroll token is standing device access — both are
        // at least as powerful as an unlock code.
        || path.ends_with("/assign-account")
        || path.ends_with("/enroll-token")
        // A VPN config is applied verbatim on the device and reshapes its
        // routing — creating/editing/activating one is takeover-adjacent, so
        // it may not ride ordinary session trust.
        || path.starts_with("/api/vpn-profiles")
        || path.ends_with("/vpn")
        || (path.starts_with("/api/devices/")
            && (path.ends_with("/unlock-code")
                || path.ends_with("/unlock-code/rotate")
                || path.ends_with("/recovery-codes")))
}

/// Layer over the `/api` router.
///
/// Ordinary reads pass straight through, and ordinary mutations pass for any
/// **trusted** session — trust was earned at login. What stops here:
/// a sensitive route (read or write) without a live confirm window, and a
/// mutation from an untrusted session (which one passed factor repairs, for
/// the life of the session).
pub async fn require_step_up(
    State(st): State<AppState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    let mutating = !matches!(
        method,
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    );
    let needs_confirm = sensitive(&path) && !exempt(&path);
    let needs_trust = mutating && path.starts_with("/api/") && !exempt(&path);
    if !needs_confirm && !needs_trust {
        return Ok(next.run(req).await);
    }

    // No session at all is a 401 from the handler's own extractor, not a 428:
    // "you are not signed in" and "prove it is you again" are different
    // answers and the client acts on them differently.
    let Some(cookie) = jar.get(SESSION_COOKIE) else {
        return Ok(next.run(req).await);
    };

    let hash = hash_token(cookie.value());
    let row: Option<(Option<DateTime<Utc>>, bool)> = sqlx::query_as(
        "SELECT stepup_until, trusted FROM admin_sessions
         WHERE (token_hash = $1
                OR (prev_token_hash = $1 AND prev_valid_until > now()))
           AND expires_at > now()",
    )
    .bind(&hash)
    .fetch_optional(&st.db)
    .await?;

    match row {
        // Unknown session: let the handler's extractor produce the 401.
        None => Ok(next.run(req).await),
        Some((until, trusted)) => {
            if needs_confirm && !until.is_some_and(|t| t > Utc::now()) {
                return Err(AppError::StepUpRequired(
                    "confirm it's you to touch the keys".into(),
                ));
            }
            if needs_trust && !trusted {
                return Err(AppError::StepUpRequired(
                    "this session hasn't proved itself yet — one code fixes that for good".into(),
                ));
            }
            Ok(next.run(req).await)
        }
    }
}

/// Resolve the session row id for the current cookie, honouring the rotation
/// grace window.
async fn session_id_for(st: &AppState, jar: &CookieJar) -> AppResult<Uuid> {
    let cookie = jar
        .get(SESSION_COOKIE)
        .ok_or_else(|| AppError::Unauthorized("no session".into()))?;
    let hash = hash_token(cookie.value());
    sqlx::query_scalar(
        "SELECT id FROM admin_sessions
         WHERE (token_hash = $1
                OR (prev_token_hash = $1 AND prev_valid_until > now()))
           AND expires_at > now()",
    )
    .bind(&hash)
    .fetch_optional(&st.db)
    .await?
    .ok_or_else(|| AppError::Unauthorized("no session".into()))
}

/// Open the session's confirm window, mark it trusted for good, and rotate
/// its token while we are here — a passed factor is the natural moment to
/// re-issue, since the user has just re-proved themselves.
async fn grant_and_rotate(
    st: &AppState,
    jar: CookieJar,
    session_id: Uuid,
) -> AppResult<(CookieJar, DateTime<Utc>)> {
    let expires = Utc::now() + Duration::minutes(GRANT_MINUTES);
    let fresh = gen_token();
    let fresh_hash = hash_token(&fresh);

    sqlx::query(
        "UPDATE admin_sessions
            SET prev_token_hash  = token_hash,
                prev_valid_until = now() + interval '2 minutes',
                token_hash       = $2,
                stepup_until     = $3,
                stepup_extended  = false,
                trusted          = true,
                last_seen_at     = now(),
                expires_at       = GREATEST(expires_at, now() + interval '7 days')
          WHERE id = $1",
    )
    .bind(session_id)
    .bind(&fresh_hash)
    .bind(expires)
    .execute(&st.db)
    .await?;

    Ok((jar.add(session_cookie(fresh, st.cookie_secure)), expires))
}

// ── failure counting ────────────────────────────────────────────────────────

async fn locked_until(st: &AppState, admin_id: Uuid) -> AppResult<Option<DateTime<Utc>>> {
    let t: Option<Option<DateTime<Utc>>> =
        sqlx::query_scalar("SELECT stepup_locked_until FROM admins WHERE id = $1")
            .bind(admin_id)
            .fetch_optional(&st.db)
            .await?;
    Ok(t.flatten().filter(|t| *t > Utc::now()))
}

async fn note_failure(st: &AppState, admin_id: Uuid) -> AppResult<()> {
    let fails: i32 = sqlx::query_scalar(
        "UPDATE admins SET stepup_fails = stepup_fails + 1 WHERE id = $1 RETURNING stepup_fails",
    )
    .bind(admin_id)
    .fetch_one(&st.db)
    .await?;

    if fails >= MAX_FAILS {
        let over = (fails - MAX_FAILS).min(8) as u32;
        let secs = (LOCKOUT_BASE_SECS * 2i64.pow(over)).min(LOCKOUT_MAX_SECS);
        sqlx::query("UPDATE admins SET stepup_locked_until = now() + make_interval(secs => $2) WHERE id = $1")
            .bind(admin_id)
            .bind(secs as f64)
            .execute(&st.db)
            .await?;
    }
    Ok(())
}

async fn clear_failures(st: &AppState, admin_id: Uuid) -> AppResult<()> {
    sqlx::query("UPDATE admins SET stepup_fails = 0, stepup_locked_until = NULL WHERE id = $1")
        .bind(admin_id)
        .execute(&st.db)
        .await?;
    Ok(())
}

// ── handlers ────────────────────────────────────────────────────────────────

/// `GET /api/me/2fa` → what factors this account can actually use.
pub async fn status(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let confirmed: Option<Option<DateTime<Utc>>> =
        sqlx::query_scalar("SELECT totp_confirmed_at FROM admins WHERE id = $1")
            .bind(admin.admin_id)
            .fetch_optional(&st.db)
            .await?;
    let locked = locked_until(&st, admin.admin_id).await?;
    // A paired phone is a factor the dialog can offer (one tap, no typing).
    let telegram: Option<i64> = if crate::telegram::bot_token().is_some() {
        sqlx::query_scalar("SELECT chat_id FROM telegram_chats WHERE admin_id = $1 LIMIT 1")
            .bind(admin.admin_id)
            .fetch_optional(&st.db)
            .await?
    } else {
        None
    };

    Ok(Json(json!({
        "totp_enrolled": confirmed.flatten().is_some(),
        "email_available": email_sender_configured(),
        "telegram_available": telegram.is_some(),
        "locked_until": locked,
    })))
}

/// `POST /api/me/2fa/totp/start` → a secret and its `otpauth://` URI, shown once.
///
/// Deliberately re-issuable while unconfirmed (a half-finished enrolment should
/// not wedge the account), but refused once confirmed — replacing a working
/// authenticator is a removal followed by an enrolment, and removal is itself a
/// guarded mutation.
pub async fn totp_start(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let row: Option<(Option<DateTime<Utc>>, Option<String>)> =
        sqlx::query_as("SELECT totp_confirmed_at, email FROM admins WHERE id = $1")
            .bind(admin.admin_id)
            .fetch_optional(&st.db)
            .await?;
    let (confirmed, email) = row.ok_or_else(|| AppError::NotFound("admin".into()))?;
    if confirmed.is_some() {
        return Err(AppError::Conflict(
            "an authenticator is already enrolled".into(),
        ));
    }

    let secret = gen_totp_secret();
    sqlx::query("UPDATE admins SET totp_secret = $2, totp_last_counter = 0 WHERE id = $1")
        .bind(admin.admin_id)
        .bind(&secret)
        .execute(&st.db)
        .await?;

    let label = email.unwrap_or_else(|| "account".into());
    Ok(Json(json!({
        "secret": secret,
        "otpauth_uri": otpauth_uri(&label, &secret),
    })))
}

/// The `otpauth://` URI an authenticator app scans for `label`.
pub(crate) fn otpauth_uri(label: &str, secret: &str) -> String {
    let label = urlencoding_min(&format!("OpenScreenTime:{label}"));
    format!(
        "otpauth://totp/{label}?secret={secret}&issuer=OpenScreenTime&period={TOTP_STEP}&digits={TOTP_DIGITS}"
    )
}

/// Percent-encode only what an `otpauth:` label actually breaks on. Pulling in
/// a URL-encoding dependency for two characters would be worse.
fn urlencoding_min(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '#' => "%23".to_string(),
            '?' => "%3F".to_string(),
            '&' => "%26".to_string(),
            c => c.to_string(),
        })
        .collect()
}

#[derive(Deserialize)]
pub struct CodeReq {
    #[serde(default)]
    pub code: String,
}

/// `POST /api/me/2fa/totp/confirm` — prove one live code before the secret
/// counts. An unconfirmed secret is one nobody has.
pub async fn totp_confirm(
    State(st): State<AppState>,
    admin: AuthAdmin,
    jar: CookieJar,
    Json(req): Json<CodeReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let secret: Option<Option<String>> = sqlx::query_scalar(
        "SELECT totp_secret FROM admins WHERE id = $1 AND totp_confirmed_at IS NULL",
    )
    .bind(admin.admin_id)
    .fetch_optional(&st.db)
    .await?;
    let secret = secret
        .flatten()
        .ok_or_else(|| AppError::BadRequest("start an enrolment first".into()))?;
    let session_id = session_id_for(&st, &jar).await?;

    let code = digits_only(&req.code);
    let counter = now_counter();
    let matched = (-TOTP_SKEW..=TOTP_SKEW).find_map(|d| {
        let c = counter.wrapping_add_signed(d);
        (totp_at(&secret, c).as_deref() == Some(code.as_str())).then_some(c)
    });

    let Some(used) = matched else {
        return Err(AppError::BadRequest("that code didn't match".into()));
    };

    sqlx::query(
        "UPDATE admins SET totp_confirmed_at = now(), totp_last_counter = $2 WHERE id = $1",
    )
    .bind(admin.admin_id)
    .bind(used as i64)
    .execute(&st.db)
    .await?;
    clear_failures(&st, admin.admin_id).await?;

    // You just proved the factor, so you are stepped up. Making people wait
    // out the 30-second window before their first change — because confirming
    // spent that code — would be friction with no security in it.
    let (jar, expires) = grant_and_rotate(&st, jar, session_id).await?;
    Ok((jar, Json(json!({ "ok": true, "expires_at": expires }))))
}

// ── emailed codes ───────────────────────────────────────────────────────────

fn email_sender_configured() -> bool {
    // Dev builds emit the code to the server log, which is a real (if blunt)
    // delivery channel for a homelab. Prod points this at a webhook.
    std::env::var("OST_STEPUP_WEBHOOK").is_ok()
        || cfg!(debug_assertions)
        || std::env::var("OST_STEPUP_LOG_CODES").is_ok()
}

/// `POST /api/auth/stepup/email/start` — mint a single-use code and send it.
pub async fn email_start(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    if let Some(t) = locked_until(&st, admin.admin_id).await? {
        return Err(AppError::RateLimited(format!(
            "too many attempts — try again at {t}"
        )));
    }

    let code: String = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
    sqlx::query(
        "INSERT INTO stepup_email_codes (admin_id, code_hash, expires_at)
         VALUES ($1, $2, now() + make_interval(mins => $3))",
    )
    .bind(admin.admin_id)
    .bind(hash_token(&code))
    // make_interval's `mins` is an integer; only `secs` is double precision.
    .bind(EMAIL_CODE_MINUTES as i32)
    .execute(&st.db)
    .await?;

    let email: Option<String> = sqlx::query_scalar("SELECT email FROM admins WHERE id = $1")
        .bind(admin.admin_id)
        .fetch_one(&st.db)
        .await?;
    let Some(email) = email else {
        return Err(AppError::BadRequest(
            "this account has no email — use an authenticator app".into(),
        ));
    };

    if let Ok(hook) = std::env::var("OST_STEPUP_WEBHOOK") {
        // Fire-and-forget: a slow or broken notifier must not hold the request
        // open, and the code is already valid either way.
        let body = json!({ "email": email, "code": code, "kind": "stepup" });
        tokio::spawn(async move {
            let _ = reqwest::Client::new().post(hook).json(&body).send().await;
        });
    } else {
        tracing::warn!(target: "stepup", "step-up code for {email}: {code}");
    }

    Ok(Json(json!({ "ok": true })))
}

// ── verification ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct VerifyReq {
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub code: String,
}

/// `POST /api/auth/stepup/verify` — pass a factor, get a grant.
pub async fn verify(
    State(st): State<AppState>,
    admin: AuthAdmin,
    jar: CookieJar,
    Json(req): Json<VerifyReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    if let Some(t) = locked_until(&st, admin.admin_id).await? {
        return Err(AppError::RateLimited(format!(
            "too many attempts — try again at {t}"
        )));
    }
    let session_id = session_id_for(&st, &jar).await?;
    let code = digits_only(&req.code);
    if code.is_empty() {
        return Err(AppError::BadRequest("no code".into()));
    }

    let ok = match req.method.as_str() {
        "totp" => verify_totp(&st, admin.admin_id, &code).await?,
        "email" => verify_email(&st, admin.admin_id, &code).await?,
        other => {
            return Err(AppError::BadRequest(format!("unknown factor: {other}")));
        }
    };

    if !ok {
        note_failure(&st, admin.admin_id).await?;
        return Err(AppError::BadRequest("that code didn't match".into()));
    }

    clear_failures(&st, admin.admin_id).await?;
    let (jar, expires) = grant_and_rotate(&st, jar, session_id).await?;
    Ok((
        jar,
        Json(json!({ "method": req.method, "expires_at": expires, "extended": false })),
    ))
}

// ── the confirm window ──────────────────────────────────────────────────────
//
// These three handlers let the console show, end and stretch the sensitive-
// corner confirm window — so "is it open?" survives a reload, "close it" is a
// click, and one overrun does not mean typing a code again.

async fn change_mode_json(st: &AppState, session_id: Uuid) -> AppResult<Value> {
    let row: (Option<DateTime<Utc>>, bool) =
        sqlx::query_as("SELECT stepup_until, stepup_extended FROM admin_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&st.db)
            .await?;
    let armed_until = row.0.filter(|t| *t > Utc::now());
    Ok(json!({
        "armed_until": armed_until,
        "extended": armed_until.is_some() && row.1,
    }))
}

/// `GET /api/auth/stepup` → is this session in change mode, until when, and
/// has it used its extension. A plain read.
pub async fn change_mode_status(
    State(st): State<AppState>,
    _admin: AuthAdmin,
    jar: CookieJar,
) -> AppResult<Json<Value>> {
    let session_id = session_id_for(&st, &jar).await?;
    Ok(Json(change_mode_json(&st, session_id).await?))
}

/// `POST /api/auth/stepup/lock` → leave change mode now. Exempt from the
/// guard (you are giving power up, not taking it).
pub async fn change_mode_lock(
    State(st): State<AppState>,
    _admin: AuthAdmin,
    jar: CookieJar,
) -> AppResult<Json<Value>> {
    let session_id = session_id_for(&st, &jar).await?;
    sqlx::query(
        "UPDATE admin_sessions SET stepup_until = NULL, stepup_extended = false WHERE id = $1",
    )
    .bind(session_id)
    .execute(&st.db)
    .await?;
    Ok(Json(json!({ "armed_until": null, "extended": false })))
}

/// `POST /api/auth/stepup/extend` → another window from now, once per grant.
/// Guarded by the layer, so it only works while change mode is live — an
/// expired grant cannot be revived without a factor.
pub async fn change_mode_extend(
    State(st): State<AppState>,
    _admin: AuthAdmin,
    jar: CookieJar,
) -> AppResult<Json<Value>> {
    let session_id = session_id_for(&st, &jar).await?;
    let expires = Utc::now() + Duration::minutes(GRANT_MINUTES);
    let updated = sqlx::query(
        "UPDATE admin_sessions SET stepup_until = $2, stepup_extended = true
          WHERE id = $1 AND stepup_extended = false AND stepup_until > now()",
    )
    .bind(session_id)
    .bind(expires)
    .execute(&st.db)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(AppError::Conflict(
            "change mode was already extended once — lock and enter a code again".into(),
        ));
    }
    Ok(Json(json!({ "armed_until": expires, "extended": true })))
}

async fn verify_totp(st: &AppState, admin_id: Uuid, code: &str) -> AppResult<bool> {
    let row: Option<(Option<String>, Option<DateTime<Utc>>, i64)> = sqlx::query_as(
        "SELECT totp_secret, totp_confirmed_at, totp_last_counter FROM admins WHERE id = $1",
    )
    .bind(admin_id)
    .fetch_optional(&st.db)
    .await?;

    let Some((Some(secret), Some(_), last)) = row else {
        return Ok(false); // no authenticator enrolled — email is the way in
    };

    let counter = now_counter();
    for drift in -TOTP_SKEW..=TOTP_SKEW {
        let c = counter.wrapping_add_signed(drift);
        // Single use: a code at or below the highest already spent is dead,
        // even if it is still inside its own time window.
        if (c as i64) <= last {
            continue;
        }
        if totp_at(&secret, c).as_deref() == Some(code) {
            sqlx::query("UPDATE admins SET totp_last_counter = $2 WHERE id = $1")
                .bind(admin_id)
                .bind(c as i64)
                .execute(&st.db)
                .await?;
            return Ok(true);
        }
    }
    Ok(false)
}

async fn verify_email(st: &AppState, admin_id: Uuid, code: &str) -> AppResult<bool> {
    let hash = hash_token(code);
    let id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE stepup_email_codes SET consumed_at = now()
          WHERE id = (SELECT id FROM stepup_email_codes
                       WHERE admin_id = $1 AND code_hash = $2
                         AND consumed_at IS NULL AND expires_at > now()
                       ORDER BY created_at DESC LIMIT 1)
        RETURNING id",
    )
    .bind(admin_id)
    .bind(&hash)
    .fetch_optional(&st.db)
    .await?;

    if id.is_none() {
        // Charge the attempt against every live code so brute force burns them.
        sqlx::query(
            "UPDATE stepup_email_codes SET attempts = attempts + 1
              WHERE admin_id = $1 AND consumed_at IS NULL AND expires_at > now()",
        )
        .bind(admin_id)
        .execute(&st.db)
        .await?;
        sqlx::query("DELETE FROM stepup_email_codes WHERE attempts >= $1")
            .bind(MAX_FAILS)
            .execute(&st.db)
            .await?;
    }
    Ok(id.is_some())
}

// ── device-voucher autologin ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct MintVoucherReq {
    /// The OS login on the device that wants to open the console. The session
    /// is issued for the person that login is linked to — a child's laptop
    /// opens the child's page, never the parent's.
    #[serde(default)]
    pub os_username: String,
}

/// `POST /agent/voucher` — the enrolled client mints a one-time voucher for a
/// local surface on its own machine (the browser, the notch) to exchange.
///
/// The device is authenticated by its bearer token, so a voucher is only ever
/// as good as possession of the machine — and under trust-at-login that is
/// the point: your own signed-in computer signs you in. The sensitive corner
/// (unlock codes, passkeys, pairing tokens) still asks for a factor.
///
/// The voucher is bound to the **account** behind `os_username`
/// (`device_users.account_id`). An OS login nobody is linked to gets
/// `404 no_account`, not the founding parent's session.
pub async fn mint_voucher(
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

    // Opportunistic cleanup; vouchers are tiny but they are also useless after
    // two minutes and there is no reason to keep them.
    let _ = sqlx::query("DELETE FROM device_vouchers WHERE expires_at < now() - interval '1 hour'")
        .execute(&st.db)
        .await;

    Ok(Json(
        json!({ "voucher": voucher, "expires_in_secs": 120, "account_id": account_id }),
    ))
}

#[derive(Deserialize)]
pub struct VoucherReq {
    #[serde(default)]
    pub voucher: String,
}

/// `POST /api/auth/voucher` — voucher in, session out, server-verified.
///
/// The session is issued for the account the voucher was minted for (the
/// person behind the OS login that asked), and it is trusted from birth.
pub async fn redeem_voucher(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<VoucherReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let hash = hash_token(&req.voucher);
    let row: Option<(Uuid, Uuid, Option<Uuid>)> = sqlx::query_as(
        "UPDATE device_vouchers SET consumed_at = now()
          WHERE voucher_hash = $1 AND consumed_at IS NULL AND expires_at > now()
        RETURNING device_id, tenant_id, account_id",
    )
    .bind(&hash)
    .fetch_optional(&st.db)
    .await?;

    let (device_id, tenant_id, account_id) =
        row.ok_or_else(|| AppError::Unauthorized("voucher not valid".into()))?;

    // The device must still be enrolled in that tenant — a de-enrolled machine
    // holding an old voucher is not a member of anything.
    let live: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM devices WHERE id = $1 AND tenant_id = $2")
            .bind(device_id)
            .bind(tenant_id)
            .fetch_optional(&st.db)
            .await?;
    if live.is_none() {
        return Err(AppError::Unauthorized("device is not enrolled".into()));
    }

    // And the person must still exist in that household.
    let account_id =
        account_id.ok_or_else(|| AppError::Unauthorized("voucher not valid".into()))?;
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM admins WHERE id = $1 AND tenant_id = $2")
            .bind(account_id)
            .bind(tenant_id)
            .fetch_optional(&st.db)
            .await?;
    let role = role.ok_or_else(|| AppError::Unauthorized("no account on this household".into()))?;

    let token = gen_token();
    // A voucher session is a completed login on an enrolled machine: trusted,
    // for a parent and a child alike. Possession of your own signed-in
    // computer *is* the proof — the same bar Screen Time on a phone sets.
    sqlx::query(
        "INSERT INTO admin_sessions (token_hash, admin_id, tenant_id, expires_at, via_voucher, trusted)
         VALUES ($1, $2, $3, now() + interval '7 days', true, true)",
    )
    .bind(hash_token(&token))
    .bind(account_id)
    .bind(tenant_id)
    .execute(&st.db)
    .await?;

    Ok((
        jar.add(session_cookie(token, st.cookie_secure)),
        Json(json!({
            "ok": true, "via": "device_voucher", "account_id": account_id, "role": role
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 test vector: the ASCII secret "12345678901234567890" at
    /// T=59 (counter 1) is 287082 for SHA-1/6 digits.
    #[test]
    fn totp_matches_the_rfc_vector() {
        let secret = base32::encode(
            base32::Alphabet::Rfc4648 { padding: false },
            b"12345678901234567890",
        );
        assert_eq!(totp_at(&secret, 1).as_deref(), Some("287082"));
        assert_eq!(totp_at(&secret, 37037036).as_deref(), Some("081804"));
    }

    #[test]
    fn totp_rejects_a_bad_secret() {
        assert!(totp_at("not base32 !!", 1).is_none());
    }

    #[test]
    fn digits_only_strips_the_spaces_people_type() {
        assert_eq!(digits_only(" 123 456 "), "123456");
    }

    #[test]
    fn the_auth_flow_itself_is_never_step_up_guarded() {
        // Otherwise you would need a grant to get a grant.
        assert!(exempt("/api/auth/stepup/verify"));
        assert!(exempt("/api/auth/login/finish"));
        assert!(exempt("/api/me/2fa/totp/confirm"));
        assert!(exempt("/api/auth/voucher"));
        assert!(exempt("/api/me/ask"));
        // Leaving change mode never needs change mode; extending it does.
        assert!(exempt("/api/auth/stepup/lock"));
        assert!(!exempt("/api/auth/stepup/extend"));
    }

    /// Shared with the client (`parentcode::recovery_mac` test): the two sides
    /// must agree byte-for-byte or no recovery code would ever open a door.
    #[test]
    fn recovery_mac_matches_the_shared_vector() {
        assert_eq!(
            recovery_mac("GEZDGNBVGY3TQOJQ", "12345678").as_deref(),
            Some("0008171f02a4c9c7b347dcc77ff65745007d09e8b442eef48f92de5f11e953cd")
        );
        // The space people type is not part of the message.
        assert_eq!(
            recovery_mac("GEZDGNBVGY3TQOJQ", "1234 5678"),
            recovery_mac("GEZDGNBVGY3TQOJQ", "12345678")
        );
        assert!(recovery_mac("not base32 !!", "12345678").is_none());
    }

    #[test]
    fn current_totp_counts_down_within_the_step() {
        let (code, left) = current_totp("GEZDGNBVGY3TQOJQ").unwrap();
        assert_eq!(code.len(), 6);
        assert!((1..=TOTP_STEP).contains(&left));
    }

    #[test]
    fn the_sensitive_corner_covers_reads_and_writes() {
        assert!(sensitive("/api/me/passkeys"));
        assert!(sensitive("/api/me/passkeys/abc")); // removal is the takeover
        assert!(sensitive("/api/parent-tokens"));
        assert!(sensitive("/api/parent-tokens/abc"));
        // The per-device unlock code is the key to the child's machine, the
        // recovery codes are the spares, and re-keying mints new ones.
        assert!(sensitive("/api/devices/abc/unlock-code"));
        assert!(sensitive("/api/devices/abc/unlock-code/rotate"));
        assert!(sensitive("/api/devices/abc/recovery-codes"));
        assert!(!sensitive("/api/devices/abc/parent-code"));
        // The status the confirm dialog itself needs must stay free, or you
        // would need a window to find out how to open a window.
        assert!(!sensitive("/api/me/2fa"));
        // Re-keying the voucher trust chain and minting device credentials are
        // takeover surface too (they slipped through as ordinary mutations).
        assert!(sensitive("/api/device-users/abc/assign-account"));
        assert!(sensitive("/api/devices/abc/enroll-token"));
        // Everything else — reads and ordinary mutations — stays out of it.
        assert!(!sensitive("/api/devices"));
        assert!(!sensitive("/api/devices/abc/lock"));
        assert!(!sensitive("/api/device-users/abc/credit-time"));
    }

    #[test]
    fn ordinary_mutations_ride_on_session_trust_not_exemption() {
        // Not exempt = they still go through the layer, where a trusted
        // session passes and an untrusted one is asked for a factor once.
        assert!(!exempt("/api/devices"));
        assert!(!exempt("/api/profiles/abc"));
        assert!(!exempt("/api/device-users/abc/credit-time"));
        assert!(!exempt("/api/devices/abc/lock"));
        // Including routes nobody has written yet.
        assert!(!exempt("/api/something/invented/tomorrow"));
    }
}
