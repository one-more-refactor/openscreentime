//! OpenScreenTime server — Axum + Tokio + SQLx(Postgres) + webauthn-rs.
//!
//! Two surfaces (see docs/API.md):
//!   * Admin API `/api/*`  — session-cookie auth after passkey login (or OIDC SSO).
//!   * Agent API `/agent/*` — `Authorization: Bearer <device_token>`.

mod agent;
mod agent_dist;
mod alerts;
mod auth;
mod auth_device;
mod auth_oidc;
mod commands;
mod db;
mod devices;
mod earn;
mod error;
mod events;
mod family;
mod members;
mod ops;
mod parent;
mod presets;
mod profiles;
mod rate_limit;
mod settings;
mod state;
mod static_web;
mod stepup;
mod supervise;
mod telegram;
mod usage;
mod vpn;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use axum::{
    extract::State,
    http::{header, HeaderValue, Method, StatusCode},
    middleware,
    routing::{any, delete, get, post, put},
    Json, Router,
};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;
use webauthn_rs::WebauthnBuilder;

use crate::state::{AppState, Hub};

async fn api_not_found() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": { "code": "not_found", "message": "no such route" } })),
    )
}

/// Baseline browser hardening on every response. No CSP yet (the SPA inlines
/// styles); these three are free and close framing/sniffing/referrer leaks.
async fn security_headers(mut resp: axum::response::Response) -> axum::response::Response {
    let h = resp.headers_mut();
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    resp
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `openscreentime-server healthcheck` is the container's health probe
    // (the runtime image has no curl): exit 0 iff /health says ok.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck().await;
    }

    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "openscreentime_server=info,tower_http=info,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url =
        std::env::var("DATABASE_URL").context("DATABASE_URL must be set (see .env.example)")?;
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    // One variable (OST_PUBLIC_URL) → RP id, origin, cookie security; the old
    // RP_ID / RP_ORIGIN / OST_INSECURE_COOKIES still override.
    let public = settings::PublicSettings::from_env()?;
    tracing::info!(
        public_url = %public.public_url,
        rp_id = %public.rp_id,
        secure_cookies = public.cookie_secure,
        version = env!("CARGO_PKG_VERSION"),
        "starting"
    );

    // Database. Retried rather than fatal: after a reboot Postgres may simply
    // still be starting, and boot order is not ours to control.
    let pool = db::connect_with_retry(&database_url).await;
    db::migrate(&pool).await?;
    tracing::info!("migrations applied");
    // 0.4 backfills: every tenant gets the bracket presets it lacks, and every
    // OS login that predates accounts gets a person. Both idempotent, and both
    // retried on the next start — neither is worth refusing to serve over.
    if let Err(e) = presets::backfill_all_tenants(&pool).await {
        tracing::warn!(error = %e, "preset backfill incomplete");
    }
    if let Err(e) = members::backfill_links(&pool).await {
        tracing::warn!(error = %e, "account backfill incomplete");
    }

    // WebAuthn relying party.
    let rp_origin = Url::parse(&public.rp_origin)?;
    let webauthn = WebauthnBuilder::new(&public.rp_id, &rp_origin)?
        .rp_name("OpenScreenTime")
        .build()?;

    // OIDC SSO (off unless the OST_OIDC_* env vars are all set). Discovery runs
    // in the background: an unreachable IdP hides the SSO button, nothing more.
    let oidc = auth_oidc::init_from_env(&public.public_url)?;

    let state = AppState {
        db: pool,
        webauthn: Arc::new(webauthn),
        cookie_secure: public.cookie_secure,
        public_url: public.public_url.clone(),
        reg_states: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        auth_states: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        oidc,
        rate_limiter: Arc::new(rate_limit::RateLimiter::from_env()),
        hub: Arc::new(Hub::default()),
        decoy_logins: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
    };

    // Offline sweeper: agents on the WS bus flip to offline the moment the
    // socket closes; a dead poll-mode agent (30 s heartbeats) would stay
    // "online" forever without this. 90 s of silence = offline. `pending` is
    // left untouched; `locked` is its own column and survives.
    {
        let db = state.db.clone();
        supervise::spawn("offline-sweep", move || {
            let db = db.clone();
            async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
                let mut sweeps: u64 = 0;
                loop {
                    tick.tick().await;
                    match sqlx::query(
                        "UPDATE devices SET status = 'offline'
                         WHERE status = 'online' AND last_seen < now() - interval '90 seconds'",
                    )
                    .execute(&db)
                    .await
                    {
                        Ok(res) => {
                            tracing::debug!(swept = res.rows_affected(), "offline sweep");
                        }
                        Err(e) => tracing::warn!(error = %e, "offline sweep failed"),
                    }
                    // Retention: first shortly after boot (a server restarted
                    // more often than hourly must still prune), then hourly.
                    sweeps += 1;
                    if sweeps % 120 == 2 {
                        prune(&db).await;
                    }
                }
            }
        });
    }

    // Phone alerts: one-way chat-bot messages on tamper/lockdown + time
    // requests. No-op unless a channel is configured in the environment.
    let alert_cfg = alerts::AlertConfig::from_env();
    alerts::spawn(state.db.clone(), alert_cfg.clone());
    telegram::spawn(state.clone());
    // System health: database, backups, updates, devices gone quiet — logged
    // always, sent to the same channels when one is configured.
    ops::spawn(state.db.clone(), alert_cfg);

    // Settled commands age out after 30 days; the event log is the audit trail.
    commands::spawn_janitor(state.clone());

    // CORS: the Vite dev server (RP_ORIGIN) talks to us with credentials.
    let cors = CorsLayer::new()
        .allow_origin(
            public
                .rp_origin
                .parse::<HeaderValue>()
                .context("RP_ORIGIN must be a valid header value")?,
        )
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    // Auth attempt endpoints: 10 req / 60 s / IP.
    let auth_attempts = Router::new()
        .route("/api/auth/register/start", post(auth::register_start))
        .route("/api/auth/register/finish", post(auth::register_finish))
        .route("/api/auth/login/start", post(auth::login_start))
        .route("/api/auth/login/finish", post(auth::login_finish))
        .route("/api/auth/device/start", post(auth_device::start))
        .route("/api/auth/oidc/start", get(auth_oidc::start))
        .route("/api/auth/oidc/callback", get(auth_oidc::callback))
        .route(
            "/api/auth/oidc/setup/{token}",
            get(auth_oidc::setup_info).post(auth_oidc::setup_finish),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::limit_auth,
        ));

    // The device-login poll gets its own generous bucket — it fires ~60 times
    // per honest sign-in and must not exhaust (or be exhausted by) the auth
    // bucket that guards the passkey fallback.
    let login_poll = Router::new()
        .route("/api/auth/device/finish", post(auth_device::finish))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::limit_poll,
        ));

    // Enrollment: 5 req / 60 s / IP.
    let enroll = Router::new()
        .route("/agent/enroll", post(agent::enroll))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::limit_enroll,
        ));

    // Agent distribution (public — the binary isn't a secret; enrollment is the
    // auth boundary). Rate-limited so it can't amplify bandwidth for free.
    let agent_dist = Router::new()
        .route("/api/agent/latest", get(agent_dist::latest))
        .route("/api/agent/download/{file}", get(agent_dist::download))
        .route("/install.sh", get(agent_dist::install_sh))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::limit_dist,
        ));

    // Parent companion API (ParentAuth bearer): 60 req / 60 s / IP.
    let parent_api = Router::new()
        .route("/api/parent/earn-requests", get(parent::list_earn_requests))
        .route(
            "/api/parent/earn-requests/{id}/approve",
            post(parent::approve),
        )
        .route("/api/parent/earn-requests/{id}/deny", post(parent::deny))
        .route("/api/parent/alerts", get(parent::alerts))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::limit_parent,
        ));

    let app = Router::new()
        .route("/health", get(health))
        // --- Agent distribution ---------------------------------------------
        .merge(agent_dist)
        // --- Auth ----------------------------------------------------------
        .merge(auth_attempts)
        .merge(login_poll)
        .route("/api/auth/config", get(auth_oidc::auth_config))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/me", get(members::me))
        .route("/api/me/today", get(members::today))
        .route("/api/me/history", get(members::history))
        .route("/api/me/goal", post(members::set_goal))
        .route("/api/me/where", get(usage::me_where))
        .route("/api/usage/where", get(usage::where_api))
        .route("/api/me/ask", post(members::ask))
        .route("/api/catalog", get(members::catalog_json))
        .route("/api/me/passkeys", get(auth::list_passkeys))
        .route("/api/me/passkeys/{id}", delete(auth::delete_passkey))
        // --- Step-up 2FA (docs/AUTH.md) -------------------------------------
        .route("/api/me/2fa", get(stepup::status))
        .route("/api/me/2fa/totp/start", post(stepup::totp_start))
        .route("/api/me/2fa/totp/confirm", post(stepup::totp_confirm))
        .route(
            "/api/me/telegram",
            get(telegram::status).delete(telegram::unpair),
        )
        .route("/api/me/telegram/pair", post(telegram::pair_start))
        .route(
            "/api/auth/stepup/telegram/start",
            post(telegram::verify_start),
        )
        // Email step-up retired: no email codes anywhere (username + passkey only).
        .route("/api/auth/stepup/verify", post(stepup::verify))
        // --- Change mode (the grant, made visible/endable/extendable) -------
        .route("/api/auth/stepup", get(stepup::change_mode_status))
        .route("/api/auth/stepup/lock", post(stepup::change_mode_lock))
        .route("/api/auth/stepup/extend", post(stepup::change_mode_extend))
        .route("/api/auth/voucher", post(stepup::redeem_voucher))
        // --- Devices -------------------------------------------------------
        .route(
            "/api/devices",
            get(devices::list_devices).post(devices::create_device),
        )
        .route(
            "/api/devices/{id}",
            get(devices::get_device)
                .patch(devices::patch_device)
                .delete(devices::delete_device),
        )
        .route(
            "/api/devices/{id}/unlock-code",
            get(devices::get_unlock_code),
        )
        .route(
            "/api/devices/{id}/unlock-code/rotate",
            post(devices::rotate_unlock_code),
        )
        .route(
            "/api/devices/{id}/recovery-codes",
            get(devices::recovery_codes_status).post(devices::generate_recovery_codes),
        )
        .route("/api/devices/{id}/ping", post(devices::ping_device))
        .route("/api/devices/{id}/lock", post(devices::lock_device))
        .route("/api/devices/{id}/unlock", post(devices::unlock_device))
        .route("/api/devices/{id}/users", get(devices::list_device_users))
        .route(
            "/api/devices/{id}/offline-window",
            put(family::set_offline_window),
        )
        .route("/api/devices/{id}/vpn", get(vpn::list).post(vpn::create))
        .route(
            "/api/vpn-profiles/{id}",
            put(vpn::update).delete(vpn::remove),
        )
        .route("/api/vpn-profiles/{id}/activate", post(vpn::activate))
        .route("/api/vpn-profiles/{id}/deactivate", post(vpn::deactivate))
        .route(
            "/api/devices/{id}/enroll-token",
            post(devices::regen_enroll_token),
        )
        .route(
            "/api/device-users/{id}/assign-profile",
            post(devices::assign_profile),
        )
        .route(
            "/api/device-users/{id}/assign-account",
            post(devices::assign_account),
        )
        .route(
            "/api/device-users/{id}/credit-time",
            post(earn::credit_time),
        )
        .route("/api/device-users/{id}/usage", get(devices::usage_history))
        // --- Command queue ---------------------------------------------------
        .route("/api/devices/{id}/commands", get(commands::list_for_device))
        .route("/api/commands/{id}/cancel", post(commands::cancel))
        // --- Earn-time requests ---------------------------------------------
        .route("/api/earn-requests", get(earn::list_requests))
        .route(
            "/api/earn-requests/{id}/approve",
            post(earn::approve_request),
        )
        .route("/api/earn-requests/{id}/deny", post(earn::deny_request))
        // --- Parent access tokens (admin manages) --------------------------
        .route(
            "/api/parent-tokens",
            get(parent::list_tokens).post(parent::mint_token),
        )
        .route("/api/parent-tokens/{id}", delete(parent::revoke_token))
        // --- Profiles ------------------------------------------------------
        .route(
            "/api/profiles",
            get(profiles::list_profiles).post(profiles::create_profile),
        )
        .route(
            "/api/profiles/{id}",
            get(profiles::get_profile)
                .put(profiles::update_profile)
                .delete(profiles::delete_profile),
        )
        // --- Members (everyone has an account) ------------------------------
        .route(
            "/api/members",
            get(members::list_members).post(members::create_member),
        )
        .route(
            "/api/members/{id}",
            axum::routing::patch(members::patch_member).delete(members::delete_member),
        )
        .route("/api/members/{id}/block", post(members::block_member))
        .route("/api/members/{id}/unblock", post(members::unblock_member))
        // --- Family (the whole home screen in one request) -----------------
        .route("/api/family", get(family::get_family))
        // --- Events --------------------------------------------------------
        .route("/api/events", get(events::list_events))
        // --- Agent API -----------------------------------------------------
        .merge(enroll)
        .route("/agent/heartbeat", post(agent::heartbeat))
        .route("/agent/policy", get(agent::policy))
        .route("/agent/events", post(agent::push_events))
        .route("/agent/earn-request", post(earn::create_request))
        .route("/agent/commands/{id}/ack", post(agent::ack_command))
        .route("/agent/ws", get(agent::ws))
        .route("/agent/voucher", post(stepup::mint_voucher))
        .route("/agent/login-decision", post(auth_device::decision))
        .route("/agent/usage", post(usage::ingest))
        // --- Parent companion API ------------------------------------------
        .merge(parent_api)
        // Read is free, write is stepped — enforced as a layer rather than a
        // per-handler extractor so that forgetting it is not possible. See
        // stepup::require_step_up for why.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            stepup::require_step_up,
        ))
        // A member session (a child on their own page) is confined to a short
        // allow-list; every other /api route is the hub's. Fails closed.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            members::guard_member,
        ))
        .with_state(state);

    // Serve the built web UI (see `web/`) as the fallback for any path that
    // didn't match an /api, /agent, or /health route above — this never
    // shadows those routes since fallbacks only run on unmatched requests.
    // No-op (API-only) if OST_WEB_DIR isn't present, e.g. plain `cargo
    // run` in dev without a web build.
    // An unmatched /api or /agent path must be a real 404, not the SPA shell
    // rewritten to 200 — otherwise a route that silently stops existing looks
    // healthy to a monitor. Static routes win over these wildcards.
    let app = app
        .route("/api/{*rest}", any(api_not_found))
        .route("/agent/{*rest}", any(api_not_found));
    let app = match static_web::web_dir() {
        Some(dir) => {
            use tower_http::services::{ServeDir, ServeFile};
            let index = dir.join("index.html");
            // Serve real files; any miss falls back to index.html. The 404 that
            // ServeDir carries through is flipped to 200 by the `spa_ok`
            // map_response layer below, so client-side routes resolve cleanly.
            let serve = ServeDir::new(&dir).not_found_service(ServeFile::new(index));
            app.fallback_service(serve)
                .layer(axum::middleware::map_response(static_web::spa_ok))
        }
        None => app,
    };

    let app = app
        .layer(middleware::map_response(security_headers))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("OpenScreenTime server listening on {bind_addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

/// SIGTERM (container stop) or Ctrl-C: stop accepting, let in-flight requests
/// finish, exit. Without this every `podman stop` waits out its timeout and
/// SIGKILLs us — and agents only notice the WebSocket is gone at their next
/// heartbeat.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    tracing::info!("shutdown signal received; draining");
}

