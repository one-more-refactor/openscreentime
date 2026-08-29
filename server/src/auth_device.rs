//! Client-first login — "install the client once, never touch a terminal
//! again" (CONTRACT-0.6 §2).
//!
//! The browser asks by **name**; the person's own computer answers. Flow:
//!
//! 1. The browser mints a random `code_verifier`, keeps it, and POSTs
//!    `{username, code_challenge = b64url(sha256(verifier))}` to `start`.
//! 2. The server finds the account behind the name, and pushes a
//!    `login_approve` command to every **online** device that person uses.
//!    The agent shows a small approval prompt to exactly that person's OS
//!    login(s) (tray notification / popup).
//! 3. The agent reports the human's decision to `/agent/login-decision`.
//! 4. The browser polls `finish` with its verifier; on an approved request
//!    whose challenge matches, it gets a **trusted** session.
//!
//! Why PKCE-style: the request id travels through logs and the agent; the
//! verifier never leaves the browser that started the flow, so an approval
//! can't be redeemed by anyone who merely saw the id.
//!
//! Name matching is deliberately homely: a display name or an OS username,
//! case-insensitive. On a family server that is unambiguous; when it isn't
//! (two "Alex"es across tenants), the answer is "use your passkey", not a
//! guessing game.

use axum::{extract::State, Json};
use axum_extra::extract::cookie::CookieJar;
use base64::Engine;
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Digest;
use uuid::Uuid;

use crate::agent::enqueue_command_delivered;
use crate::auth::{create_session, session_cookie};
use crate::error::{AppError, AppResult};
use crate::state::{AgentAuth, AppState};

/// How long an approval request lives. Long enough to walk to the machine's
/// tray; short enough that a stale prompt can't be honored much later.
const REQUEST_MINUTES: i64 = 2;

pub const CMD_LOGIN_APPROVE: &str = "login_approve";

fn challenge_of(verifier: &str) -> String {
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// The 4-digit code shown in the BROWSER only. The approving device shows this
/// code among two decoys and the human taps the one their browser shows
/// (number-matching, like Google's sign-in prompt). A cold-call attacker who
/// triggers the prompt for a name they know never sees the victim's browser,
/// so they cannot tell the victim which number to tap — a stray tap is 1-in-3,
/// rate-limited, and alerted. (The earlier "show the same code on both" design
/// only defended a victim-initiated login, not this unsolicited case.)
fn gen_match_code() -> String {
    let n: u16 = rand::thread_rng().gen_range(0..10_000);
    format!("{n:04}")
}

/// The real code plus two distinct decoys, shuffled — what the device shows.
fn code_choices(real: &str) -> Vec<String> {
    let mut rng = rand::thread_rng();
    let mut out = vec![real.to_string()];
    while out.len() < 3 {
        let c = format!("{:04}", rng.gen_range(0..10_000));
        if !out.contains(&c) {
            out.push(c);
        }
    }
    // Fisher-Yates on 3 elements.
    for i in (1..out.len()).rev() {
        out.swap(i, rng.gen_range(0..=i));
    }
    out
}

#[derive(Deserialize)]
pub struct StartReq {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub code_challenge: String,
}

/// `POST /api/auth/device/start` — name in, approval prompts out.
pub async fn start(
    State(st): State<AppState>,
    Json(req): Json<StartReq>,
) -> AppResult<Json<Value>> {
    let name = req.username.trim();
    if name.is_empty() || req.code_challenge.len() < 20 {
        return Err(AppError::BadRequest("who is signing in?".into()));
    }

    // The match code is generated for EVERY request, real or not, so the
    // response shape and timing don't distinguish a known name from an
    // unknown one (the login page must not be a username/device oracle).
    let match_code = gen_match_code();
    let uniform = |code: &str| {
        // A decoy request_id that indexes no row: the browser polls, gets
        // "pending" until the window closes, then "nobody approved". Identical
        // to a real request that no one approves.
        Json(json!({
            "request_id": Uuid::new_v4(),
            "match_code": code,
            "expires_in_secs": REQUEST_MINUTES * 60,
        }))
    };

    // The account behind the name: a display name, or an OS login on an
    // enrolled device. Distinct accounts; ambiguity resolves to a decoy (never
    // a distinguishable "which household are you in" answer).
    let accounts: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT DISTINCT a.id, a.tenant_id, a.display_name
           FROM admins a
           LEFT JOIN device_users du ON du.account_id = a.id
          WHERE (lower(a.username) = lower($1)
              OR lower(a.display_name) = lower($1)
              OR lower(du.os_username) = lower($1))
            AND a.blocked_at IS NULL",
    )
    .bind(name)
    .fetch_all(&st.db)
    .await?;

    let (account_id, tenant_id, display_name) = match accounts.as_slice() {
        [one] => one.clone(),
        // Unknown or ambiguous → a decoy that never approves. Uniform.
        _ => return Ok(uniform(&match_code)),
    };

    // Every online device that person actually uses, with the OS logins that
    // are theirs on it — the agent prompts only those sessions.
    let targets: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT d.id, du.os_username
           FROM device_users du
           JOIN devices d ON d.id = du.device_id
          WHERE du.account_id = $1 AND d.tenant_id = $2 AND d.status = 'online'",
    )
    .bind(account_id)
    .bind(tenant_id)
    .fetch_all(&st.db)
    .await?;

    if targets.is_empty() {
        // No awake device → a decoy, same shape. The browser's own copy tells
        // the human to use their passkey when nothing answers.
        return Ok(uniform(&match_code));
    }

    let request_id: Uuid = sqlx::query_scalar(
        "INSERT INTO login_requests (tenant_id, account_id, code_challenge, match_code, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(mins => $5))
        RETURNING id",
    )
    .bind(tenant_id)
    .bind(account_id)
    .bind(&req.code_challenge)
    .bind(&match_code)
    .bind(REQUEST_MINUTES as i32)
    .fetch_one(&st.db)
    .await?;
    let _ = sqlx::query("DELETE FROM login_requests WHERE expires_at < now() - interval '1 hour'")
        .execute(&st.db)
        .await;

    // Group OS logins per device and push one command each. The prompt carries
    // the match code; device names are NOT returned to the caller.
    let mut by_device: std::collections::HashMap<Uuid, Vec<String>> =
        std::collections::HashMap::new();
    for (device_id, os_user) in targets {
        by_device.entry(device_id).or_default().push(os_user);
    }
    for (device_id, os_users) in by_device {
        let _ = enqueue_command_delivered(
            &st,
            device_id,
            CMD_LOGIN_APPROVE,
            json!({
                "request_id": request_id,
                "username": display_name,
                "os_users": os_users,
                // The device shows these three; the browser shows the real one.
                // The device is NOT told which is real.
                "codes": code_choices(&match_code),
                "expires_in_secs": REQUEST_MINUTES * 60,
            }),
        )
        .await;
    }

    Ok(Json(json!({
        "request_id": request_id,
        "match_code": match_code,
        "expires_in_secs": REQUEST_MINUTES * 60,
    })))
}

