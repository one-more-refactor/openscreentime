//! Door one: **your name, then a code on your own computer.**
//!
//! 1. The browser keeps a random PKCE verifier and POSTs
//!    `{name, code_challenge = b64url(sha256(verifier))}` to `start`.
//! 2. The server finds the person behind the name and sends a fresh 6-digit
//!    code to their own computer(s) — only to the OS logins linked to that
//!    person, and for a parent only on a computer declared as theirs. The
//!    computer shows it in the app window, as a notification, and in
//!    `ost code`.
//! 3. The person types the code into the browser; `verify` checks it together
//!    with the verifier and signs that browser in.
//!
//! The same code machinery answers "confirm it's you" for a signed-in session
//! (`confirm.rs`), with the session instead of PKCE binding the code.
//!
//! **Nothing here tells a stranger whether a name exists.** An unknown or
//! ambiguous name, or a person with no computer online, gets a *decoy*: a real
//! row whose code went nowhere. Its answers — wrong code, too many tries,
//! expired — are word for word what a real request gives, and the work done
//! per request is the same (the pushes to the computer happen after the
//! response).
//!
//! Brute force: a 6-digit code, 5 tries per code, 5 minutes, and the auth
//! rate limit on both calls. The person's computer is also asked at most five
//! times in ten minutes (sign-in and confirm codes together, counted under a
//! per-person lock so parallel asks can't all slip under the cap), and after
//! ten wrong codes for one person in an hour their code door goes quiet —
//! decoys — for the rest of that hour, with one alert to the household.
//!
//! **The code is never readable from the database** by anything but the
//! agent it is for: a live socket gets it in the frame and the queue row
//! stays empty; a polling agent's row holds it only until that agent pulls
//! it, and every code row is wiped once acked, used up or expired
//! (`agent::enqueue_secret_command`, `scrub_commands`).

use axum::{extract::State, Json};
use axum_extra::extract::cookie::CookieJar;
use base64::Engine;
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Digest;
use uuid::Uuid;

use crate::agent::enqueue_secret_command;
use crate::auth::{constant_time_eq, create_session, hash_token, session_cookie};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// How long a code works.
pub const CODE_MINUTES: i32 = 5;
/// Wrong codes before the request is gone.
pub const MAX_ATTEMPTS: i32 = 5;
/// Codes a person's computer is asked to show per ten minutes — sign-in and
/// confirm codes together.
pub const MAX_RECENT_PER_ACCOUNT: i64 = 5;
/// Wrong codes typed for one person in an hour, across all their codes,
/// before their code door answers only with decoys until the hour is up.
pub const WRONG_PER_HOUR: i64 = 10;

pub const CMD_LOGIN_CODE: &str = "login_code";

/// What an agent says it understands (`devices.agent_features`) before a code
/// is sent to it. Agents from before sign-in codes say nothing, and would
/// only fail the command — so a code is never sent where nobody can see it.
pub const FEATURE: &str = "login_code";

/// Advisory-lock class for one person's code bookkeeping (count + insert, and
/// the one warning per incident); the second key is `hashtext(account id)`.
const CODE_LOCK_CLASS: i32 = 0x0C0DE;

/// Serialize code bookkeeping for one person until `tx` ends.
async fn lock_person(tx: &mut sqlx::PgConnection, account_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2::text))")
        .bind(CODE_LOCK_CLASS)
        .bind(account_id)
        .execute(tx)
        .await?;
    Ok(())
}

pub fn challenge_of(verifier: &str) -> String {
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn gen_code() -> String {
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32))
}

/// What is stored: bound to the request id, so equal codes on two requests
/// don't hash alike.
fn code_hash(id: Uuid, code: &str) -> String {
    hash_token(&format!("{id}:{code}"))
}

