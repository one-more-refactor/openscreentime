//! Sentinel server — Axum + Tokio + SQLx(Postgres) + webauthn-rs.
//!
//! Two surfaces (see docs/API.md):
//!   * Admin API `/api/*`  — session-cookie auth after passkey login.
//!   * Agent API `/agent/*` — `Authorization: Bearer <device_token>`.

mod agent;
mod auth;
mod db;
mod devices;
mod discovery;
mod error;
mod events;
mod presets;
mod profiles;
mod ssh;
mod state;

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    http::{header, HeaderValue, Method},
    routing::{get, post},
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
                .unwrap_or_else(|_| "sentinel_server=debug,tower_http=info,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set (see .env.example)");
    let rp_id = std::env::var("RP_ID").unwrap_or_else(|_| "localhost".into());
    let rp_origin_str =
        std::env::var("RP_ORIGIN").unwrap_or_else(|_| "http://localhost:5173".into());
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let broker_host = std::env::var("BROKER_HOST").unwrap_or_else(|_| "localhost".into());

    // Database.
    let pool = db::connect(&database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("migrations applied");

    // WebAuthn relying party.
    let rp_origin = Url::parse(&rp_origin_str)?;
    let webauthn = WebauthnBuilder::new(&rp_id, &rp_origin)?
        .rp_name("Sentinel")
        .build()?;

    let state = AppState {
        db: pool,
        webauthn: Arc::new(webauthn),
        broker_host,
        sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        reg_states: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        auth_states: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        hub: Arc::new(Hub::default()),
    };

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

    let app = Router::new()
        .route("/health", get(health))
        // --- Auth ----------------------------------------------------------
        .route("/api/auth/register/start", post(auth::register_start))
        .route("/api/auth/register/finish", post(auth::register_finish))
        .route("/api/auth/login/start", post(auth::login_start))
        .route("/api/auth/login/finish", post(auth::login_finish))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/me/passkeys", get(auth::list_passkeys))
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
        .route("/api/devices/{id}/lock", post(devices::lock_device))
        .route("/api/devices/{id}/unlock", post(devices::unlock_device))
        .route("/api/devices/{id}/ssh", post(ssh::open_or_close))
        .route("/api/devices/{id}/users", get(devices::list_device_users))
        .route(
            "/api/device-users/{id}/assign-profile",
            post(devices::assign_profile),
        )
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
        // --- Discovery -----------------------------------------------------
        .route("/api/discovery/scan", post(discovery::scan))
        .route("/api/discovery/results", get(discovery::results))
        // --- Events --------------------------------------------------------
        .route("/api/events", get(events::list_events))
        // --- Agent API -----------------------------------------------------
        .route("/agent/enroll", post(agent::enroll))
        .route("/agent/heartbeat", post(agent::heartbeat))
        .route("/agent/policy", get(agent::policy))
        .route("/agent/events", post(agent::push_events))
        .route("/agent/commands/{id}/ack", post(agent::ack_command))
        .route("/agent/ws", get(agent::ws))
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("Sentinel server listening on {bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "sentinel-server" }))
}