#[derive(Deserialize)]
pub struct FinishReq {
    #[serde(default)]
    pub request_id: Uuid,
    #[serde(default)]
    pub code_verifier: String,
}

/// `POST /api/auth/device/finish` — the browser's poll. Pending answers 202-
/// style (`{status:"pending"}`), approval + matching verifier mints a
/// **trusted** session (the approval WAS the login ceremony).
pub async fn finish(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<FinishReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let row: Option<(Uuid, Uuid, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT tenant_id, account_id, code_challenge, status, expires_at
           FROM login_requests WHERE id = $1",
    )
    .bind(req.request_id)
    .fetch_optional(&st.db)
    .await?;
    let Some((tenant_id, account_id, challenge, status, expires_at)) = row else {
        return Err(AppError::NotFound("that sign-in request is gone".into()));
    };

    if expires_at < Utc::now() {
        let _ = sqlx::query("DELETE FROM login_requests WHERE id = $1")
            .bind(req.request_id)
            .execute(&st.db)
            .await;
        return Err(AppError::Unauthorized(
            "nobody approved in time — try again".into(),
        ));
    }
    match status.as_str() {
        "pending" => return Ok((jar, Json(json!({ "status": "pending" })))),
        "denied" => {
            let _ = sqlx::query("DELETE FROM login_requests WHERE id = $1")
                .bind(req.request_id)
                .execute(&st.db)
                .await;
            return Err(AppError::Unauthorized(
                "the computer said no to this sign-in".into(),
            ));
        }
        _ => {}
    }

    // Approved: the verifier must hash to the stored challenge.
    if challenge_of(req.code_verifier.trim()) != challenge {
        return Err(AppError::Unauthorized("that sign-in isn't yours".into()));
    }
    // Single-use, atomically: exactly one concurrent finish() wins the row and
    // mints a session; the rest see zero rows and fail. Deletes-after-select
    // let two polls both mint a session from one approval.
    let consumed = sqlx::query("DELETE FROM login_requests WHERE id = $1 AND status = 'approved'")
        .bind(req.request_id)
        .execute(&st.db)
        .await?
        .rows_affected();
    if consumed == 0 {
        return Err(AppError::Unauthorized(
            "that sign-in was already used".into(),
        ));
    }

    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM admins WHERE id = $1 AND tenant_id = $2")
            .bind(account_id)
            .bind(tenant_id)
            .fetch_optional(&st.db)
            .await?;
    let role = role.ok_or_else(|| AppError::Unauthorized("no such account any more".into()))?;

    let token = create_session(&st.db, account_id, tenant_id).await?;

    // A new session on your account is security-relevant: phone it out (the
    // alert worker drains critical events), so a takeover can never be silent.
    let display: Option<String> =
        sqlx::query_scalar("SELECT display_name FROM admins WHERE id = $1")
            .bind(account_id)
            .fetch_optional(&st.db)
            .await?;
    let _ = crate::events::insert(
        &st.db,
        tenant_id,
        None,
        None,
        "account_login",
        "critical",
        json!({
            "message": format!(
                "New web sign-in as {} — if this wasn't you, change your passkey.",
                display.unwrap_or_else(|| "someone".into())
            ),
            "account_id": account_id,
            "via": "device_approval",
        }),
    )
    .await;

    Ok((
        jar.add(session_cookie(token, st.cookie_secure)),
        Json(json!({ "status": "approved", "role": role })),
    ))
}

