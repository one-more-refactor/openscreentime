//! "Confirm it's you" — the sensitive corner.
//!
//! Signing in is the proof; inside, everything just works. The exception is a
//! handful of routes that are themselves takeover surface: a computer's unlock
//! code and recovery codes, your passkeys, standing pairing tokens, the
//! Telegram pairing, re-pointing an OS login at another person, a fresh enroll
//! token, VPN configs. Those need a live **confirm window** on the session
//! (`admin_sessions.stepup_until`, 15 minutes).
//!
//! A fresh sign-in opens the window. After that, either of the two things you
//! sign in with opens it again:
//!
//! * **your passkey**, or
//! * **a code shown on your own computer** (the same code machinery as the
//!   sign-in door, bound to this session instead of a PKCE verifier).
//!
//! An account with neither (one that only ever signs in with SSO) confirms by
//! signing in again — so the dialog is never a dead end.
//!
//! It is a **layer**, not a per-handler extractor: a new sensitive route is
//! guarded the moment it matches `sensitive()`, and nobody can forget a
//! parameter. The same layer refuses every change from a paused account.
//! The one call whose sensitivity depends on its body — `POST /api/devices`
//! for a *parent's* own computer, whose enroll token becomes device vouchers
//! for that parent — asks [`require_window`] itself.

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use webauthn_rs::prelude::PublicKeyCredential;

use crate::auth::{
    gen_token, hash_token, load_passkeys, note_passkey_used, session_cookie, stash_auth_challenge,
    take_auth_challenge, CONFIRM_MINUTES,
};
use crate::error::{AppError, AppResult};
use crate::login_code::{self, Held, Purpose, Recipient, CODE_MINUTES};
use crate::state::{AppState, AuthAdmin, PasskeyCeremony, AUTH_COOKIE, SESSION_COOKIE};

// ── the layer ───────────────────────────────────────────────────────────────

/// Paths the layer never stops: the auth flow itself (which cannot require
/// the thing it produces), and a child asking for more time (not takeover
/// surface — it only creates a request a parent still answers).
fn exempt(path: &str) -> bool {
    path.starts_with("/api/auth/") || path == "/api/me/ask"
}

/// The sensitive corner, read **or** write. A passkey list tells an attacker
/// what to remove; a pairing token is standing parent access; a computer's
/// unlock code is the key to it and its recovery codes are the spares.
fn sensitive(path: &str) -> bool {
    path.starts_with("/api/me/passkeys")
        || path.starts_with("/api/me/telegram")
        || path.starts_with("/api/parent-tokens")
        // Re-pointing an OS login at another person re-keys who that login
        // signs in as, and an enroll token is standing device access.
        || path.ends_with("/assign-account")
        || path.ends_with("/enroll-token")
        // A VPN config is applied verbatim on the device and reshapes its
        // routing.
        || path.starts_with("/api/vpn-profiles")
        || path.ends_with("/vpn")
        || (path.starts_with("/api/devices/")
            && (path.ends_with("/unlock-code")
                || path.ends_with("/unlock-code/rotate")
                || path.ends_with("/recovery-codes")))
}

/// Layer over `/api`: sensitive routes need a live confirm window; a paused
/// account may read but never change anything.
pub async fn require_confirm(
    State(st): State<AppState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let path = req.uri().path().to_string();
    let mutating = !matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    );
    let needs_confirm = sensitive(&path) && !exempt(&path);
    let is_change = mutating && path.starts_with("/api/") && !exempt(&path);
    if !needs_confirm && !is_change {
        return Ok(next.run(req).await);
    }
    // No session is the handler's 401, not a 428: "you are not signed in" and
    // "prove it's you again" are different answers.
    let Some(cookie) = jar.get(SESSION_COOKIE) else {
        return Ok(next.run(req).await);
    };
    /// (confirm window, paused-at) for the session's account.
    type Gate = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);
    let row: Option<Gate> = sqlx::query_as(
        "SELECT s.stepup_until, a.blocked_at FROM admin_sessions s
         JOIN admins a ON a.id = s.admin_id
         WHERE (s.token_hash = $1
                OR (s.prev_token_hash = $1 AND s.prev_valid_until > now()))
           AND s.expires_at > now()",
    )
    .bind(hash_token(cookie.value()))
    .fetch_optional(&st.db)
    .await?;
    match row {
        None => Ok(next.run(req).await),
        Some((_, Some(_paused))) => Err(AppError::ForbiddenForMember(
            "this account is paused — a parent has to lift it first".into(),
        )),
        Some((until, None)) => {
            if needs_confirm && until.is_none_or(|t| t <= Utc::now()) {
                return Err(AppError::StepUpRequired(
                    "confirm it's you to touch the keys".into(),
                ));
            }
            Ok(next.run(req).await)
        }
    }
}

