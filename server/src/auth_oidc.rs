//! OIDC SSO (tested against Authentik). Enabled only when all of
//! SENTINEL_OIDC_ISSUER / SENTINEL_OIDC_CLIENT_ID / SENTINEL_OIDC_CLIENT_SECRET
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
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{create_session, create_tenant_with_admin, gen_token, session_cookie};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// `state` parameters expire after this.
const STATE_TTL: Duration = Duration::from_secs(600);

struct PendingState {
    created: Instant,
    /// Path (relative to the public URL) to send the browser to after login.
    redirect_to: String,
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
}

#[derive(Deserialize)]
struct DiscoveryDoc {
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
}

/// Reads the SENTINEL_OIDC_* env vars; when all three are set, runs discovery
/// and returns a live config. Returns None when the feature is off.
pub async fn init_from_env(public_url: &str) -> anyhow::Result<Option<Arc<Oidc>>> {
    let issuer = std::env::var("SENTINEL_OIDC_ISSUER").ok();
    let client_id = std::env::var("SENTINEL_OIDC_CLIENT_ID").ok();
    let client_secret = std::env::var("SENTINEL_OIDC_CLIENT_SECRET").ok();
    let (Some(issuer), Some(client_id), Some(client_secret)) =
        (issuer, client_id, client_secret)
    else {
        return Ok(None);
    };
    let name = std::env::var("SENTINEL_OIDC_NAME").unwrap_or_else(|_| "SSO".into());

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

/// GET /api/auth/config — public; tells the login page whether SSO exists.
pub async fn auth_config(State(st): State<AppState>) -> Json<Value> {
    let (enabled, name) = match &st.oidc {
        Some(o) => (true, o.name.clone()),
        None => (false, "SSO".to_string()),
    };
    Json(json!({ "auth": { "oidc": enabled, "oidc_name": name } }))
}

/// GET /api/auth/oidc/start — 302 to the provider's authorize URL.
pub async fn start(State(st): State<AppState>) -> AppResult<Redirect> {
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
    Ok(Redirect::temporary(url.as_str()))
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

    let (Some(code), Some(state)) = (q.code, q.state) else {
        return Ok(fail(jar, "sso_failed"));
    };
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

    // Email match: any tenant (admins.email is globally unique).
    let existing: Option<(Uuid, Uuid)> =
        sqlx::query_as("SELECT id, tenant_id FROM admins WHERE email = $1")
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
            // Fresh install: bootstrap tenant + admin, same as the first
            // passkey registration.
            let display_name = info
                .name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| email.split('@').next().unwrap_or("Admin").to_string());
            let (tenant_id, admin_id) =
                create_tenant_with_admin(&st.db, &email, &display_name).await?;
            (admin_id, tenant_id)
        }
    };

    let sid = create_session(&st.db, admin_id, tenant_id).await?;
    let jar = jar.add(session_cookie(sid, st.cookie_secure));
    let to = format!("{}{redirect_to}", st.public_url);
    Ok((jar, Redirect::temporary(&to)))
}
