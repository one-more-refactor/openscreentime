//! OIDC SSO (tested against Authentik). Enabled only when all of
//! OST_OIDC_ISSUER / OST_OIDC_CLIENT_ID / OST_OIDC_CLIENT_SECRET
//! are set; provider endpoints are discovered at startup via
//! `<issuer>/.well-known/openid-configuration`.
//!
//! Flow: `GET /api/auth/oidc/start` 302s to the authorize URL with a random
//! `state` held in-memory (10-min TTL); `GET /api/auth/oidc/callback` exchanges
//! the code, fetches userinfo, matches the verified email against existing
//! admins (fresh installs bootstrap a tenant + admin) and issues a session.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Query, State},
    response::Redirect,
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{create_session, create_tenant_with_admin, gen_token, session_cookie};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// `state` parameters expire after this.
const STATE_TTL: Duration = Duration::from_secs(600);

/// A first-run SSO signup, held after the identity is verified but before the
/// account exists, expires after this — long enough to pick a name, short
/// enough that a stale link is useless.
const SIGNUP_TTL: Duration = Duration::from_secs(1800);

/// Cookie that binds the OIDC `state` to the browser that started the flow.
/// Without it, `state` existing server-side is not proof the SAME browser began
/// the login, which enables login-CSRF / session fixation (an attacker primes a
/// state+code for their own account and makes the victim complete the callback).
const OIDC_STATE_COOKIE: &str = "oidc_state";

/// Short-lived, HttpOnly cookie carrying the in-flight `state`. SameSite=Lax so
/// it still rides the top-level GET redirect back from the IdP.
fn state_cookie(value: String, secure: bool) -> Cookie<'static> {
    Cookie::build((OIDC_STATE_COOKIE, value))
        .path("/api/auth/oidc")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .build()
}

struct PendingState {
    created: Instant,
    /// Path (relative to the public URL) to send the browser to after login.
    redirect_to: String,
}

/// A verified first-run SSO identity, parked while the person chooses their
/// username. The account is not created — and no session issued — until they
/// finish, so a half-finished SSO login leaves nothing behind.
struct PendingSignup {
    created: Instant,
    /// The IdP-verified email, kept to stamp on the account they create.
    email: String,
    /// A friendly starting point for the username field (email local-part).
    suggested_username: String,
    /// The IdP `name` claim (or the local-part), for the display name.
    suggested_name: String,
}

/// Discovered provider config + in-flight `state` store.
pub struct Oidc {
    pub name: String,
    client_id: String,
    client_secret: String,
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
    redirect_uri: String,
    http: reqwest::Client,
    states: tokio::sync::Mutex<HashMap<String, PendingState>>,
    /// First-run signups awaiting a chosen username (keyed by an opaque token
    /// carried in the /welcome URL — never anything sensitive).
    pending_signups: tokio::sync::Mutex<HashMap<String, PendingSignup>>,
}

#[derive(Deserialize)]
struct DiscoveryDoc {
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
}

/// Reads the OST_OIDC_* env vars; when all three are set, runs discovery
/// and returns a live config. Returns None when the feature is off.
pub async fn init_from_env(public_url: &str) -> anyhow::Result<Option<Arc<Oidc>>> {
    // `configured` (state.rs) is what decides a variable was really set —
    // empty strings and unexpanded compose placeholders both count as unset,
    // which is what keeps a no-OIDC deploy from crash-looping on discovery.
    let non_empty = crate::state::configured;
    let (Some(issuer), Some(client_id), Some(client_secret)) = (
        non_empty("OST_OIDC_ISSUER"),
        non_empty("OST_OIDC_CLIENT_ID"),
        non_empty("OST_OIDC_CLIENT_SECRET"),
    ) else {
        return Ok(None);
    };
    let name = non_empty("OST_OIDC_NAME").unwrap_or_else(|| "SSO".into());

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let discovery_url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let doc: DiscoveryDoc = http
        .get(&discovery_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| anyhow::anyhow!("OIDC discovery at {discovery_url} failed: {e}"))?
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("OIDC discovery document invalid: {e}"))?;

    tracing::info!(issuer, "OIDC SSO enabled");
    Ok(Some(Arc::new(Oidc {
        name,
        client_id,
        client_secret,
        authorization_endpoint: doc.authorization_endpoint,
        token_endpoint: doc.token_endpoint,
        userinfo_endpoint: doc.userinfo_endpoint,
        redirect_uri: format!("{public_url}/api/auth/oidc/callback"),
        http,
        states: tokio::sync::Mutex::new(HashMap::new()),
        pending_signups: tokio::sync::Mutex::new(HashMap::new()),
    })))
}