pub fn digits_only(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Why a code was refused. The browser acts on the difference (type again vs.
/// start again); a real request and a decoy produce the same ones.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    WrongCode,
    StartAgain,
}

impl From<Refusal> for AppError {
    fn from(r: Refusal) -> Self {
        match r {
            Refusal::WrongCode => AppError::WrongCode(
                "that code didn't match — check your computer and try again".into(),
            ),
            Refusal::StartAgain => {
                AppError::CodeExpired("that code has run out — ask for a new one".into())
            }
        }
    }
}

// ── who, and where ─────────────────────────────────────────────────────────

/// The person behind a typed name, case-insensitive: a login name (unique on
/// the server, so it wins outright), else a name or an OS login that points
/// at exactly one person. Unknown or ambiguous → `None`. Both lookups always
/// run, so a hit takes no less time than a miss.
async fn resolve_name(db: &sqlx::PgPool, name: &str) -> AppResult<Option<(Uuid, Uuid, String)>> {
    let by_login: Option<(Uuid, Uuid, String)> =
        sqlx::query_as("SELECT id, tenant_id, role FROM admins WHERE lower(username) = lower($1)")
            .bind(name)
            .fetch_optional(db)
            .await?;
    let by_name: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT DISTINCT a.id, a.tenant_id, a.role
           FROM admins a
           LEFT JOIN device_users du ON du.account_id = a.id
          WHERE lower(a.display_name) = lower($1)
             OR lower(du.os_username) = lower($1)",
    )
    .bind(name)
    .fetch_all(db)
    .await?;
    Ok(match (by_login, by_name.as_slice()) {
        (Some(one), _) => Some(one),
        (None, [one]) => Some(one.clone()),
        _ => None,
    })
}

/// The online computers where `account` may be shown a code, with the OS
/// logins that are theirs there, and whether that computer's agent can show
/// one ([`FEATURE`]). A member: any computer they use. A parent: only a
/// computer declared as theirs, and there only **the owner's login**
/// (`devices.owner_os_username`) — root on a child's laptop must not be able
/// to read a parent's code off a login it linked to the parent, and neither
/// must a child's login that an older server linked to the parent on the
/// parent's own computer. No owner login settled yet: no code there.
pub async fn all_code_targets(
    db: &sqlx::PgPool,
    account_id: Uuid,
    tenant_id: Uuid,
) -> AppResult<Vec<(Uuid, String, bool)>> {
    Ok(sqlx::query_as(
        "SELECT d.id, du.os_username, COALESCE($3 = ANY(d.agent_features), false)
           FROM device_users du
           JOIN devices d ON d.id = du.device_id
           JOIN admins a ON a.id = du.account_id
          WHERE du.account_id = $1 AND d.tenant_id = $2 AND d.status = 'online'
            AND (a.role = 'member'
                 OR (d.owner_account_id = a.id
                     AND lower(du.os_username) = lower(d.owner_os_username)))
          ORDER BY d.id, du.os_username",
    )
    .bind(account_id)
    .bind(tenant_id)
    .bind(FEATURE)
    .fetch_all(db)
    .await?)
}

/// [`all_code_targets`] whose agent can actually show a code.
pub async fn code_targets(
    db: &sqlx::PgPool,
    account_id: Uuid,
    tenant_id: Uuid,
) -> AppResult<Vec<(Uuid, String)>> {
    Ok(all_code_targets(db, account_id, tenant_id)
        .await?
        .into_iter()
        .filter(|t| t.2)
        .map(|(d, u, _)| (d, u))
        .collect())
}

/// What a new code row is for.
pub enum Purpose<'a> {
    /// A browser signing in, bound by its PKCE challenge.
    Login { code_challenge: &'a str },
    /// A signed-in session confirming it's them.
    Confirm { session_id: Uuid },
}

/// Who a code is for, and the (computer, OS login) pairs to show it on.
pub struct Recipient {
    pub account_id: Uuid,
    pub tenant_id: Uuid,
    pub targets: Vec<(Uuid, String)>,
}

/// Why a code for a real person went nowhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Their computer was asked [`MAX_RECENT_PER_ACCOUNT`] times in ten
    /// minutes.
    TooOften,
    /// [`WRONG_PER_HOUR`] wrong codes were typed for them this hour.
    TooManyWrong,
}

