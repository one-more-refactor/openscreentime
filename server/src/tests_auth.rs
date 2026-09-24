//! Database-backed tests for sign-in, confirm, recovery and OS-login linking.
//!
//! They need a Postgres they may create databases in: `OST_TEST_DATABASE_URL`,
//! else `DATABASE_URL` (CI sets it). Each test makes its own throwaway
//! database and drops it at the end; without either they skip with a note
//! instead of failing.
//!
//! The second half re-runs, as tests, the attacks an adversarial review
//! reproduced against a live server (docs/AUTH.md holds the design they
//! break): each one must now fail.

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::login_code::{self, challenge_of};
use crate::state::{AppState, AuthAdmin, Hub, SESSION_COOKIE};

struct Env {
    st: AppState,
    admin_url: String,
    name: String,
}

impl Env {
    async fn new() -> Option<Env> {
        Self::with_setup_code(None).await
    }

    async fn with_setup_code(code: Option<&str>) -> Option<Env> {
        Self::build(code, None).await
    }

    /// A database migrated only up to (not including) `before` — for testing
    /// what a migration does to data written under the old schema. Finish
    /// with [`Env::migrate_rest`].
    async fn migrated_before(before: i64) -> Option<Env> {
        Self::build(None, Some(before)).await
    }

    async fn migrate_rest(&self) {
        crate::db::migrate(&self.st.db).await.unwrap();
    }

    async fn build(code: Option<&str>, before: Option<i64>) -> Option<Env> {
        let Ok(url) =
            std::env::var("OST_TEST_DATABASE_URL").or_else(|_| std::env::var("DATABASE_URL"))
        else {
            eprintln!("DATABASE_URL not set — skipping a database test");
            return None;
        };
        let admin = PgPool::connect(&url)
            .await
            .expect("connect to DATABASE_URL");
        let name = format!("ost_test_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&admin)
            .await
            .expect("CREATE DATABASE (the test role needs CREATEDB)");
        admin.close().await;
        let mut db_url = url::Url::parse(&url).unwrap();
        db_url.set_path(&format!("/{name}"));
        let db = PgPool::connect(db_url.as_str()).await.unwrap();
        match before {
            None => crate::db::migrate(&db).await.unwrap(),
            Some(v) => {
                let mut m = sqlx::migrate!("./migrations");
                m.migrations = m
                    .migrations
                    .iter()
                    .filter(|x| x.version < v)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into();
                m.run(&db).await.unwrap();
            }
        }

        let origin = url::Url::parse("http://localhost:5173").unwrap();
        let webauthn = webauthn_rs::WebauthnBuilder::new("localhost", &origin)
            .unwrap()
            .build()
            .unwrap();
        let st = AppState {
            db,
            webauthn: Arc::new(webauthn),
            cookie_secure: false,
            public_url: "http://localhost:5173".into(),
            reg_states: Default::default(),
            auth_states: Default::default(),
            oidc: None,
            rate_limiter: Arc::new(crate::rate_limit::RateLimiter::from_env()),
            hub: Arc::new(Hub::default()),
            bootstrap_token: code.map(str::to_string),
        };
        Some(Env {
            st,
            admin_url: url,
            name,
        })
    }

    async fn drop_db(self) {
        self.st.db.close().await;
        let admin = PgPool::connect(&self.admin_url).await.unwrap();
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        ))
        .execute(&admin)
        .await;
    }

    /// A household: its owner, and a device.
    async fn household(&self, owner: &str) -> (Uuid, Uuid) {
        let username = crate::auth::username_from_name(owner);
        crate::auth::create_tenant_with_admin(&self.st.db, None, &username, owner, true)
            .await
            .unwrap()
    }

    async fn member(&self, tenant: Uuid, name: &str) -> Uuid {
        let pid = crate::members::create_profile_for(
            &self.st.db,
            tenant,
            openscreentime_policy::AgeBracket::Kid,
            name,
        )
        .await
        .unwrap();
        sqlx::query_scalar(
            "INSERT INTO admins (tenant_id, display_name, role, age_bracket, profile_id)
             VALUES ($1, $2, 'member', 'kid', $3) RETURNING id",
        )
        .bind(tenant)
        .bind(name)
        .bind(pid)
        .fetch_one(&self.st.db)
        .await
        .unwrap()
    }

    /// An enrolled, online computer owned by `owner`, with these OS logins,
    /// installed from `installer`, the owner's login picked at install time as
    /// `chosen` — through the real linking code.
    async fn computer(
        &self,
        tenant: Uuid,
        owner: Option<Uuid>,
        logins: &[&str],
        installer: Option<&str>,
        chosen: Option<&str>,
    ) -> Uuid {
        // An agent of today: it says it can show a sign-in code.
        let device: Uuid = sqlx::query_scalar(
            "INSERT INTO devices (tenant_id, name, status, owner_account_id, last_seen,
                                  agent_features)
             VALUES ($1, 'a computer', 'online', $2, now(), '{login_code}') RETURNING id",
        )
        .bind(tenant)
        .bind(owner)
        .fetch_one(&self.st.db)
        .await
        .unwrap();
        let logins: Vec<String> = logins.iter().map(|s| s.to_string()).collect();
        crate::members::settle_owner_login(&self.st.db, tenant, device, &logins, installer, chosen)
            .await
            .unwrap();
        for l in &logins {
            crate::members::link_os_user(&self.st.db, tenant, device, l, None)
                .await
                .unwrap();
        }
        device
    }

    async fn linked_to(&self, device: Uuid, login: &str) -> (Uuid, String, String) {
        sqlx::query_as(
            "SELECT a.id, a.role, a.age_bracket FROM device_users du
               JOIN admins a ON a.id = du.account_id
              WHERE du.device_id = $1 AND du.os_username = $2",
        )
        .bind(device)
        .bind(login)
        .fetch_one(&self.st.db)
        .await
        .unwrap()
    }

    /// The code the server sent to computers for `request_id` (the push runs
    /// after the response, so wait for it briefly). `None` = nothing was sent.
    /// No agent has a live socket in these tests, so the code waits in its
    /// queue row for a polling agent; this reads it the way that agent would.
    async fn sent_code(&self, request_id: Uuid) -> Option<(String, Vec<String>)> {
        for _ in 0..30 {
            let row: Option<Value> = sqlx::query_scalar(
                "SELECT payload FROM commands
                  WHERE type = 'login_code' AND payload->>'request_id' = $1",
            )
            .bind(request_id.to_string())
            .fetch_optional(&self.st.db)
            .await
            .unwrap();
            if let Some(p) = row {
                let users = p["os_users"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|u| u.as_str().unwrap().to_string())
                    .collect();
                return Some((p["code"].as_str().unwrap().to_string(), users));
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        None
    }

    async fn start(&self, name: &str, verifier: &str) -> Value {
        login_code::start(
            State(self.st.clone()),
            Json(login_code::StartReq {
                name: name.into(),
                code_challenge: challenge_of(verifier),
            }),
        )
        .await
        .unwrap()
        .0
    }

    async fn verify(&self, id: Uuid, verifier: &str, code: &str) -> Result<CookieJar, AppError> {
        login_code::verify(
            State(self.st.clone()),
            CookieJar::new(),
            Json(login_code::VerifyReq {
                request_id: id,
                code_verifier: verifier.into(),
                code: code.into(),
            }),
        )
        .await
        .map(|(jar, _)| jar)
    }
}

const VERIFIER: &str = "a-very-random-browser-verifier-0123456789";

fn id_of(v: &Value) -> Uuid {
    v["request_id"].as_str().unwrap().parse().unwrap()
}

fn wrong(code: &str) -> String {
    format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000)
}

