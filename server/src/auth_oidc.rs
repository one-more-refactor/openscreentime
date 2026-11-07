//! OIDC SSO (tested against Authentik). Enabled only when all of
//! OST_OIDC_ISSUER / OST_OIDC_CLIENT_ID / OST_OIDC_CLIENT_SECRET
//! are set; provider endpoints are discovered via
//! `<issuer>/.well-known/openid-configuration` in the background, retried
//! until the provider answers. An IdP that is down (or boots slower than we do
//! after a power cut) only hides the SSO button meanwhile — it must never take
//! passkey login and the agent API down with it.
//!
//! Flow: `GET /api/auth/oidc/start` 302s to the authorize URL with a random
//! `state` held in-memory (10-min TTL); `GET /api/auth/oidc/callback` exchanges
//! the code (PKCE-bound), fetches userinfo and signs in the account bound to
//! the IdP's `(issuer, sub)`. The email claim is never an identity: an account
//! is bound by the first-run SSO signup, or by "Link SSO" started from inside a
//! signed-in, confirmed session (`/api/auth/oidc/start?link=1`). A fresh
//! install bootstraps a tenant + admin.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Query, State},
    response::Redirect,
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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
    /// PKCE verifier; its S256 challenge went out with the authorize request.
    pkce_verifier: String,
    /// Set when a signed-in, confirmed account started the flow to LINK this
    /// IdP identity to itself, rather than to sign in.
    link_admin: Option<Uuid>,
}

/// A verified first-run SSO identity, parked while the person chooses their
/// username. The account is not created — and no session issued — until they
/// finish, so a half-finished SSO login leaves nothing behind.
struct PendingSignup {
    created: Instant,
    /// The IdP-verified email, kept to stamp on the account they create.
    email: String,
    /// The IdP `sub`, bound (with the issuer) to the account they create.
    subject: String,
    /// A friendly starting point for the username field (email local-part).
    suggested_username: String,
    /// The IdP `name` claim (or the local-part), for the display name.
    suggested_name: String,
}

/// Provider config (endpoints once discovered) + in-flight `state` store.
pub struct Oidc {
    pub name: String,
    /// The configured issuer (no trailing slash): half of every SSO binding.
    issuer: String,
    client_id: String,
    client_secret: String,
    /// Filled by the background discovery; empty while the IdP is unreachable.
    endpoints: std::sync::OnceLock<DiscoveryDoc>,
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

/// Reads the OST_OIDC_* env vars; when all three are set, returns the config
/// and starts discovery in the background (retrying until the provider
/// answers). Returns None when the feature is off.
pub fn init_from_env(public_url: &str) -> anyhow::Result<Option<Arc<Oidc>>> {
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
    let oidc = Arc::new(Oidc {
        name,
        issuer: issuer.trim_end_matches('/').to_string(),
        client_id,
        client_secret,
        endpoints: std::sync::OnceLock::new(),
        redirect_uri: format!("{public_url}/api/auth/oidc/callback"),
        http,
        states: tokio::sync::Mutex::new(HashMap::new()),
        pending_signups: tokio::sync::Mutex::new(HashMap::new()),
    });
    let bg = oidc.clone();
    crate::supervise::spawn("oidc-discovery", move || {
        let (oidc, issuer) = (bg.clone(), issuer.clone());
        async move { oidc.discover_until_ready(&issuer).await }
    });
    Ok(Some(oidc))
}

#[cfg(test)]
impl Oidc {
    /// An SSO config with no provider behind it, holding one first-run
    /// identity parked the way `callback` parks it. Returns the /welcome token.
    pub(crate) async fn parked_for_test(email: &str) -> (Arc<Oidc>, String) {
        let oidc = Self::unconfigured_for_test();
        let token = gen_token();
        oidc.pending_signups.lock().await.insert(
            token.clone(),
            PendingSignup {
                created: Instant::now(),
                email: email.into(),
                subject: "sub-of-the-first-parent".into(),
                suggested_username: "someone".into(),
                suggested_name: "Someone".into(),
            },
        );
        (oidc, token)
    }