/// Store a code and send it. `to = None` makes a decoy: same row, same
/// shape, a code nobody will ever see. A real person over their caps gets a
/// decoy too, and the reason comes back. Returns the request id.
///
/// The count and the insert happen in one transaction under a per-person
/// advisory lock: sixty parallel asks for one name still send five codes.
/// Decoys take the same lock (on the nil id) and run the same queries, so a
/// real request costs what a decoy costs.
pub async fn issue(
    st: &AppState,
    purpose: Purpose<'_>,
    to: Option<Recipient>,
) -> AppResult<(Uuid, Option<Held>)> {
    let id = Uuid::new_v4();
    let code = gen_code();
    let (kind, challenge, session_id) = match purpose {
        Purpose::Login { code_challenge } => ("login", Some(code_challenge), None),
        Purpose::Confirm { session_id } => ("confirm", None, Some(session_id)),
    };
    let who = to.as_ref().map_or(Uuid::nil(), |r| r.account_id);

    let mut tx = st.db.begin().await?;
    lock_person(&mut tx, who).await?;
    // Rows outlive their use (marked `used_at`) until an hour past expiry, so
    // both windows see every ask and every wrong try, not just open ones.
    let (recent, wrong): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE created_at > now() - interval '10 minutes'),
                COALESCE(sum(attempts) FILTER (WHERE created_at > now() - interval '1 hour'), 0)
           FROM login_codes WHERE account_id = $1",
    )
    .bind(who)
    .fetch_one(&mut *tx)
    .await?;
    let held = match to {
        None => None,
        Some(_) if wrong >= WRONG_PER_HOUR => Some(Held::TooManyWrong),
        Some(_) if recent >= MAX_RECENT_PER_ACCOUNT => Some(Held::TooOften),
        Some(_) => None,
    };
    let to = to.filter(|_| held.is_none());
    let (tenant_id, account_id) = match &to {
        Some(r) => (Some(r.tenant_id), Some(r.account_id)),
        None => (None, None),
    };
    sqlx::query(
        "INSERT INTO login_codes (id, tenant_id, account_id, purpose, code_challenge, session_id,
                                  code_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(mins => $8))",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(account_id)
    .bind(kind)
    .bind(challenge)
    .bind(session_id)
    .bind(code_hash(id, &code))
    .bind(CODE_MINUTES)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let _ = sqlx::query("DELETE FROM login_codes WHERE expires_at < now() - interval '1 hour'")
        .execute(&st.db)
        .await;
    scrub_commands(&st.db).await;

    if let Some(Recipient {
        account_id,
        targets,
        ..
    }) = to
    {
        // After the response: a real request must not take measurably longer
        // than a decoy.
        let st = st.clone();
        tokio::spawn(async move {
            if let Err(e) = send_code(&st, id, account_id, kind, &code, targets).await {
                tracing::warn!(error = %e, "could not send a sign-in code");
            }
        });
    }
    Ok((id, held))
}

/// Take codes out of the command queue once nobody can use them: past their
/// five minutes, a code row is wiped and an undelivered one withdrawn (a
/// delivered one unacked for an hour is closed too — that ack isn't coming).
/// Best-effort; run on every new code and by the hourly sweep.
pub async fn scrub_commands(db: &sqlx::PgPool) {
    if let Err(e) = sqlx::query(
        "UPDATE commands
            SET payload = '{}'::jsonb,
                status = CASE WHEN status = 'queued'
                                OR (status = 'sent' AND created_at < now() - interval '1 hour')
                              THEN 'cancelled' ELSE status END
          WHERE type = 'login_code'
            AND created_at < now() - make_interval(mins => $1)
            AND (payload <> '{}'::jsonb OR status = 'queued'
                 OR (status = 'sent' AND created_at < now() - interval '1 hour'))",
    )
    .bind(CODE_MINUTES)
    .execute(db)
    .await
    {
        tracing::warn!(error = %e, "could not scrub old sign-in codes from the command queue");
    }
}

/// A code is spent (used, or out of tries): wipe it from whatever queue row
/// still holds it, and withdraw it where it was never delivered.
async fn scrub_request(db: &sqlx::PgPool, id: Uuid) {
    let _ = sqlx::query(
        "UPDATE commands
            SET payload = '{}'::jsonb,
                status = CASE WHEN status = 'queued' THEN 'cancelled' ELSE status END
          WHERE type = 'login_code' AND payload->>'request_id' = $1",
    )
    .bind(id.to_string())
    .execute(db)
    .await;
}

async fn send_code(
    st: &AppState,
    id: Uuid,
    account_id: Uuid,
    purpose: &str,
    code: &str,
    targets: Vec<(Uuid, String)>,
) -> AppResult<()> {
    let name: String = sqlx::query_scalar("SELECT display_name FROM admins WHERE id = $1")
        .bind(account_id)
        .fetch_one(&st.db)
        .await?;
    // The address the person should be typing it into — the device says so,
    // which is the defence against a look-alike page relaying the code.
    let site = st.public_url.clone();
    let mut by_device: std::collections::BTreeMap<Uuid, Vec<String>> = Default::default();
    for (device_id, os_user) in targets {
        by_device.entry(device_id).or_default().push(os_user);
    }
    for (device_id, os_users) in by_device {
        enqueue_secret_command(
            st,
            device_id,
            CMD_LOGIN_CODE,
            json!({
                "request_id": id,
                "name": name,
                "os_users": os_users,
                "code": code,
                "purpose": purpose,
                "site": site,
                "expires_in_secs": CODE_MINUTES * 60,
            }),
        )
        .await?;
    }
    Ok(())
}