fn is_wrong_code(r: &Result<CookieJar, AppError>) -> bool {
    matches!(r, Err(AppError::WrongCode(_)))
}

fn is_start_again(r: &Result<CookieJar, AppError>) -> bool {
    matches!(r, Err(AppError::CodeExpired(_)))
}

#[tokio::test]
async fn name_then_code_on_your_own_computer_signs_you_in() {
    let Some(env) = Env::new().await else { return };
    let (tenant, _owner) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    env.computer(tenant, Some(mia), &["dad", "mia"], Some("dad"), None)
        .await;

    let started = env.start("mia", VERIFIER).await;
    assert_eq!(started["expires_in_secs"], 300);
    let (code, os_users) = env
        .sent_code(id_of(&started))
        .await
        .expect("a code was sent");
    // Only to Mia's own login — not to the parent's admin login next to it.
    assert_eq!(os_users, vec!["mia".to_string()]);

    // A wrong code: try again. The right code from another browser: no.
    assert!(is_wrong_code(
        &env.verify(id_of(&started), VERIFIER, &wrong(&code)).await
    ));
    assert!(is_wrong_code(
        &env.verify(id_of(&started), "someone-elses-verifier-0123456789", &code)
            .await
    ));
    // The right code, from the browser that asked: signed in as Mia.
    let jar = env
        .verify(
            id_of(&started),
            VERIFIER,
            &format!(" {} {} ", &code[..3], &code[3..]),
        )
        .await
        .expect("signed in");
    let token = jar.get(SESSION_COOKIE).unwrap().value().to_string();
    let who: Uuid = sqlx::query_scalar("SELECT admin_id FROM admin_sessions WHERE token_hash = $1")
        .bind(crate::auth::hash_token(&token))
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(who, mia);
    // Once.
    assert!(is_start_again(
        &env.verify(id_of(&started), VERIFIER, &code).await
    ));
    env.drop_db().await;
}

