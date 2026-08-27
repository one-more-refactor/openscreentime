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
mod parent;
mod presets;
mod profiles;
mod rate_limit;
mod state;
mod static_web;
mod stepup;
mod telegram;
mod usage;
mod vpn;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    http::{header, HeaderValue, Method},
    middleware,
    routing::{delete, get, post, put},
    Json, Router,
};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;
use webauthn_rs::WebauthnBuilder;

use crate::state::{AppState, Hub};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "openscreentime_server=debug,tower_http=info,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set (see .env.example)");
    let rp_id = std::env::var("RP_ID").unwrap_or_else(|_| "localhost".into());
    let rp_origin_str =
        std::env::var("RP_ORIGIN").unwrap_or_else(|_| "http://localhost:5173".into());
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    // Cookies are Secure unless explicitly opted out for plain-http dev.
    let cookie_secure = std::env::var("OST_INSECURE_COOKIES").map(|v| v == "1") != Ok(true);
    // Public base URL (OIDC redirect URI + post-login redirects); falls back
    // to the WebAuthn RP origin.
    // Falling back to RP_ORIGIN beats minting OIDC redirect URIs out of a
    // value compose never expanded — see `state::configured`.
    let public_url = state::configured("OST_PUBLIC_URL")
        .unwrap_or_else(|| rp_origin_str.clone())
        .trim_end_matches('/')
        .to_string();

    // Database.
    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("migrations applied");
    // 0.4 backfills: every tenant gets the bracket presets it lacks, and every
    // OS login that predates accounts gets a person. Both idempotent.
    presets::backfill_all_tenants(&pool).await?;
    if let Err(e) = members::backfill_links(&pool).await {
        tracing::warn!(error = %e, "account backfill incomplete");
    }

    // WebAuthn relying party.
    let rp_origin = Url::parse(&rp_origin_str)?;
    let webauthn = WebauthnBuilder::new(&rp_id, &rp_origin)?
        .rp_name("OpenScreenTime")
        .build()?;

    // OIDC SSO (off unless the OST_OIDC_* env vars are all set).
    let oidc = auth_oidc::init_from_env(&public_url).await?;

    let state = AppState {
        db: pool,
        webauthn: Arc::new(webauthn),
        cookie_secure,
        public_url,
        reg_states: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        auth_states: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        oidc,
        rate_limiter: Arc::new(rate_limit::RateLimiter::from_env()),
        hub: Arc::new(Hub::default()),
    };

    // Offline sweeper: agents on the WS bus flip to offline the moment the
    // socket closes; a dead poll-mode agent (30 s heartbeats) would stay
    // "online" forever without this. 90 s of silence = offline. `pending` is
    // left untouched; `locked` is its own column and survives.
    {
        let db = state.db.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
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
            }
        });
    }

    // Phone alerts: one-way chat-bot messages on tamper/lockdown + time
    // requests. No-op unless a channel is configured in the environment.
    alerts::spawn(state.db.clone(), alerts::AlertConfig::from_env());
    telegram::spawn(state.clone());

    // Settled commands age out after 30 days; the event log is the audit trail.
    commands::spawn_janitor(state.clone());

    // CORS: the Vite dev server (RP_ORIGIN) talks to us with credentials.
    let cors = CorsLayer::new()
        .allow_origin(
            rp_origin_str
                .parse::<HeaderValue>()
                .expect("RP_ORIGIN must be a valid header value"),
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
        .route("/api/auth/stepup/email/start", post(stepup::email_start))
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

    let app = app.layer(cors).layer(TraceLayer::new_for_http());

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

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "openscreentime-server" }))
}
