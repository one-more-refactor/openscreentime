//! Database-backed tests for sign-in, confirm, recovery and OS-login linking.
//!
//! They need a Postgres they may create databases in: `DATABASE_URL` (CI sets
//! it). Each test makes its own throwaway database and drops it at the end;
//! without `DATABASE_URL` they skip with a note instead of failing.

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
        let Ok(url) = std::env::var("DATABASE_URL") else {
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
        crate::db::migrate(&db).await.unwrap();

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
        let device: Uuid = sqlx::query_scalar(
            "INSERT INTO devices (tenant_id, name, status, owner_account_id, last_seen)
             VALUES ($1, 'a computer', 'online', $2, now()) RETURNING id",
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
    // The child's login there is a person of its own, not the parent.
    let (leo, role, bracket) = env.linked_to(desk, "leo").await;
    assert_ne!(leo, philip);
    assert_eq!((role.as_str(), bracket.as_str()), ("member", "kid"));
    env.drop_db().await;
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