#[tokio::test]
async fn an_unknown_name_is_indistinguishable_from_a_known_one() {
    let Some(env) = Env::new().await else { return };
    let (tenant, _) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    env.computer(tenant, Some(mia), &["mia"], None, None).await;

    let real = env.start("Mia", VERIFIER).await;
    let fake = env.start("nobody-here", VERIFIER).await;
    // Same shape, same numbers.
    let keys = |v: &Value| {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(keys(&real), keys(&fake));
    assert_eq!(real["expires_in_secs"], fake["expires_in_secs"]);
    assert!(
        env.sent_code(id_of(&fake)).await.is_none(),
        "a decoy sends nothing"
    );

    // Same answers, try for try: four "wrong code", then "start again".
    for i in 0..5 {
        let r = env.verify(id_of(&real), VERIFIER, "000000").await;
        let f = env.verify(id_of(&fake), VERIFIER, "000000").await;
        if i < 4 {
            assert!(is_wrong_code(&r) && is_wrong_code(&f), "try {i}");
        } else {
            assert!(is_start_again(&r) && is_start_again(&f), "try {i}");
        }
    }
    // And a request id that never existed reads like an expired one.
    assert!(is_start_again(
        &env.verify(Uuid::new_v4(), VERIFIER, "123456").await
    ));
    env.drop_db().await;
}

#[tokio::test]
async fn codes_expire_and_run_out_of_tries() {
    let Some(env) = Env::new().await else { return };
    let (tenant, _) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    env.computer(tenant, Some(mia), &["mia"], None, None).await;

    // Expiry: the right code after the five minutes is no good.
    let a = env.start("Mia", VERIFIER).await;
    let (code_a, _) = env.sent_code(id_of(&a)).await.unwrap();
    sqlx::query("UPDATE login_codes SET expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(id_of(&a))
        .execute(&env.st.db)
        .await
        .unwrap();
    assert!(is_start_again(
        &env.verify(id_of(&a), VERIFIER, &code_a).await
    ));

    // Tries: five wrong codes and even the right one is refused.
    let b = env.start("Mia", VERIFIER).await;
    let (code_b, _) = env.sent_code(id_of(&b)).await.unwrap();
    for _ in 0..4 {
        assert!(is_wrong_code(
            &env.verify(id_of(&b), VERIFIER, &wrong(&code_b)).await
        ));
    }
    assert!(is_start_again(
        &env.verify(id_of(&b), VERIFIER, &wrong(&code_b)).await
    ));
    assert!(is_start_again(
        &env.verify(id_of(&b), VERIFIER, &code_b).await
    ));

    // A stranger typing her name can't flood her computer: the sixth ask in
    // ten minutes (used-up ones count) is a decoy, and looks like any other.
    for _ in 0..3 {
        let c = env.start("Mia", VERIFIER).await;
        assert!(env.sent_code(id_of(&c)).await.is_some());
    }
    let f = env.start("Mia", VERIFIER).await;
    assert!(env.sent_code(id_of(&f)).await.is_none());
    env.drop_db().await;
}

#[tokio::test]
async fn a_parents_code_only_goes_to_the_parents_own_computer() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;

    // Mia's laptop, installed from the parent's admin login there.
    let laptop = env
        .computer(tenant, Some(mia), &["philip", "mia"], Some("philip"), None)
        .await;
    // Even if that login is linked to the parent by hand…
    sqlx::query(
        "UPDATE device_users SET account_id = $1 WHERE device_id = $2 AND os_username = 'philip'",
    )
    .bind(philip)
    .bind(laptop)
    .execute(&env.st.db)
    .await
    .unwrap();
    // …a child's computer never gets the parent's code.
    let s = env.start("Philip", VERIFIER).await;
    assert!(env.sent_code(id_of(&s)).await.is_none());

    // "This is my computer", installed from the parent's own login, with the
    // child's login on it too.
    let desk = env
        .computer(
            tenant,
            Some(philip),
            &["philip", "leo"],
            Some("philip"),
            None,
        )
        .await;
    let s = env.start("philip", VERIFIER).await;
    let (_, os_users) = env
        .sent_code(id_of(&s))
        .await
        .expect("sent to the parent's computer");
    assert_eq!(os_users, vec!["philip".to_string()]);
    // The child's login there is a person of its own, not the parent — with
    // rules that enforce nothing on a parent's computer until a parent sorts
    // it out, and flagged for them to.
    let (leo, role, bracket) = env.linked_to(desk, "leo").await;
    assert_ne!(leo, philip);
    assert_eq!((role.as_str(), bracket.as_str()), ("member", "adult"));
    assert!(unsorted(&env, desk, "leo").await);
    assert!(!unsorted(&env, desk, "philip").await);
    env.drop_db().await;
}

async fn unsorted(env: &Env, device: Uuid, login: &str) -> bool {
    sqlx::query_scalar(
        "SELECT unsorted FROM device_users WHERE device_id = $1 AND os_username = $2",
    )
    .bind(device)
    .bind(login)
    .fetch_one(&env.st.db)
    .await
    .unwrap()
}

/// What rules a login is under: (profile kind, screen time enforced?).
async fn rules_of(env: &Env, device: Uuid, login: &str) -> (String, bool) {
    sqlx::query_as(
        "SELECT p.kind, COALESCE((p.policy->'screen_time'->>'enabled')::boolean, false)
           FROM device_users du JOIN profiles p ON p.id = du.profile_id
          WHERE du.device_id = $1 AND du.os_username = $2",
    )
    .bind(device)
    .bind(login)
    .fetch_one(&env.st.db)
    .await
    .unwrap()
}

#[tokio::test]
async fn each_os_login_is_its_own_person() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;

    // A child's laptop, installed from the parent's admin login "dad"; asked
    // "which login is Mia's?", they picked hers.
    let laptop = env
        .computer(
            tenant,
            Some(mia),
            &["dad", "minecraftqueen"],
            Some("dad"),
            Some("minecraftqueen"),
        )
        .await;
    assert_eq!(env.linked_to(laptop, "minecraftqueen").await.0, mia);
    // The parent's admin login is not the child — a person of its own, until
    // the parent says it's them.
    let (dad, role, _) = env.linked_to(laptop, "dad").await;
    assert!(dad != mia && dad != philip);
    assert_eq!(role, "member");

    // Nobody answered and nothing gives it away: nobody is linked to Mia by
    // guesswork, and nobody escapes rules either (a child's, by default).
    let quiet = env
        .computer(
            tenant,
            Some(mia),
            &["dad", "minecraftqueen"],
            Some("minecraftqueen"),
            None,
        )
        .await;
    for login in ["dad", "minecraftqueen"] {
        let (who, _, bracket) = env.linked_to(quiet, login).await;
        assert!(who != mia && who != philip, "{login}");
        assert_eq!(bracket, "kid", "{login}");
    }

    // A login that appears later (a heartbeat) is its own person too.
    crate::members::link_os_user(&env.st.db, tenant, laptop, "guest", None)
        .await
        .unwrap();
    let (guest, _, _) = env.linked_to(laptop, "guest").await;
    assert!(guest != mia && guest != philip && guest != dad);

    // A parent's computer with several logins and no idea who installed it:
    // nobody is linked to the parent.
    let shared = env
        .computer(tenant, Some(philip), &["philip", "leo"], None, None)
        .await;
    assert_ne!(env.linked_to(shared, "philip").await.0, philip);
    assert_ne!(env.linked_to(shared, "leo").await.0, philip);
    env.drop_db().await;
}

async fn session_for(env: &Env, admin: Uuid, tenant: Uuid) -> (CookieJar, AuthAdmin) {
    let token = crate::auth::create_session(&env.st.db, admin, tenant)
        .await
        .unwrap();
    // Start with the confirm window shut, as it is 15 minutes after sign-in.
    sqlx::query("UPDATE admin_sessions SET stepup_until = NULL WHERE token_hash = $1")
        .bind(crate::auth::hash_token(&token))
        .execute(&env.st.db)
        .await
        .unwrap();
    let role: String = sqlx::query_scalar("SELECT role FROM admins WHERE id = $1")
        .bind(admin)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    (
        CookieJar::new().add(Cookie::new(SESSION_COOKIE, token)),
        AuthAdmin {
            admin_id: admin,
            tenant_id: tenant,
            role,
            blocked: false,
        },
    )
}

