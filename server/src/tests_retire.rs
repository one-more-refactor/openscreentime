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

/// Acceptance round 5: "Tmp" added through Add a person (which sets up their
/// computer next), then removed — and "Tmp's computer" stayed behind on
/// Computers, waiting for an install line that would link it to nobody.
/// Removing a person takes the computers set up for them that never joined;
/// one that joined stays, under the computer's own removal rules; nobody
/// else's is touched.
#[tokio::test]
async fn removing_a_person_takes_their_unfinished_computer_along() {
    let Some(env) = Env::new().await else { return };
    let (tenant, admin) = env.household("Philip").await;
    let tmp = env.member(tenant, "Tmp").await;
    let leo = env.member(tenant, "Leo").await;
    let set_up = |name: &str, account_id: Option<Uuid>| {
        let (st, who) = (env.st.clone(), owner(tenant, admin));
        let req = crate::devices::CreateDeviceReq {
            name: name.into(),
            account_id,
        };
        async move {
            let out = crate::devices::create_device(
                State(st),
                who,
                axum_extra::extract::cookie::CookieJar::new(),
                axum::Json(req),
            )
            .await
            .unwrap()
            .0;
            Uuid::parse_str(out["device"]["id"].as_str().unwrap()).unwrap()
        }
    };
    // Through the console, as "Add a person" does: waiting for its install.
    let tmps = set_up("Tmp's computer", Some(tmp)).await;
    let leos = set_up("Leo's computer", Some(leo)).await;
    let nobodys = set_up("Spare laptop", None).await;
    // …and one of Tmp's that joined: an agent holds its token.
    let joined = env.computer(tenant, Some(tmp), &["tmp"], None, None).await;
    sqlx::query("UPDATE devices SET device_token = $2, enroll_token = NULL WHERE id = $1")
        .bind(joined)
        .bind(crate::auth::hash_token("tok-tmp"))
        .execute(&env.st.db)
        .await
        .unwrap();

    let removed =
        crate::members::delete_member(State(env.st.clone()), owner(tenant, admin), Path(tmp))
            .await
            .unwrap();
    assert_eq!(removed.0["ok"], true);

    let left: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, owner_account_id FROM devices WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_all(&env.st.db)
            .await
            .unwrap();
    let has = |d: Uuid| left.iter().any(|(id, _)| *id == d);
    assert!(!has(tmps), "Tmp's unfinished computer went with Tmp");
    assert!(has(leos) && has(nobodys), "nobody else's is touched");
    assert!(has(joined), "a computer that joined stays");
    assert_eq!(
        left.iter().find(|(id, _)| *id == joined).unwrap().1,
        None,
        "…nobody's now"
    );
    // It's still an agent's computer: its token works, not "retired".
    assert!(agent_auth(&env, "tok-tmp").await.is_ok());
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