    /// An SSO config with no provider behind it (issuer `https://idp.test`).
    pub(crate) fn unconfigured_for_test() -> Arc<Oidc> {
        Arc::new(Oidc {
            name: "SSO".into(),
            issuer: "https://idp.test".into(),
            client_id: "test".into(),
            client_secret: "test".into(),
            endpoints: std::sync::OnceLock::new(),
            redirect_uri: "http://localhost/api/auth/oidc/callback".into(),
            http: reqwest::Client::new(),
            states: tokio::sync::Mutex::new(HashMap::new()),
            pending_signups: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn issuer(&self) -> &str {
        &self.issuer
    }
}

impl Oidc {
    /// Discovery ran and the SSO button can be offered.
    pub fn ready(&self) -> bool {
        self.endpoints.get().is_some()
    }

    fn endpoints(&self) -> anyhow::Result<&DiscoveryDoc> {
        self.endpoints
            .get()
            .ok_or_else(|| anyhow::anyhow!("OIDC provider not discovered yet"))
    }

    /// Fetch the discovery document, retrying with backoff (capped at five
    /// minutes) until the provider answers.
    async fn discover_until_ready(&self, issuer: &str) {
        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let mut delay = Duration::from_secs(5);
        loop {
            let doc = async {
                self.http
                    .get(&url)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<DiscoveryDoc>()
                    .await
            }
            .await;
            match doc {
                Ok(doc) => {
                    let _ = self.endpoints.set(doc);
                    tracing::info!(issuer, "OIDC SSO enabled");
                    return;
                }
                Err(e) => {
                    tracing::warn!(
                        issuer,
                        error = %e,
                        retry_in_secs = delay.as_secs(),
                        "OIDC discovery failed; SSO stays hidden until it succeeds"
                    );
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(300));
        }
    }

    /// Returns `(state, pkce_challenge)`.
    async fn issue_state(&self, redirect_to: String, link_admin: Option<Uuid>) -> (String, String) {
        let token = gen_token();
        let pkce_verifier = gen_token();
        let challenge = pkce_challenge(&pkce_verifier);
        let mut states = self.states.lock().await;
        states.retain(|_, s| s.created.elapsed() < STATE_TTL);
        states.insert(
            token.clone(),
            PendingState {
                created: Instant::now(),
                redirect_to,
                pkce_verifier,
                link_admin,
            },
        );
        (token, challenge)
    }

    async fn take_state(&self, token: &str) -> Option<PendingState> {
        let mut states = self.states.lock().await;
        states.retain(|_, s| s.created.elapsed() < STATE_TTL);
        states.remove(token)
    }

    async fn exchange_code(
        &self,
        code: &str,
        pkce_verifier: &str,
    ) -> anyhow::Result<TokenResponse> {
        Ok(self
            .http
            .post(&self.endpoints()?.token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code_verifier", pkce_verifier),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn fetch_userinfo(&self, access_token: &str) -> anyhow::Result<UserInfo> {
        Ok(self
            .http
            .get(&self.endpoints()?.userinfo_endpoint)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

/// GET /api/auth/config — public; tells the entry page whether SSO exists and
/// whether this is a fresh install (no account yet) so it can show the
/// first-run registration flow instead of login.
pub async fn auth_config(State(st): State<AppState>) -> Json<Value> {
    // SSO is offered only once the provider has been discovered; until then
    // the button simply isn't there (passkeys work regardless).
    let (enabled, name) = match &st.oidc {
        Some(o) => (o.ready(), o.name.clone()),
        None => (false, "SSO".to_string()),
    };
    // needs_setup: no account exists yet → the console shows "Create your
    // household". setup_code_required: …and it needs the one-time setup code
    // (normally carried in the #setup= link the installer printed).
    let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM admins")
        .fetch_one(&st.db)
        .await
        .unwrap_or(1);
    Json(json!({
        "needs_setup": admins == 0,
        "setup_code_required": admins == 0 && st.bootstrap_token.is_some(),
        "auth": { "oidc": enabled, "oidc_name": name },
    }))
}

/// RFC 7636 S256: base64url(sha256(verifier)), unpadded.
fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[derive(Deserialize)]
pub struct StartQuery {
    /// `?link=1`: bind the IdP identity to the signed-in account instead of
    /// signing in. Needs a session with an open confirm window.
    #[serde(default)]
    pub link: Option<String>,
}

/// The account behind the session cookie, for a link request. Needs the
/// confirm window open (`confirm::require_window`): binding a sign-in method
/// is as durable a foothold as adding a passkey.
pub(crate) async fn link_requester(st: &AppState, jar: &CookieJar) -> AppResult<Uuid> {
    crate::confirm::require_window(st, jar).await?;
    let cookie = jar
        .get(crate::state::SESSION_COOKIE)
        .ok_or_else(|| AppError::Unauthorized("no session".into()))?;
    let row: Option<(Uuid, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT s.admin_id, a.blocked_at FROM admin_sessions s
         JOIN admins a ON a.id = s.admin_id
         WHERE (s.token_hash = $1
                OR (s.prev_token_hash = $1 AND s.prev_valid_until > now()))
           AND s.expires_at > now()",
    )
    .bind(crate::auth::hash_token(cookie.value()))
    .fetch_optional(&st.db)
    .await?;
    match row {
        None => Err(AppError::Unauthorized("no session".into())),
        Some((_, Some(_paused))) => Err(AppError::ForbiddenForMember(
            "this account is paused — a parent has to lift it first".into(),
        )),
        Some((admin_id, None)) => Ok(admin_id),
    }
}

/// GET /api/auth/oidc/start — 302 to the provider's authorize URL.
pub async fn start(
    State(st): State<AppState>,
    jar: CookieJar,
    Query(q): Query<StartQuery>,
) -> AppResult<(CookieJar, Redirect)> {
    let oidc = st
        .oidc
        .as_ref()
        .ok_or_else(|| AppError::NotFound("sso is not configured".into()))?;
    let endpoints = oidc.endpoints().map_err(|_| {
        AppError::NotFound("sso is not reachable right now — try again in a minute".into())
    })?;
    let link_admin = match q.link.as_deref() {
        Some("1") | Some("true") => Some(link_requester(&st, &jar).await?),
        _ => None,
    };
    let redirect_to = if link_admin.is_some() {
        "/settings?sso=linked"
    } else {
        "/"
    };
    let (state, challenge) = oidc.issue_state(redirect_to.to_string(), link_admin).await;

    let mut url = url::Url::parse(&endpoints.authorization_endpoint)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("bad authorization endpoint: {e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &oidc.client_id)
        .append_pair("redirect_uri", &oidc.redirect_uri)
        .append_pair("scope", "openid email profile")
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
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
    /// The IdP's stable subject identifier: with the issuer, THE identity.
    #[serde(default)]
    sub: Option<String>,
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
/// `?error=sso_not_linked` for an IdP identity no account is bound to) instead of
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
    let Some(pending) = oidc.take_state(&state).await else {
        return Ok(fail(jar, "sso_failed"));
    };

    // Exchange the code (client_secret in the POST body, per Authentik; the
    // PKCE verifier proves this server started the flow).
    let token = match oidc.exchange_code(&code, &pending.pkce_verifier).await {
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

    let Some(subject) = info.sub.filter(|s| !s.trim().is_empty()) else {
        tracing::warn!("oidc userinfo has no `sub`");
        return Ok(fail(jar, "sso_failed"));
    };

    match resolve_identity(&st.db, &oidc.issuer, &subject, pending.link_admin).await? {
        Resolved::SignIn {
            admin_id,
            tenant_id,
        } => {
            let sid = create_session(&st.db, admin_id, tenant_id).await?;
            let jar = jar.add(session_cookie(sid, st.cookie_secure));
            let to = format!("{}{}", st.public_url, pending.redirect_to);
            Ok((jar, Redirect::temporary(&to)))
        }
        Resolved::Linked => {
            let to = format!("{}{}", st.public_url, pending.redirect_to);
            Ok((jar, Redirect::temporary(&to)))
        }
        Resolved::LinkedElsewhere => {
            let to = format!("{}/settings?sso=in_use", st.public_url);
            Ok((jar, Redirect::temporary(&to)))
        }
        // Family server: no auto-provisioning of extra admins, and no
        // matching on the email claim (anyone who can set their email at the
        // IdP could otherwise become a parent here).
        Resolved::NotLinked => Ok(fail(jar, "sso_not_linked")),
        Resolved::FirstRun => {
            let Some(email) = info.email.filter(|e| !e.trim().is_empty()) else {
                return Ok(fail(jar, "sso_failed"));
            };
            // The first-run account is stamped with this email, so require a
            // positively verified one (`email_verified: None` is not proof).
            if info.email_verified != Some(true) {
                return Ok(fail(jar, "sso_failed"));
            }
            // The identity is verified, but the account is not created yet —
            // the first parent gets to CHOOSE their username (the same as the
            // passkey path), instead of one derived from their email. Park the
            // identity and send them to the name-choosing page; no session is
            // issued until they finish.
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
                        subject,
                        suggested_username,
                        suggested_name,
                    },
                );
            }
            let to = format!("{}/welcome?setup={token}", st.public_url);
            Ok((jar, Redirect::temporary(&to)))
        }
    }
}

/// What an IdP identity means here.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// Bound to this account: sign it in.
    SignIn { admin_id: Uuid, tenant_id: Uuid },
    /// A link request: the identity is now bound to the requesting account.
    Linked,
    /// A link request, but the identity is already bound to another account.
    LinkedElsewhere,
    /// Nobody here has this identity, and accounts exist: refuse.
    NotLinked,
    /// Nobody exists yet: the first-run signup may claim the server.
    FirstRun,
}

/// Map `(issuer, subject)` to an account — the ONLY identity SSO trusts.
/// `link_admin` is the signed-in, confirmed account that asked to link
/// (`start?link=1`); for a plain sign-in it is `None`.
pub(crate) async fn resolve_identity(
    db: &sqlx::PgPool,
    issuer: &str,
    subject: &str,
    link_admin: Option<Uuid>,
) -> AppResult<Resolved> {
    let bound: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, tenant_id FROM admins WHERE oidc_issuer = $1 AND oidc_subject = $2",
    )
    .bind(issuer)
    .bind(subject)
    .fetch_optional(db)
    .await?;

    if let Some(me) = link_admin {
        return match bound {
            Some((owner, _)) if owner == me => Ok(Resolved::Linked),
            Some(_) => Ok(Resolved::LinkedElsewhere),
            None => {
                // The unique index still refuses a concurrent double-bind.
                let res = sqlx::query(
                    "UPDATE admins SET oidc_issuer = $1, oidc_subject = $2 WHERE id = $3",
                )
                .bind(issuer)
                .bind(subject)
                .bind(me)
                .execute(db)
                .await;
                match res {
                    Ok(_) => Ok(Resolved::Linked),
                    Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                        Ok(Resolved::LinkedElsewhere)
                    }
                    Err(e) => Err(e.into()),
                }
            }
        };
    }

    if let Some((admin_id, tenant_id)) = bound {
        return Ok(Resolved::SignIn {
            admin_id,
            tenant_id,
        });
    }
    let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM admins")
        .fetch_one(db)
        .await?;
    Ok(if admins == 0 {
        Resolved::FirstRun
    } else {
        Resolved::NotLinked
    })
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
    /// The one-time setup code (`#setup=` link), exactly as the passkey first
    /// run needs it: being able to sign in at the IdP is not the same as
    /// being the person who installed this server.
    #[serde(default)]
    setup_token: Option<String>,
}

/// POST /api/auth/oidc/setup/:token — create the first-run account with the
/// chosen username, stamp the verified email, and sign them in. Consumes the
/// token. Like the passkey first run it needs the server's setup code when it
/// has one (`auth::ensure_first_run`) — otherwise anyone the IdP lets sign in
/// could claim a fresh server. `create_tenant_with_admin(require_first =
/// true)` still guards the zero-admin race, so a second concurrent finisher
/// is refused.
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
    // Before the parked identity is taken: a wrong code must not burn it.
    crate::auth::ensure_first_run(&st, req.setup_token.as_deref()).await?;

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
        create_tenant_with_admin(&st.db, None, &username, &display_name, true).await?;
    sqlx::query("UPDATE admins SET email = $1, oidc_issuer = $2, oidc_subject = $3 WHERE id = $4")
        .bind(&pending.email)
        .bind(&oidc.issuer)
        .bind(&pending.subject)
        .bind(admin_id)
        .execute(&st.db)
        .await?;

    let sid = create_session(&st.db, admin_id, tenant_id).await?;
    let jar = jar.add(session_cookie(sid, st.cookie_secure));
    Ok((jar, Json(json!({ "ok": true }))))
}