/// Check a typed code against a stored row. `binding_ok` is the PKCE or
/// session check, decided by the caller; a failed binding costs a try like a
/// wrong code. On success the row is used up (exactly once, even under
/// concurrent calls) and its (tenant, account) returned.
pub async fn check(
    db: &sqlx::PgPool,
    id: Uuid,
    code: &str,
    binding_ok: impl FnOnce(Option<&str>, Option<Uuid>) -> bool,
) -> Result<(Uuid, Uuid), CheckError> {
    type Row = (
        Option<Uuid>,
        Option<Uuid>,
        Option<String>,
        Option<Uuid>,
        String,
        DateTime<Utc>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT tenant_id, account_id, code_challenge, session_id, code_hash, expires_at
           FROM login_codes WHERE id = $1 AND attempts < $2 AND used_at IS NULL",
    )
    .bind(id)
    .bind(MAX_ATTEMPTS)
    .fetch_optional(db)
    .await?;
    let Some((tenant, account, challenge, session, stored, expires)) = row else {
        return Err(Refusal::StartAgain.into());
    };
    if expires < Utc::now() {
        return Err(Refusal::StartAgain.into());
    }
    let code = digits_only(code);
    let matches =
        code.len() == 6 && constant_time_eq(code_hash(id, &code).as_bytes(), stored.as_bytes());
    let bound = binding_ok(challenge.as_deref(), session);

    match (matches && bound, tenant, account) {
        (true, Some(tenant), Some(account)) => {
            let consumed = sqlx::query(
                "UPDATE login_codes SET used_at = now()
                  WHERE id = $1 AND attempts < $2 AND used_at IS NULL",
            )
            .bind(id)
            .bind(MAX_ATTEMPTS)
            .execute(db)
            .await?
            .rows_affected();
            if consumed == 1 {
                scrub_request(db, id).await;
                Ok((tenant, account))
            } else {
                Err(Refusal::StartAgain.into())
            }
        }
        _ => {
            let tries: Option<i32> = sqlx::query_scalar(
                "UPDATE login_codes SET attempts = attempts + 1 WHERE id = $1 RETURNING attempts",
            )
            .bind(id)
            .fetch_optional(db)
            .await?;
            // A decoy does the same bookkeeping, on nobody: a wrong code must
            // cost a real request no more than a decoy.
            let (tenant, account) = (tenant.unwrap_or_default(), account.unwrap_or_default());
            if let Err(e) = note_wrong_code(db, tenant, account).await {
                tracing::warn!(error = %e, "could not count a wrong sign-in code");
            }
            if tries.is_none_or(|t| t >= MAX_ATTEMPTS) {
                scrub_request(db, id).await;
                Err(Refusal::StartAgain.into())
            } else {
                Err(Refusal::WrongCode.into())
            }
        }
    }
}