#[tokio::test]
async fn confirm_with_a_code_from_your_computer() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let (jar, me) = session_for(&env, philip, tenant).await;

    // No computer yet: the dialog must say so, not offer a dead end.
    let status = crate::confirm::status(State(env.st.clone()), me.clone(), jar.clone())
        .await
        .unwrap()
        .0;
    assert_eq!(status["computer"], false);
    assert_eq!(status["passkey"], false);
    assert!(status["armed_until"].is_null());
    assert!(matches!(
        crate::confirm::code_start(State(env.st.clone()), me.clone(), jar.clone()).await,
        Err(AppError::Conflict(_))
    ));

    env.computer(tenant, Some(philip), &["philip"], Some("philip"), None)
        .await;
    let started = crate::confirm::code_start(State(env.st.clone()), me.clone(), jar.clone())
        .await
        .unwrap()
        .0;
    let id = id_of(&started);
    let (code, _) = env.sent_code(id).await.unwrap();

    // Another session of the same person can't use this code.
    let (other_jar, _) = session_for(&env, philip, tenant).await;
    let r = crate::confirm::code_verify(
        State(env.st.clone()),
        me.clone(),
        other_jar,
        Json(crate::confirm::CodeVerifyReq {
            request_id: id,
            code: code.clone(),
        }),
    )
    .await;
    assert!(matches!(r, Err(AppError::WrongCode(_))));

    let (_, body) = crate::confirm::code_verify(
        State(env.st.clone()),
        me.clone(),
        jar,
        Json(crate::confirm::CodeVerifyReq {
            request_id: id,
            code,
        }),
    )
    .await
    .unwrap();
    assert!(body.0["armed_until"].is_string());
    env.drop_db().await;
}

#[tokio::test]
async fn first_run_needs_the_setup_code_and_happens_once() {
    let Some(env) = Env::with_setup_code(Some("the-setup-code")).await else {
        return;
    };
    let start = |code: Option<&str>| {
        crate::auth::register_start(
            State(env.st.clone()),
            CookieJar::new(),
            Json(crate::auth::RegisterStartReq {
                name: "Philip".into(),
                setup_token: code.map(str::to_string),
            }),
        )
    };
    assert!(matches!(start(None).await, Err(AppError::Unauthorized(_))));
    assert!(matches!(
        start(Some("wrong")).await,
        Err(AppError::Unauthorized(_))
    ));
    let (_, opts) = start(Some("the-setup-code")).await.unwrap();
    // A discoverable passkey, so "Sign in with a passkey" can find it later.
    assert_eq!(
        opts.0
            .pointer("/publicKey/authenticatorSelection/residentKey"),
        Some(&serde_json::json!("required"))
    );
    assert_eq!(
        opts.0.pointer("/publicKey/user/displayName"),
        Some(&serde_json::json!("Philip"))
    );

    // Once a household exists, the door is shut — setup code or not.
    env.household("Someone").await;
    assert!(matches!(
        start(Some("the-setup-code")).await,
        Err(AppError::RegistrationClosed(_))
    ));
    env.drop_db().await;
}

#[tokio::test]
async fn a_recovery_link_signs_in_the_existing_account_once() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let token = crate::voucher::mint_link(&env.st.db, tenant, philip)
        .await
        .unwrap();
    let redeem = || {
        crate::voucher::redeem_link(
            State(env.st.clone()),
            CookieJar::new(),
            Json(crate::voucher::TokenReq {
                voucher: String::new(),
                token: token.clone(),
            }),
        )
    };
    let (jar, _) = redeem().await.unwrap();
    let sid = jar.get(SESSION_COOKIE).unwrap().value().to_string();
    let (who, confirmed): (Uuid, bool) = sqlx::query_as(
        "SELECT admin_id, stepup_until > now() FROM admin_sessions WHERE token_hash = $1",
    )
    .bind(crate::auth::hash_token(&sid))
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    assert_eq!(who, philip, "the same account — not a new household");
    assert!(confirmed, "so a new passkey can be added straight away");
    assert!(redeem().await.is_err(), "once");
    let households: i64 = sqlx::query_scalar("SELECT count(*) FROM tenants")
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(households, 1);
    env.drop_db().await;
}

// ── the review's attacks, re-run ────────────────────────────────────────────

/// The console routes an attack goes through, behind the real confirm layer.
fn guarded(env: &Env) -> axum::Router {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/devices", post(crate::devices::create_device))
        .route(
            "/api/devices/{id}/unlock-code",
            get(crate::devices::get_unlock_code),
        )
        .route(
            "/api/devices/{id}/commands",
            get(crate::commands::list_for_device),
        )
        .route(
            "/api/device-users/{id}/assign-account",
            post(crate::devices::assign_account),
        )
        .route(
            "/api/auth/confirm/code/start",
            post(crate::confirm::code_start),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            env.st.clone(),
            crate::confirm::require_confirm,
        ))
        .with_state(env.st.clone())
}

/// One request through [`guarded`] as the session in `jar`: (status, body).
async fn call(env: &Env, jar: &CookieJar, method: &str, uri: &str, body: Value) -> (u16, Value) {
    use tower::ServiceExt;
    let token = jar.get(SESSION_COOKIE).unwrap().value().to_string();
    let req = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("cookie", format!("{SESSION_COOKIE}={token}"))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(if body.is_null() {
            String::new()
        } else {
            body.to_string()
        }))
        .unwrap();
    let resp = guarded(env).oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn open_confirm_window(env: &Env, jar: &CookieJar) {
    let token = jar.get(SESSION_COOKIE).unwrap().value().to_string();
    sqlx::query(
        "UPDATE admin_sessions SET stepup_until = now() + interval '15 minutes'
          WHERE token_hash = $1",
    )
    .bind(crate::auth::hash_token(&token))
    .execute(&env.st.db)
    .await
    .unwrap();
}