/// For a handler whose request is sensitive only for some bodies: the same
/// `428 step_up_required` the layer gives, unless the session's confirm
/// window is open.
pub async fn require_window(st: &AppState, jar: &CookieJar) -> AppResult<()> {
    let cookie = jar
        .get(SESSION_COOKIE)
        .ok_or_else(|| AppError::Unauthorized("no session".into()))?;
    let until: Option<Option<DateTime<Utc>>> = sqlx::query_scalar(
        "SELECT stepup_until FROM admin_sessions
         WHERE (token_hash = $1
                OR (prev_token_hash = $1 AND prev_valid_until > now()))
           AND expires_at > now()",
    )
    .bind(hash_token(cookie.value()))
    .fetch_optional(&st.db)
    .await?;
    match until {
        None => Err(AppError::Unauthorized("no session".into())),
        Some(t) if t.is_some_and(|t| t > Utc::now()) => Ok(()),
        Some(_) => Err(AppError::StepUpRequired(
            "confirm it's you to touch the keys".into(),
        )),
    }
}

// ── the window ──────────────────────────────────────────────────────────────

/// The session row behind the cookie (honouring the rotation grace).
async fn session_id_for(st: &AppState, jar: &CookieJar) -> AppResult<Uuid> {
    let cookie = jar
        .get(SESSION_COOKIE)
        .ok_or_else(|| AppError::Unauthorized("no session".into()))?;
    sqlx::query_scalar(
        "SELECT id FROM admin_sessions
         WHERE (token_hash = $1
                OR (prev_token_hash = $1 AND prev_valid_until > now()))
           AND expires_at > now()",
    )
    .bind(hash_token(cookie.value()))
    .fetch_optional(&st.db)
    .await?
    .ok_or_else(|| AppError::Unauthorized("no session".into()))
}

/// Open the session's confirm window and rotate its token while we're here —
/// the person just proved themselves, the natural moment to re-issue. The old
/// token stays valid for two minutes so an in-flight request or a second tab
/// isn't thrown out.
async fn open_window(
    st: &AppState,
    jar: CookieJar,
    session_id: Uuid,
) -> AppResult<(CookieJar, DateTime<Utc>)> {
    let until = Utc::now() + Duration::minutes(i64::from(CONFIRM_MINUTES));
    let fresh = gen_token();
    sqlx::query(
        "UPDATE admin_sessions
            SET prev_token_hash  = token_hash,
                prev_valid_until = now() + interval '2 minutes',
                token_hash       = $2,
                stepup_until     = $3,
                last_seen_at     = now(),
                expires_at       = GREATEST(expires_at, now() + interval '7 days')
          WHERE id = $1",
    )
    .bind(session_id)
    .bind(hash_token(&fresh))
    .bind(until)
    .execute(&st.db)
    .await?;
    Ok((jar.add(session_cookie(fresh, st.cookie_secure)), until))
}

/// `GET /api/auth/confirm` → whether the window is open, and which ways to
/// confirm this account has right now.
pub async fn status(
    State(st): State<AppState>,
    admin: AuthAdmin,
    jar: CookieJar,
) -> AppResult<Json<Value>> {
    let session_id = session_id_for(&st, &jar).await?;
    let until: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT stepup_until FROM admin_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&st.db)
            .await?;
    let passkeys: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webauthn_credentials WHERE admin_id = $1")
            .bind(admin.admin_id)
            .fetch_one(&st.db)
            .await?;
    let computers = login_code::code_targets(&st.db, admin.admin_id, admin.tenant_id).await?;
    Ok(Json(json!({
        "armed_until": until.filter(|t| *t > Utc::now()),
        "passkey": passkeys > 0,
        "computer": !computers.is_empty(),
    })))
}

/// `POST /api/auth/confirm/passkey/start` — an assertion challenge over this
/// account's own passkeys.
pub async fn passkey_start(
    State(st): State<AppState>,
    admin: AuthAdmin,
    jar: CookieJar,
) -> AppResult<(CookieJar, Json<Value>)> {
    let passkeys = load_passkeys(&st.db, admin.admin_id).await?;
    if passkeys.is_empty() {
        return Err(AppError::Conflict(
            "there's no passkey on this account".into(),
        ));
    }
    let (rcr, auth) = st
        .webauthn
        .start_passkey_authentication(&passkeys)
        .map_err(|e| AppError::BadRequest(format!("webauthn: {e}")))?;
    let jar = stash_auth_challenge(
        &st,
        jar,
        PasskeyCeremony::Confirm {
            admin_id: admin.admin_id,
            auth,
        },
    )
    .await;
    Ok((jar, Json(serde_json::to_value(rcr).unwrap())))
}

#[derive(Deserialize)]
pub struct PasskeyFinishReq {
    pub credential: PublicKeyCredential,
}