/// A wrong code was typed for `account`. On the one that uses up the hour's
/// budget ([`WRONG_PER_HOUR`]) the household hears about it — once per
/// incident: while the hour's warning stands, no second one.
async fn note_wrong_code(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    account_id: Uuid,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    lock_person(&mut tx, account_id).await?;
    let wrong: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(attempts), 0) FROM login_codes
          WHERE account_id = $1 AND created_at > now() - interval '1 hour'",
    )
    .bind(account_id)
    .fetch_one(&mut *tx)
    .await?;
    if wrong < WRONG_PER_HOUR {
        return Ok(());
    }
    // Server-written only (no device): an agent can push `account_login`
    // events too, and must not be able to pre-empt this one.
    let warned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events
                         WHERE tenant_id = $1 AND device_id IS NULL AND type = 'account_login'
                           AND payload->>'kind' = 'code_guessing'
                           AND payload->>'account_id' = $2
                           AND created_at > now() - interval '1 hour')",
    )
    .bind(tenant_id)
    .bind(account_id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    if warned {
        return Ok(());
    }
    let name: String = sqlx::query_scalar("SELECT display_name FROM admins WHERE id = $1")
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or_else(|| "someone".into());
    // Critical: the alert worker phones it out like any other sign-in alert.
    sqlx::query(
        "INSERT INTO events (tenant_id, type, severity, payload)
         VALUES ($1, 'account_login', 'critical', $2)",
    )
    .bind(tenant_id)
    .bind(json!({
        "message": format!(
            "Someone typed {WRONG_PER_HOUR} wrong sign-in codes for {name} within an hour. \
             Signing in with a code is off for {name} until the hour is up; a passkey \
             still works."
        ),
        "account_id": account_id,
        "kind": "code_guessing",
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// A refusal, or the database failing underneath it.
#[derive(Debug)]
pub enum CheckError {
    Refused(Refusal),
    Db(sqlx::Error),
}

impl From<Refusal> for CheckError {
    fn from(r: Refusal) -> Self {
        CheckError::Refused(r)
    }
}

impl From<sqlx::Error> for CheckError {
    fn from(e: sqlx::Error) -> Self {
        CheckError::Db(e)
    }
}

impl From<CheckError> for AppError {
    fn from(e: CheckError) -> Self {
        match e {
            CheckError::Refused(r) => r.into(),
            CheckError::Db(e) => e.into(),
        }
    }
}

// ── the sign-in door ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct StartReq {
    /// What the person typed: their name (or login name).
    #[serde(default, alias = "username")]
    pub name: String,
    #[serde(default)]
    pub code_challenge: String,
}

/// `POST /api/auth/code/start` — a name in, a code on that person's computer.
pub async fn start(
    State(st): State<AppState>,
    Json(req): Json<StartReq>,
) -> AppResult<Json<Value>> {
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 64 || req.code_challenge.len() < 20 {
        return Err(AppError::BadRequest("who is signing in?".into()));
    }

    let person = resolve_name(&st.db, name).await?;
    // The same queries run for every name; an unknown one looks up nobody.
    let (account_id, tenant_id) = person
        .as_ref()
        .map(|(a, t, _)| (*a, *t))
        .unwrap_or((Uuid::nil(), Uuid::nil()));
    let all = all_code_targets(&st.db, account_id, tenant_id).await?;
    let targets: Vec<(Uuid, String)> = all
        .iter()
        .filter(|t| t.2)
        .map(|(d, u, _)| (*d, u.clone()))
        .collect();
    if person.is_some() && targets.is_empty() && !all.is_empty() {
        // Their computer is online but its agent predates sign-in codes: it
        // would only fail the command. The browser gets the usual decoy; the
        // operator gets the reason.
        tracing::info!(
            account = %account_id,
            "a sign-in code was asked for, but none of their online computers runs an agent \
             that can show one (update the agent there) — answered with a decoy"
        );
    }

    let to = (person.is_some() && !targets.is_empty()).then_some(Recipient {
        account_id,
        tenant_id,
        targets,
    });
    let (id, held) = issue(
        &st,
        Purpose::Login {
            code_challenge: &req.code_challenge,
        },
        to,
    )
    .await?;
    if let Some(why) = held {
        tracing::info!(account = %account_id, ?why, "sign-in code held back — answered with a decoy");
    }
    Ok(Json(json!({
        "request_id": id,
        "expires_in_secs": CODE_MINUTES * 60,
    })))
}

#[derive(Deserialize)]
pub struct VerifyReq {
    #[serde(default)]
    pub request_id: Uuid,
    #[serde(default)]
    pub code_verifier: String,
    #[serde(default)]
    pub code: String,
}

/// `POST /api/auth/code/verify` — the typed code and the verifier in; a
/// session out.
pub async fn verify(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<VerifyReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let verifier = req.code_verifier.trim().to_string();
    let (tenant_id, account_id) = check(&st.db, req.request_id, &req.code, |challenge, _| {
        challenge
            .is_some_and(|c| constant_time_eq(challenge_of(&verifier).as_bytes(), c.as_bytes()))
    })
    .await?;

    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM admins WHERE id = $1 AND tenant_id = $2")
            .bind(account_id)
            .bind(tenant_id)
            .fetch_optional(&st.db)
            .await?;
    let role = role.ok_or_else(|| AppError::from(Refusal::StartAgain))?;
    let token = create_session(&st.db, account_id, tenant_id).await?;

    // A new session on your account is security-relevant: phone it out (the
    // alert worker drains critical events), so a takeover is never silent.
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
                "New web sign-in as {} with a code from their computer.",
                display.unwrap_or_else(|| "someone".into())
            ),
            "account_id": account_id,
            "via": "computer_code",
        }),
    )
    .await;

    Ok((
        jar.add(session_cookie(token, st.cookie_secure)),
        Json(json!({ "ok": true, "role": role })),
    ))
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
    fn codes_are_six_digits() {
        for _ in 0..200 {
            let c = gen_code();
            assert_eq!(c.len(), 6);
            assert!(c.chars().all(|ch| ch.is_ascii_digit()));
        }
    }

    #[test]
    fn the_stored_hash_is_bound_to_the_request() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert_ne!(code_hash(a, "123456"), code_hash(b, "123456"));
        assert_eq!(code_hash(a, "123456"), code_hash(a, "123456"));
    }

    #[test]
    fn people_type_spaces() {
        assert_eq!(digits_only(" 123 456 "), "123456");
    }
}