#[derive(Deserialize)]
pub struct DecisionReq {
    #[serde(default)]
    pub request_id: Uuid,
    /// The number the human tapped (number-matching). `None`/empty = "not me".
    /// Approval happens only if this equals the request's real `match_code`.
    #[serde(default)]
    pub code: Option<String>,
    /// Which OS login answered (for the audit trail).
    #[serde(default)]
    pub os_username: String,
}

/// `POST /agent/login-decision` — the human at the machine answered. Only a
/// device the target account actually uses may answer, and only once. The
/// server — not the device — decides approve vs deny by matching the tapped
/// code against the real one, so a device that guessed wrong (or a cold-call
/// victim who tapped a random number) is denied.
pub async fn decision(
    State(st): State<AppState>,
    agent: AgentAuth,
    Json(req): Json<DecisionReq>,
) -> AppResult<Json<Value>> {
    let tapped = req.code.as_deref().unwrap_or("").trim().to_string();
    let updated: Option<(Uuid, bool)> = sqlx::query_as(
        "UPDATE login_requests lr
            SET status = CASE WHEN lr.match_code = $3 AND $3 <> '' THEN 'approved' ELSE 'denied' END,
                approved_device_id = $2
          WHERE lr.id = $1 AND lr.status = 'pending' AND lr.expires_at > now()
            AND lr.tenant_id = $4
            AND EXISTS (SELECT 1 FROM device_users du
                         WHERE du.device_id = $2 AND du.account_id = lr.account_id)
        RETURNING lr.id, (lr.match_code = $3 AND $3 <> '')",
    )
    .bind(req.request_id)
    .bind(agent.device_id)
    .bind(&tapped)
    .bind(agent.tenant_id)
    .fetch_optional(&st.db)
    .await?;

    let Some((_, approved)) = updated else {
        return Err(AppError::NotFound(
            "that sign-in request is gone or not this device's to answer".into(),
        ));
    };

    crate::events::insert(
        &st.db,
        agent.tenant_id,
        Some(agent.device_id),
        None,
        "login_approval",
        "info",
        json!({
            "request_id": req.request_id,
            "approved": approved,
            "by_os_user": req.os_username,
        }),
    )
    .await?;

    Ok(Json(json!({ "ok": true, "approved": approved })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_rfc7636_s256() {
        // RFC 7636 appendix B test vector.
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn match_code_is_four_digits() {
        for _ in 0..50 {
            let c = gen_match_code();
            assert_eq!(c.len(), 4);
            assert!(c.chars().all(|ch| ch.is_ascii_digit()));
        }
    }

    #[test]
    fn code_choices_hold_the_real_code_among_distinct_decoys() {
        for _ in 0..100 {
            let real = gen_match_code();
            let cs = code_choices(&real);
            assert_eq!(cs.len(), 3);
            assert!(cs.contains(&real), "the real code must be offered");
            // All three distinct — no accidental duplicate that would make the
            // decoy trivially guessable.
            let mut uniq = cs.clone();
            uniq.sort();
            uniq.dedup();
            assert_eq!(uniq.len(), 3);
            assert!(cs
                .iter()
                .all(|c| c.len() == 4 && c.chars().all(|d| d.is_ascii_digit())));
        }
    }
}