async fn preset_profile(env: &Env, tenant: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM profiles WHERE tenant_id = $1 AND is_preset LIMIT 1")
        .bind(tenant)
        .fetch_one(&env.st.db)
        .await
        .unwrap()
}

/// A computer as a server before 0030 left it: owned by `owner`, each login
/// linked to whoever the old rule linked it to, no owner login settled.
async fn legacy_computer(env: &Env, tenant: Uuid, owner: Uuid, logins: &[(&str, Uuid)]) -> Uuid {
    let device: Uuid = sqlx::query_scalar(
        "INSERT INTO devices (tenant_id, name, status, owner_account_id, last_seen)
         VALUES ($1, 'an old computer', 'online', $2, now()) RETURNING id",
    )
    .bind(tenant)
    .bind(owner)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    let profile = preset_profile(env, tenant).await;
    for (login, who) in logins {
        sqlx::query(
            "INSERT INTO device_users (device_id, os_username, profile_id, account_id)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(device)
        .bind(login)
        .bind(profile)
        .bind(who)
        .execute(&env.st.db)
        .await
        .unwrap();
    }
    device
}

async fn owner_login(env: &Env, device: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT owner_os_username FROM devices WHERE id = $1")
        .bind(device)
        .fetch_one(&env.st.db)
        .await
        .unwrap()
}

async fn mint_voucher(
    env: &Env,
    device: Uuid,
    tenant: Uuid,
    login: &str,
) -> Result<String, AppError> {
    crate::voucher::mint(
        State(env.st.clone()),
        crate::state::AgentAuth {
            device_id: device,
            tenant_id: tenant,
        },
        Some(Json(crate::voucher::MintVoucherReq {
            os_username: login.into(),
        })),
    )
    .await
    .map(|v| v.0["voucher"].as_str().unwrap().to_string())
}

async fn device_user(env: &Env, device: Uuid, login: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM device_users WHERE device_id = $1 AND os_username = $2")
        .bind(device)
        .bind(login)
        .fetch_one(&env.st.db)
        .await
        .unwrap()
}

/// Finding 1 (HIGH): on a parent's computer that an older server linked
/// every login on to the parent, a "Philip" code went to the child's login
/// too, and `ost login` from the child's login minted a parent session.
#[tokio::test]
async fn attack_stale_owner_links_open_no_door_to_the_parent() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let desk = legacy_computer(&env, tenant, philip, &[("philip", philip), ("leo", philip)]).await;
    sqlx::query("UPDATE devices SET agent_features = '{login_code}' WHERE id = $1")
        .bind(desk)
        .execute(&env.st.db)
        .await
        .unwrap();

    // Until the owner's login is settled, neither login gets the parent's
    // code or a voucher for the parent.
    let s = env.start("Philip", VERIFIER).await;
    assert!(
        env.sent_code(id_of(&s)).await.is_none(),
        "code to an unsettled login"
    );
    for login in ["leo", "philip"] {
        assert!(
            matches!(
                mint_voucher(&env, desk, tenant, login).await,
                Err(AppError::NoAccount(_))
            ),
            "voucher for {login}"
        );
    }

    // The parent says which login is theirs (Who's who — behind confirm).
    let (jar, _) = session_for(&env, philip, tenant).await;
    let who = device_user(&env, desk, "philip").await;
    let uri = format!("/api/device-users/{who}/assign-account");
    let body = serde_json::json!({ "account_id": philip });
    assert_eq!(call(&env, &jar, "POST", &uri, body.clone()).await.0, 428);
    open_confirm_window(&env, &jar).await;
    assert_eq!(call(&env, &jar, "POST", &uri, body).await.0, 200);
    assert_eq!(owner_login(&env, desk).await.as_deref(), Some("philip"));

    // Now the code goes to the parent's login only, and only it vouches.
    let s = env.start("Philip", VERIFIER).await;
    let (_, users) = env.sent_code(id_of(&s)).await.expect("sent to philip");
    assert_eq!(users, vec!["philip".to_string()]);
    assert!(mint_voucher(&env, desk, tenant, "philip").await.is_ok());
    assert!(matches!(
        mint_voucher(&env, desk, tenant, "leo").await,
        Err(AppError::NoAccount(_))
    ));
    env.drop_db().await;
}

