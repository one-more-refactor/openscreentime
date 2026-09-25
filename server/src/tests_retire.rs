//! Removing a computer frees it: the server keeps a tombstone for the removed
//! device's token and answers it with `410 device_retired` + `retired: true`
//! — the one answer the agent takes itself off the computer on. Any other
//! token stays a plain 401. Database-backed (the harness in `tests_auth`).

use axum::extract::{FromRequestParts, Path, State};
use axum::response::IntoResponse;
use serde_json::Value;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::{AgentAuth, AuthAdmin};
use crate::tests_auth::Env;

fn owner(tenant: Uuid, admin: Uuid) -> AuthAdmin {
    AuthAdmin {
        admin_id: admin,
        tenant_id: tenant,
        role: "owner".into(),
        blocked: false,
    }
}

/// An enrolled computer whose agent holds `token`.
async fn enrolled(env: &Env, tenant: Uuid, token: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO devices (tenant_id, name, status, device_token, last_seen)
         VALUES ($1, 'Mia''s computer', 'online', $2, now()) RETURNING id",
    )
    .bind(tenant)
    .bind(crate::auth::hash_token(token))
    .fetch_one(&env.st.db)
    .await
    .unwrap()
}

/// What the agent's bearer token gets from any `/agent/*` route.
async fn agent_auth(env: &Env, token: &str) -> Result<AgentAuth, AppError> {
    let (mut parts, _) = axum::http::Request::builder()
        .uri("/agent/heartbeat")
        .header("authorization", format!("Bearer {token}"))
        .body(())
        .unwrap()
        .into_parts();
    AgentAuth::from_request_parts(&mut parts, &env.st).await
}

async fn answer(err: AppError) -> (u16, Value) {
    let resp = err.into_response();
    let status = resp.status().as_u16();
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

async fn remove(env: &Env, who: &AuthAdmin, device: Uuid) -> Result<(), AppError> {
    let who = AuthAdmin {
        admin_id: who.admin_id,
        tenant_id: who.tenant_id,
        role: who.role.clone(),
        blocked: who.blocked,
    };
    crate::devices::delete_device(State(env.st.clone()), who, Path(device))
        .await
        .map(|_| ())
}

#[tokio::test]
async fn a_removed_computer_hears_that_it_was_retired() {
    let Some(env) = Env::new().await else { return };
    let (tenant, admin) = env.household("Philip").await;
    let device = enrolled(&env, tenant, "tok-mia").await;

    // Enrolled: the token works.
    let ok = agent_auth(&env, "tok-mia").await.unwrap();
    assert_eq!(ok.device_id, device);

    remove(&env, &owner(tenant, admin), device).await.unwrap();

    // Removed: a distinct 410, with the flag the agent keys on.
    let err = agent_auth(&env, "tok-mia")
        .await
        .err()
        .expect("a retired token");
    assert!(matches!(err, AppError::DeviceRetired(_)), "{err:?}");
    let (status, body) = answer(err).await;
    assert_eq!(status, 410);
    assert_eq!(body["retired"], true);
    assert_eq!(body["error"]["code"], "device_retired");

    // A token nobody ever had is still a plain 401 — never "retired".
    let err = agent_auth(&env, "tok-never")
        .await
        .err()
        .expect("an unknown token");
    let (status, body) = answer(err).await;
    assert_eq!(status, 401);
    assert!(body.get("retired").is_none());

    // The device is gone for the console, and removing it again is a 404.
    assert!(matches!(
        remove(&env, &owner(tenant, admin), device).await,
        Err(AppError::NotFound(_))
    ));
    env.drop_db().await;
}

#[tokio::test]
async fn only_the_household_can_retire_its_computer() {
    let Some(env) = Env::new().await else { return };
    let (tenant, _) = env.household("Philip").await;
    // A second household on the same server.
    let (other, other_admin) =
        crate::auth::create_tenant_with_admin(&env.st.db, None, "stranger", "Stranger", false)
            .await
            .unwrap();
    let device = enrolled(&env, tenant, "tok-kept").await;

    // Another household's owner can't remove it — and leaves no tombstone.
    assert!(matches!(
        remove(&env, &owner(other, other_admin), device).await,
        Err(AppError::NotFound(_))
    ));
    assert!(agent_auth(&env, "tok-kept").await.is_ok());
    let tombstones: i64 = sqlx::query_scalar("SELECT count(*) FROM retired_devices")
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(tombstones, 0);
    env.drop_db().await;
}

#[tokio::test]
async fn a_computer_that_never_enrolled_leaves_no_tombstone() {
    let Some(env) = Env::new().await else { return };
    let (tenant, admin) = env.household("Philip").await;
    let pending: Uuid = sqlx::query_scalar(
        "INSERT INTO devices (tenant_id, name, status) VALUES ($1, 'new', 'pending') RETURNING id",
    )
    .bind(tenant)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    remove(&env, &owner(tenant, admin), pending).await.unwrap();
    let tombstones: i64 = sqlx::query_scalar("SELECT count(*) FROM retired_devices")
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(tombstones, 0);
    env.drop_db().await;
}