impl Oidc {
    async fn issue_state(&self, redirect_to: String) -> String {
        let token = gen_token();
        let mut states = self.states.lock().await;
        states.retain(|_, s| s.created.elapsed() < STATE_TTL);
        states.insert(
            token.clone(),
            PendingState {
                created: Instant::now(),
                redirect_to,
            },
        );
        token
    }

    async fn take_state(&self, token: &str) -> Option<String> {
        let mut states = self.states.lock().await;
        states.retain(|_, s| s.created.elapsed() < STATE_TTL);
        states.remove(token).map(|s| s.redirect_to)
    }

    async fn exchange_code(&self, code: &str) -> reqwest::Result<TokenResponse> {
        self.http
            .post(&self.token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }

    async fn fetch_userinfo(&self, access_token: &str) -> reqwest::Result<UserInfo> {
        self.http
            .get(&self.userinfo_endpoint)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }
}

/// GET /api/auth/config — public; tells the entry page whether SSO exists and
/// whether this is a fresh install (no account yet) so it can show the
/// first-run registration flow instead of login.
pub async fn auth_config(State(st): State<AppState>) -> Json<Value> {
    let (enabled, name) = match &st.oidc {
        Some(o) => (true, o.name.clone()),
        None => (false, "SSO".to_string()),
    };
    // needs_setup: no admin exists yet → the console should show registration.
    let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM admins")
        .fetch_one(&st.db)
        .await
        .unwrap_or(1);
    Json(json!({
        "needs_setup": admins == 0,
        "auth": { "oidc": enabled, "oidc_name": name },
    }))
}

/// GET /api/auth/oidc/start — 302 to the provider's authorize URL.
pub async fn start(State(st): State<AppState>, jar: CookieJar) -> AppResult<(CookieJar, Redirect)> {
    let oidc = st
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotFound("sso is not configured".into()))?;
    let state = oidc.issue_state("/".to_string()).await;

    let mut url = url::Url::parse(&oidc.authorization_endpoint)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("bad authorization endpoint: {e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &oidc.client_id)
        .append_pair("redirect_uri", &oidc.redirect_uri)
        .append_pair("scope", "openid email profile")
        .append_pair("state", &state);
    // Bind the flow to this browser: the callback requires this cookie to equal
    // the returned `state`.
    let jar = jar.add(state_cookie(state, st.cookie_secure));
    Ok((jar, Redirect::temporary(url.as_str())))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct UserInfo {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    name: Option<String>,
}

/// GET /api/auth/oidc/callback — code exchange + userinfo + session.
///
/// Failures redirect back to the login page (`?error=sso_failed`, or
/// `?error=sso_unknown_account` for an email no admin owns) instead of
/// surfacing a JSON error to a mid-redirect browser.
pub async fn callback(
    State(st): State<AppState>,
    jar: CookieJar,
    Query(q): Query<CallbackQuery>,
) -> AppResult<(CookieJar, Redirect)> {
    let oidc = st
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotFound("sso is not configured".into()))?;
    let fail = |jar: CookieJar, code: &str| {
        let to = format!("{}/login?error={code}", st.public_url);
        (jar, Redirect::temporary(&to))
    };

    // The state bound to THIS browser in `start`. Clear it on every path out.
    let cookie_state = jar.get(OIDC_STATE_COOKIE).map(|c| c.value().to_string());
    let jar = jar.remove(state_cookie(String::new(), st.cookie_secure));

    let (Some(code), Some(state)) = (q.code, q.state) else {
        return Ok(fail(jar, "sso_failed"));
    };
    // CSRF / login-fixation guard: the returned `state` must match the cookie
    // set when this browser began the flow. A cross-site forced callback (with
    // an attacker's code+state) won't carry the matching cookie.
    if cookie_state.as_deref() != Some(state.as_str()) {
        tracing::warn!("oidc callback state does not match the browser cookie");
        return Ok(fail(jar, "sso_failed"));
    }
    let Some(redirect_to) = oidc.take_state(&state).await else {
        return Ok(fail(jar, "sso_failed"));
    };

    // Exchange the code (client_secret in the POST body, per Authentik).
    let token = match oidc.exchange_code(&code).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "oidc code exchange failed");
            return Ok(fail(jar, "sso_failed"));
        }
    };

    let info = match oidc.fetch_userinfo(&token.access_token).await {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(error = %e, "oidc userinfo fetch failed");
            return Ok(fail(jar, "sso_failed"));
        }
    };

    let Some(email) = info.email.filter(|e| !e.trim().is_empty()) else {
        return Ok(fail(jar, "sso_failed"));
    };
    // Require a positively verified email. An IdP that omits the claim entirely
    // (email_verified: None) must NOT be trusted to prove ownership — otherwise a
    // user who can set an arbitrary unverified email at the IdP could claim the
    // admin's address and take over the account.
    if info.email_verified != Some(true) {
        return Ok(fail(jar, "sso_failed"));
    }

    // SSO keys the verified email as the account username (identity moved off the
    // email column, which is being retired). Match case-insensitively.
    let existing: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, tenant_id FROM admins
              WHERE lower(username) = lower($1) OR lower(email) = lower($1)",
    )
    .bind(&email)
    .fetch_optional(&st.db)
    .await?;

    let (admin_id, tenant_id) = match existing {
        Some((admin_id, tenant_id)) => (admin_id, tenant_id),
        None => {
            let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM admins")
                .fetch_one(&st.db)
                .await?;
            if admins > 0 {
                // Family server: no auto-provisioning of extra admins.
                return Ok(fail(jar, "sso_unknown_account"));
            }
            // Fresh install: the identity is verified, but the account is not
            // created yet — the first parent gets to CHOOSE their username (the
            // same as the passkey path), instead of one derived from their
            // email. Park the identity and send them to the name-choosing page;
            // no session is issued until they finish.
            let local = email.split('@').next().unwrap_or("user");
            let suggested_username = crate::auth::normalize_username(local)
                .unwrap_or_else(|_| format!("user-{}", &gen_token()[..8]));
            let suggested_name = info
                .name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| local.to_string());
            let token = gen_token();
            {
                let mut pend = oidc.pending_signups.lock().await;
                pend.retain(|_, s| s.created.elapsed() < SIGNUP_TTL);
                pend.insert(
                    token.clone(),
                    PendingSignup {
                        created: Instant::now(),
                        email: email.clone(),
                        suggested_username,
                        suggested_name,
                    },
                );
            }
            let to = format!("{}/welcome?setup={token}", st.public_url);
            return Ok((jar, Redirect::temporary(&to)));
        }
    };

    let sid = create_session(&st.db, admin_id, tenant_id).await?;
    let jar = jar.add(session_cookie(sid, st.cookie_secure));
    let to = format!("{}{redirect_to}", st.public_url);
    Ok((jar, Redirect::temporary(&to)))
}