/// Finding 1, the upgrade itself: migration 0030 must not carry the old
/// links over as "the parent's login".
#[tokio::test]
async fn attack_upgrading_does_not_make_a_childs_login_the_parent() {
    let Some(env) = Env::migrated_before(30).await else {
        return;
    };
    let (tenant, philip) = env.household("Philip").await;
    // Several logins linked to the parent: nobody can say which is theirs.
    let shared =
        legacy_computer(&env, tenant, philip, &[("philip", philip), ("leo", philip)]).await;
    // One login linked to the parent: that one is.
    let own = legacy_computer(&env, tenant, philip, &[("philip", philip)]).await;

    env.migrate_rest().await;
    crate::members::backfill_links(&env.st.db).await.unwrap();

    assert_eq!(owner_login(&env, shared).await, None);
    for login in ["philip", "leo"] {
        let (who, role, bracket) = env.linked_to(shared, login).await;
        assert_ne!(who, philip, "{login} is still the parent");
        // Never brick: one of them is the parent's own login, so neither is
        // put under a child's rules by a guess — nothing is enforced on them
        // until a parent sorts them out.
        assert_eq!(
            (role.as_str(), bracket.as_str()),
            ("member", "adult"),
            "{login}"
        );
        assert_eq!(rules_of(&env, shared, login).await, ("adult".into(), false));
        assert!(unsorted(&env, shared, login).await, "{login}");
    }
    assert_eq!(owner_login(&env, own).await.as_deref(), Some("philip"));
    assert_eq!(env.linked_to(own, "philip").await.0, philip);

    // …and the parent gets no code on either of them until it's settled.
    sqlx::query("UPDATE devices SET agent_features = '{login_code}'")
        .execute(&env.st.db)
        .await
        .unwrap();
    let targets = login_code::code_targets(&env.st.db, philip, tenant)
        .await
        .unwrap();
    assert_eq!(targets, vec![(own, "philip".to_string())]);

    // The Family page asks them to: two logins nobody has sorted, there.
    let (jar, me) = session_for(&env, philip, tenant).await;
    let unsorted_on = |family: &Value, device: Uuid| {
        family["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == device.to_string().as_str())
            .unwrap()["unsorted_logins"]
            .as_i64()
            .unwrap()
    };
    let family = crate::family::get_family(State(env.st.clone()), me.clone())
        .await
        .unwrap()
        .0;
    assert_eq!(unsorted_on(&family, shared), 2);
    assert_eq!(unsorted_on(&family, own), 0);

    // The parent says which is theirs (Who's who): that one is sorted and
    // gets their codes; the other still waits to be sorted.
    open_confirm_window(&env, &jar).await;
    let who = device_user(&env, shared, "philip").await;
    let uri = format!("/api/device-users/{who}/assign-account");
    let body = serde_json::json!({ "account_id": philip });
    assert_eq!(call(&env, &jar, "POST", &uri, body).await.0, 200);
    let family = crate::family::get_family(State(env.st.clone()), me)
        .await
        .unwrap()
        .0;
    assert_eq!(unsorted_on(&family, shared), 1);
    let mut targets = login_code::code_targets(&env.st.db, philip, tenant)
        .await
        .unwrap();
    targets.sort();
    let mut expected = vec![(own, "philip".to_string()), (shared, "philip".to_string())];
    expected.sort();
    assert_eq!(targets, expected);
    env.drop_db().await;
}

/// Finding 2 (HIGH): with the confirm window shut, a stolen session asked for
/// a confirm code, read it off `GET /api/devices/{id}/commands`, and opened
/// the window with it.
#[tokio::test]
async fn attack_a_confirm_code_cannot_be_read_off_the_command_queue() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let desk = env
        .computer(tenant, Some(philip), &["philip"], Some("philip"), None)
        .await;
    let (jar, _) = session_for(&env, philip, tenant).await;
    let keys = format!("/api/devices/{desk}/unlock-code");
    assert_eq!(call(&env, &jar, "GET", &keys, Value::Null).await.0, 428);
    let ask = || {
        call(
            &env,
            &jar,
            "POST",
            "/api/auth/confirm/code/start",
            serde_json::json!({}),
        )
    };

    let (status, started) = ask().await;
    assert_eq!(status, 200);
    // What the parent's (polling) computer receives…
    let (code, _) = env.sent_code(id_of(&started)).await.unwrap();
    // …is nowhere in the console's view of the queue.
    let (status, listing) = call(
        &env,
        &jar,
        "GET",
        &format!("/api/devices/{desk}/commands"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    let listed = listing["commands"].as_array().unwrap();
    assert!(listed.iter().all(|c| c["type"] != "login_code"));
    assert!(!listing.to_string().contains(&code));
    assert_eq!(call(&env, &jar, "GET", &keys, Value::Null).await.0, 428);

    let payloads = |id: Option<Uuid>| {
        let db = env.st.db.clone();
        async move {
            let rows: Vec<Value> = sqlx::query_scalar(
                "SELECT payload FROM commands
                  WHERE type = 'login_code' AND ($1::uuid IS NULL OR id = $1)",
            )
            .bind(id)
            .fetch_all(&db)
            .await
            .unwrap();
            rows
        }
    };
    let empty = serde_json::json!({});

    // The polling agent takes it: the row is emptied as it goes, and a code
    // is never redelivered.
    let pulled = crate::agent::pull_pending_commands(&env.st.db, desk)
        .await
        .unwrap();
    assert!(pulled.iter().any(|c| c["payload"]["code"] == code.as_str()));
    assert!(payloads(None).await.iter().all(|p| p == &empty));
    sqlx::query("UPDATE commands SET sent_at = now() - interval '10 minutes'")
        .execute(&env.st.db)
        .await
        .unwrap();
    let again = crate::agent::pull_pending_commands(&env.st.db, desk)
        .await
        .unwrap();
    assert!(again.iter().all(|c| c["type"] != "login_code"));

    // A live socket: the code travels in the frame only; the row never has it.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    env.st.hub.register_agent(desk, tx).await;
    let (_, started) = ask().await;
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
        .await
        .expect("a frame")
        .unwrap();
    assert_eq!(
        frame["command"]["payload"]["request_id"],
        started["request_id"]
    );
    assert_eq!(
        frame["command"]["payload"]["code"].as_str().unwrap().len(),
        6
    );
    let cmd: Uuid = frame["command"]["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(payloads(Some(cmd)).await, vec![empty.clone()]);
    env.st.hub.force_unregister(desk).await;

    // One nobody picked up is emptied and withdrawn once it has run out.
    let (_, started) = ask().await;
    assert!(env.sent_code(id_of(&started)).await.is_some());
    sqlx::query(
        "UPDATE commands SET created_at = now() - interval '6 minutes' WHERE status = 'queued'",
    )
    .execute(&env.st.db)
    .await
    .unwrap();
    crate::login_code::scrub_commands(&env.st.db).await;
    assert!(payloads(None).await.iter().all(|p| p == &empty));
    let withdrawn: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE type = 'login_code' AND status = 'cancelled'",
    )
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    assert_eq!(withdrawn, 1);
    env.drop_db().await;
}

/// Finding 3 (HIGH): with the confirm window shut, a stolen session set up a
/// "my computer" for the parent, enrolled it, minted a voucher there — and
/// had a fresh parent session with the window open, surviving logout, with
/// no alert.
#[tokio::test]
async fn attack_a_stolen_session_cannot_set_up_a_computer_as_the_parent() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    let (jar, _) = session_for(&env, philip, tenant).await;

    let mine = serde_json::json!({ "name": "Philip's computer", "account_id": philip });
    let (status, body) = call(&env, &jar, "POST", "/api/devices", mine.clone()).await;
    assert_eq!(status, 428, "{body}");
    let devices: i64 = sqlx::query_scalar("SELECT count(*) FROM devices")
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(devices, 0, "nothing was created");
    // A child's computer is no door to a parent: no confirm needed.
    let hers = serde_json::json!({ "name": "Mia's computer", "account_id": mia });
    assert_eq!(call(&env, &jar, "POST", "/api/devices", hers).await.0, 200);
    // Confirmed: fine.
    open_confirm_window(&env, &jar).await;
    let (status, body) = call(&env, &jar, "POST", "/api/devices", mine).await;
    assert_eq!(status, 200);
    assert!(body["enroll_token"].is_string());

    // And a voucher sign-in is never silent: a parent's is phoned out.
    let desk = env
        .computer(tenant, Some(philip), &["philip"], Some("philip"), None)
        .await;
    let laptop = env.computer(tenant, Some(mia), &["mia"], None, None).await;
    for (device, login) in [(desk, "philip"), (laptop, "mia")] {
        let voucher = mint_voucher(&env, device, tenant, login).await.unwrap();
        let _ = crate::voucher::redeem(
            State(env.st.clone()),
            CookieJar::new(),
            Json(crate::voucher::TokenReq {
                voucher,
                token: String::new(),
            }),
        )
        .await
        .unwrap();
    }
    let alerts: Vec<(String, String)> = sqlx::query_as(
        "SELECT payload->>'account_id', severity FROM events
          WHERE type = 'account_login' AND payload->>'via' = 'device_voucher'
          ORDER BY severity",
    )
    .fetch_all(&env.st.db)
    .await
    .unwrap();
    assert_eq!(
        alerts,
        vec![
            (philip.to_string(), "critical".to_string()),
            (mia.to_string(), "info".to_string()),
        ]
    );

    // So is a recovery link.
    let token = crate::voucher::mint_link(&env.st.db, tenant, philip)
        .await
        .unwrap();
    let _ = crate::voucher::redeem_link(
        State(env.st.clone()),
        CookieJar::new(),
        Json(crate::voucher::TokenReq {
            voucher: String::new(),
            token,
        }),
    )
    .await
    .unwrap();
    let severity: String = sqlx::query_scalar(
        "SELECT severity FROM events
          WHERE type = 'account_login' AND payload->>'via' = 'recovery_link'",
    )
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    assert_eq!(severity, "critical");
    env.drop_db().await;
}

/// Finding 4 (MEDIUM): sixty parallel asks for one name issued 43 real codes
/// against a cap of 5, and confirm codes had no cap at all.
#[tokio::test]
async fn attack_parallel_asks_cannot_outrun_the_code_cap() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    env.computer(tenant, Some(mia), &["mia"], None, None).await;
    env.computer(tenant, Some(philip), &["philip"], Some("philip"), None)
        .await;

    futures::future::join_all((0..30).map(|_| env.start("Mia", VERIFIER))).await;
    let real: i64 = sqlx::query_scalar("SELECT count(*) FROM login_codes WHERE account_id = $1")
        .bind(mia)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(real, login_code::MAX_RECENT_PER_ACCOUNT);

    // Confirm codes: the same cap, in parallel too.
    let (jar, me) = session_for(&env, philip, tenant).await;
    let asks = futures::future::join_all(
        (0..12).map(|_| crate::confirm::code_start(State(env.st.clone()), me.clone(), jar.clone())),
    )
    .await;
    let sent = asks.iter().filter(|r| r.is_ok()).count() as i64;
    assert_eq!(sent, login_code::MAX_RECENT_PER_ACCOUNT);
    assert!(asks
        .iter()
        .filter_map(|r| r.as_ref().err())
        .all(|e| matches!(e, AppError::RateLimited(_))));
    env.drop_db().await;
}