/// Retention: usage_slices and the event log grow forever otherwise —
/// attribution is a rolling ~2-week signal and the event feed is an audit
/// trail, not an archive. Failures are logged, never fatal.
async fn prune(db: &sqlx::PgPool) {
    for (what, q) in [
        (
            "sessions",
            "DELETE FROM admin_sessions WHERE expires_at < now()",
        ),
        (
            "vouchers",
            "DELETE FROM device_vouchers WHERE expires_at < now() - interval '1 hour'",
        ),
        (
            "login requests",
            "DELETE FROM login_requests WHERE expires_at < now() - interval '1 hour'",
        ),
        (
            "usage slices",
            "DELETE FROM usage_slices WHERE hour < now() - interval '21 days'",
        ),
        (
            "events",
            "DELETE FROM events WHERE created_at < now() - interval '90 days'",
        ),
        (
            "ops log",
            "DELETE FROM ops_log WHERE created_at < now() - interval '90 days'",
        ),
    ] {
        if let Err(e) = sqlx::query(q).execute(db).await {
            tracing::warn!(what, error = %e, "retention prune failed");
        }
    }
}

/// `GET /health` — liveness AND the one dependency that matters: 200
/// `{"status":"ok"}` when the database answers, 503 `{"status":"degraded"}`
/// when it doesn't. Unauthenticated; the DB probe is cached for two seconds.
async fn health(State(st): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    let db_ok = ops::db_ok_cached(&st.db).await;
    let code = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(serde_json::json!({
            "status": if db_ok { "ok" } else { "degraded" },
            "service": "openscreentime-server",
            "version": env!("CARGO_PKG_VERSION"),
            "db": if db_ok { "ok" } else { "unreachable" },
        })),
    )
}

/// The container healthcheck: ask our own `/health` on loopback.
async fn healthcheck() -> anyhow::Result<()> {
    let port = std::env::var("BIND_ADDR")
        .ok()
        .and_then(|a| a.rsplit(':').next().map(str::to_string))
        .unwrap_or_else(|| "8080".into());
    let resp = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .build()?
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await
        .context("server not answering")?;
    anyhow::ensure!(resp.status().is_success(), "unhealthy: {}", resp.status());
    Ok(())
}