#[derive(serde::Serialize)]
pub struct SetupInfo {
    email: String,
    suggested_username: String,
    suggested_name: String,
}

/// GET /api/auth/oidc/setup/:token — the parked first-run identity behind a
/// /welcome link, so the page can greet them and pre-fill the name. 404 once it
/// has expired or been used (they simply sign in again).
pub async fn setup_info(
    State(st): State<AppState>,
    axum::extract::Path(token): axum::extract::Path<String>,
) -> AppResult<Json<SetupInfo>> {
    let oidc = st
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotFound("sso is not configured".into()))?;
    let mut pend = oidc.pending_signups.lock().await;
    pend.retain(|_, s| s.created.elapsed() < SIGNUP_TTL);
    let s = pend
        .get(&token)
        .ok_or_else(|| AppError::NotFound("this setup link has expired — sign in again".into()))?;
    Ok(Json(SetupInfo {
        email: s.email.clone(),
        suggested_username: s.suggested_username.clone(),
        suggested_name: s.suggested_name.clone(),
    }))
}

#[derive(Deserialize)]
pub struct SetupFinishReq {
    username: String,
    #[serde(default)]
    display_name: Option<String>,
}

/// POST /api/auth/oidc/setup/:token — create the first-run account with the
/// chosen username, stamp the verified email, and sign them in. Consumes the
/// token. `create_tenant_with_admin(require_first = true)` still guards the
/// zero-admin race, so a second concurrent finisher is refused.
pub async fn setup_finish(
    State(st): State<AppState>,
    jar: CookieJar,
    axum::extract::Path(token): axum::extract::Path<String>,
    Json(req): Json<SetupFinishReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let oidc = st
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotFound("sso is not configured".into()))?;

    let username = crate::auth::normalize_username(&req.username)?;

    // Take the parked identity out (single-use) only once the username validates,
    // so a bad name lets them try again rather than burning the link.
    let pending = {
        let mut pend = oidc.pending_signups.lock().await;
        pend.retain(|_, s| s.created.elapsed() < SIGNUP_TTL);
        if !pend.contains_key(&token) {
            return Err(AppError::NotFound(
                "this setup link has expired — sign in again".into(),
            ));
        }
        pend.remove(&token).unwrap()
    };

    let display_name = req
        .display_name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or(pending.suggested_name);

    let (tenant_id, admin_id) =
        create_tenant_with_admin(&st.db, &username, &display_name, true).await?;
    sqlx::query("UPDATE admins SET email = $1 WHERE id = $2")
        .bind(&pending.email)
        .bind(admin_id)
        .execute(&st.db)
        .await?;

    let sid = create_session(&st.db, admin_id, tenant_id).await?;
    let jar = jar.add(session_cookie(sid, st.cookie_secure));
    Ok((jar, Json(json!({ "ok": true }))))
}