/// Finding 4: a per-person budget of wrong codes. Spent, the code door
/// answers with decoys for the rest of the hour, and the household hears
/// about it once.
#[tokio::test]
async fn attack_guessing_codes_closes_the_code_door_for_the_hour() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    env.computer(tenant, Some(mia), &["mia"], None, None).await;

    let mut guessed = 0;
    while guessed < login_code::WRONG_PER_HOUR {
        let s = env.start("Mia", VERIFIER).await;
        let (code, _) = env.sent_code(id_of(&s)).await.expect("a real code");
        for _ in 0..login_code::MAX_ATTEMPTS {
            let _ = env.verify(id_of(&s), VERIFIER, &wrong(&code)).await;
            guessed += 1;
        }
    }
    // Budget spent: a decoy, though the ten-minute cap isn't reached.
    let s = env.start("Mia", VERIFIER).await;
    assert!(env.sent_code(id_of(&s)).await.is_none());
    // Guessing on at the decoy changes nothing, and warns nobody again.
    for _ in 0..3 {
        assert!(is_wrong_code(
            &env.verify(id_of(&s), VERIFIER, "000000").await
        ));
    }
    let warnings: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT severity, device_id FROM events
          WHERE type = 'account_login' AND payload->>'kind' = 'code_guessing'
            AND payload->>'account_id' = $1",
    )
    .bind(mia.to_string())
    .fetch_all(&env.st.db)
    .await
    .unwrap();
    assert_eq!(warnings, vec![("critical".to_string(), None)]);

    // The same budget closes the confirm-code door for a parent.
    env.computer(tenant, Some(philip), &["philip"], Some("philip"), None)
        .await;
    let (jar, me) = session_for(&env, philip, tenant).await;
    let mut guessed = 0;
    while guessed < login_code::WRONG_PER_HOUR {
        let started = crate::confirm::code_start(State(env.st.clone()), me.clone(), jar.clone())
            .await
            .unwrap()
            .0;
        for _ in 0..login_code::MAX_ATTEMPTS {
            let _ = crate::confirm::code_verify(
                State(env.st.clone()),
                me.clone(),
                jar.clone(),
                Json(crate::confirm::CodeVerifyReq {
                    request_id: id_of(&started),
                    code: "not-it".into(),
                }),
            )
            .await;
            guessed += 1;
        }
    }
    assert!(matches!(
        crate::confirm::code_start(State(env.st.clone()), me, jar).await,
        Err(AppError::RateLimited(_))
    ));
    env.drop_db().await;
}