/// `POST /api/auth/confirm/passkey/finish`.
pub async fn passkey_finish(
    State(st): State<AppState>,
    admin: AuthAdmin,
    jar: CookieJar,
    Json(req): Json<PasskeyFinishReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let PasskeyCeremony::Confirm { admin_id, auth } = take_auth_challenge(&st, &jar).await? else {
        return Err(AppError::BadRequest("no confirm in progress".into()));
    };
    if admin_id != admin.admin_id {
        return Err(AppError::BadRequest("no confirm in progress".into()));
    }
    let result = st
        .webauthn
        .finish_passkey_authentication(&req.credential, &auth)
        .map_err(|e| AppError::Unauthorized(format!("that passkey didn't check out ({e})")))?;
    note_passkey_used(&st.db, admin.admin_id, &result).await?;

    let session_id = session_id_for(&st, &jar).await?;
    let (jar, until) = open_window(&st, jar.remove(Cookie::from(AUTH_COOKIE)), session_id).await?;
    Ok((jar, Json(json!({ "armed_until": until }))))
}

/// `POST /api/auth/confirm/code/start` — a code on this person's own computer.
pub async fn code_start(
    State(st): State<AppState>,
    admin: AuthAdmin,
    jar: CookieJar,
) -> AppResult<Json<Value>> {
    let targets = login_code::code_targets(&st.db, admin.admin_id, admin.tenant_id).await?;
    if targets.is_empty() {
        return Err(AppError::Conflict(
            "none of your computers is online right now".into(),
        ));
    }
    let session_id = session_id_for(&st, &jar).await?;
    // The same caps as the sign-in door: five codes per ten minutes, and none
    // at all after the hour's wrong codes are spent.
    let (id, held) = login_code::issue(
        &st,
        Purpose::Confirm { session_id },
        Some(Recipient {
            account_id: admin.admin_id,
            tenant_id: admin.tenant_id,
            targets,
        }),
    )
    .await?;
    match held {
        None => {}
        Some(Held::TooOften) => {
            return Err(AppError::RateLimited(
                "your computer was asked for a code a lot just now — wait a few minutes, \
                 or use your passkey"
                    .into(),
            ))
        }
        Some(Held::TooManyWrong) => {
            return Err(AppError::RateLimited(
                "too many wrong codes this hour — use your passkey, or wait a while".into(),
            ))
        }
    }
    Ok(Json(json!({
        "request_id": id,
        "expires_in_secs": CODE_MINUTES * 60,
    })))
}

#[derive(Deserialize)]
pub struct CodeVerifyReq {
    #[serde(default)]
    pub request_id: Uuid,
    #[serde(default)]
    pub code: String,
}

/// `POST /api/auth/confirm/code/verify` — the code from your computer, typed
/// into the session that asked for it.
pub async fn code_verify(
    State(st): State<AppState>,
    admin: AuthAdmin,
    jar: CookieJar,
    Json(req): Json<CodeVerifyReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let session_id = session_id_for(&st, &jar).await?;
    let (_, account) = login_code::check(&st.db, req.request_id, &req.code, |_, s| {
        s == Some(session_id)
    })
    .await?;
    if account != admin.admin_id {
        return Err(login_code::Refusal::StartAgain.into());
    }
    let (jar, until) = open_window(&st, jar, session_id).await?;
    Ok((jar, Json(json!({ "armed_until": until }))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_auth_flow_itself_is_never_guarded() {
        // Otherwise you would need a confirm to get a confirm.
        assert!(exempt("/api/auth/confirm/code/verify"));
        assert!(exempt("/api/auth/confirm/passkey/finish"));
        assert!(exempt("/api/auth/login/finish"));
        assert!(exempt("/api/auth/code/verify"));
        assert!(exempt("/api/auth/voucher"));
        assert!(exempt("/api/me/ask"));
        assert!(!exempt("/api/devices"));
        assert!(!exempt("/api/something/invented/tomorrow"));
    }

    #[test]
    fn the_sensitive_corner_covers_reads_and_writes() {
        assert!(sensitive("/api/me/passkeys"));
        assert!(sensitive("/api/me/passkeys/new/start"));
        assert!(sensitive("/api/me/passkeys/abc"));
        assert!(sensitive("/api/me/telegram/pair"));
        assert!(sensitive("/api/parent-tokens"));
        assert!(sensitive("/api/devices/abc/unlock-code"));
        assert!(sensitive("/api/devices/abc/unlock-code/rotate"));
        assert!(sensitive("/api/devices/abc/recovery-codes"));
        assert!(sensitive("/api/device-users/abc/assign-account"));
        assert!(sensitive("/api/devices/abc/enroll-token"));
        assert!(sensitive("/api/vpn-profiles/abc"));
        // Everything else — reads and ordinary changes — stays out of it.
        // (`POST /api/devices` for a parent's own computer is checked by the
        // handler: `require_window`.)
        assert!(!sensitive("/api/devices"));
        assert!(!sensitive("/api/devices/abc/lock"));
        assert!(!sensitive("/api/device-users/abc/credit-time"));
        assert!(!sensitive("/api/auth/confirm"));
    }
}