/// Finding 5a (LOW): enrolls racing with one token could all get
/// credentials — the losers slipping in through the retry arm from any host.
#[tokio::test]
async fn attack_racing_enrolls_with_one_token_enroll_one_host() {
    let Some(env) = Env::new().await else { return };
    let (tenant, _) = env.household("Philip").await;
    let token = crate::auth::gen_token();
    sqlx::query(
        "INSERT INTO devices (tenant_id, name, status, enroll_token, enroll_token_expires_at)
         VALUES ($1, 'new', 'pending', $2, now() + interval '1 day')",
    )
    .bind(tenant)
    .bind(crate::auth::hash_token(&token))
    .execute(&env.st.db)
    .await
    .unwrap();
    let enroll = |host: String| {
        let req = serde_json::from_value(serde_json::json!({
            "enroll_token": token, "hostname": host, "os_users": [],
        }))
        .unwrap();
        crate::agent::enroll(State(env.st.clone()), Json(req))
    };
    let raced = futures::future::join_all((0..8).map(|i| enroll(format!("host-{i}")))).await;
    assert_eq!(raced.iter().filter(|r| r.is_ok()).count(), 1);

    // The designed retry — the same host, before it ever used its
    // credentials — still works; another host still can't.
    let host: String = sqlx::query_scalar("SELECT hostname FROM devices")
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert!(enroll(host.clone()).await.is_ok());
    assert!(enroll(format!("{host}-not")).await.is_err());
    env.drop_db().await;
}

/// Finding 5b (LOW): the SSO first run skipped the setup code the passkey
/// first run needs — anyone the IdP lets sign in could claim a fresh server.
#[tokio::test]
async fn attack_sso_cannot_claim_a_fresh_server_without_the_setup_code() {
    let Some(mut env) = Env::with_setup_code(Some("the-setup-code")).await else {
        return;
    };
    let (oidc, parked) = crate::auth_oidc::Oidc::parked_for_test("stranger@example.com").await;
    env.st.oidc = Some(oidc);
    let finish = |code: Option<&str>| {
        let req = serde_json::from_value(serde_json::json!({
            "username": "stranger", "setup_token": code,
        }))
        .unwrap();
        crate::auth_oidc::setup_finish(
            State(env.st.clone()),
            CookieJar::new(),
            axum::extract::Path(parked.clone()),
            Json(req),
        )
    };
    assert!(matches!(finish(None).await, Err(AppError::Unauthorized(_))));
    assert!(matches!(
        finish(Some("wrong")).await,
        Err(AppError::Unauthorized(_))
    ));
    let households: i64 = sqlx::query_scalar("SELECT count(*) FROM tenants")
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(households, 0);
    // The installer, with the code: the parked sign-in survived the misses.
    assert!(finish(Some("the-setup-code")).await.is_ok());
    env.drop_db().await;
}

/// Finding 5c (LOW): an agent from before sign-in codes doesn't know the
/// command. It is never sent one (a decoy answers, as for anyone with no
/// computer to show it on), and one it gets anyway can't wedge its queue.
#[tokio::test]
async fn an_agent_from_before_codes_is_never_sent_one() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    let laptop = env.computer(tenant, Some(mia), &["mia"], None, None).await;
    env.computer(tenant, Some(philip), &["philip"], Some("philip"), None)
        .await;
    sqlx::query("UPDATE devices SET agent_features = NULL")
        .execute(&env.st.db)
        .await
        .unwrap();

    let s = env.start("Mia", VERIFIER).await;
    assert!(env.sent_code(id_of(&s)).await.is_none());
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE type = 'login_code'")
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(queued, 0);
    // A parent is told plainly instead of waiting for a code that never comes.
    let (jar, me) = session_for(&env, philip, tenant).await;
    let status = crate::confirm::status(State(env.st.clone()), me.clone(), jar.clone())
        .await
        .unwrap()
        .0;
    assert_eq!(status["computer"], false);
    assert!(matches!(
        crate::confirm::code_start(State(env.st.clone()), me, jar).await,
        Err(AppError::Conflict(m)) if m.contains("too old")
    ));

    // An agent that says it can (in its heartbeat) is sent codes again…
    let agent = crate::state::AgentAuth {
        device_id: laptop,
        tenant_id: tenant,
    };
    let beat = |features: Value| {
        let req = serde_json::from_value(serde_json::json!({ "features": features })).unwrap();
        crate::agent::heartbeat(State(env.st.clone()), agent, Json(req))
    };
    let _ = beat(serde_json::json!(["login_code"])).await.unwrap();
    let s = env.start("Mia", VERIFIER).await;
    assert!(env.sent_code(id_of(&s)).await.is_some());

    // …and should one reach an agent that fails it ("unknown command"), the
    // failure settles it: nothing is left pending to redeliver.
    let pulled = crate::agent::pull_pending_commands(&env.st.db, laptop)
        .await
        .unwrap();
    let id: Uuid = pulled[0]["id"].as_str().unwrap().parse().unwrap();
    let _ = crate::agent::ack_command(
        State(env.st.clone()),
        agent,
        axum::extract::Path(id),
        Json(crate::agent::AckReq {
            status: "failed".into(),
            result: Some(serde_json::json!({ "error": "unknown command 'login_code'" })),
        }),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE commands SET sent_at = now() - interval '10 minutes'")
        .execute(&env.st.db)
        .await
        .unwrap();
    assert!(crate::agent::pull_pending_commands(&env.st.db, laptop)
        .await
        .unwrap()
        .is_empty());
    // A heartbeat that declares nothing is an old agent's again.
    let _ = beat(Value::Null).await.unwrap();
    let s = env.start("Mia", VERIFIER).await;
    assert!(env.sent_code(id_of(&s)).await.is_none());
    env.drop_db().await;
}
