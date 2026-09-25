//! Database-backed tests for the numbers a family sees: time left on the
//! console (the same verdict the computer shows, override included), asks
//! that a grant answers, and Who's who handing a login its person's own
//! rules. Each reproduces a failure from the end-to-end acceptance run.
//! Same harness and skip rule as `tests_auth`.

use axum::extract::{Path, State};
use axum::Json;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::state::{AgentAuth, AuthAdmin};
use crate::tests_auth::{session_for, Env};

fn clone(a: &AuthAdmin) -> AuthAdmin {
    AuthAdmin {
        admin_id: a.admin_id,
        tenant_id: a.tenant_id,
        role: a.role.clone(),
        blocked: a.blocked,
    }
}

async fn du_of(env: &Env, device: Uuid, login: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM device_users WHERE device_id = $1 AND os_username = $2")
        .bind(device)
        .bind(login)
        .fetch_one(&env.st.db)
        .await
        .unwrap()
}

async fn set_limit(env: &Env, person: Uuid, minutes: i64) {
    sqlx::query(
        "UPDATE profiles SET policy = jsonb_set(jsonb_set(jsonb_set(policy,
                  '{screen_time,daily_limit_minutes}', to_jsonb($2::bigint)),
                  '{screen_time,schedule}', '[]'::jsonb),
                  '{screen_time,bedtime}', 'null'::jsonb)
          WHERE id = (SELECT profile_id FROM admins WHERE id = $1)",
    )
    .bind(person)
    .bind(minutes)
    .execute(&env.st.db)
    .await
    .unwrap();
}

async fn used_today(env: &Env, du: Uuid, secs: i32) {
    let day = crate::ledger::local_today(None, Utc::now());
    sqlx::query(
        "INSERT INTO screen_time_ledger (device_user_id, day, used_seconds) VALUES ($1, $2, $3)
         ON CONFLICT (device_user_id, day) DO UPDATE SET used_seconds = $3",
    )
    .bind(du)
    .bind(day)
    .bind(secs)
    .execute(&env.st.db)
    .await
    .unwrap();
}

async fn family_card(env: &Env, hub: &AuthAdmin, name: &str) -> Value {
    let f = crate::family::get_family(State(env.st.clone()), clone(hub))
        .await
        .unwrap()
        .0;
    f["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("{name} is not on the Family page: {f}"))
}

/// Acceptance, step 5: after the unlock code the console said "time's up /
/// 0 min left" while Mia was unlocked for 30 minutes — it computed its own
/// number and knew nothing of the override. It now runs the same rules
/// function with the override her computer reports.
#[tokio::test]
async fn the_console_counts_the_override_the_computer_reports() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    set_limit(&env, mia, 5).await;
    let laptop = env.computer(tenant, Some(mia), &["mia"], None, None).await;
    let du = du_of(&env, laptop, "mia").await;
    used_today(&env, du, 6 * 60).await;
    let (_, hub) = session_for(&env, philip, tenant).await;

    // By the rules alone: time's up.
    let card = family_card(&env, &hub, "Mia").await;
    assert_eq!(card["left_minutes"], 0);
    assert_eq!(card["rules"]["allowed"], false);

    // The computer reports the code's override in its state frame.
    let until = Utc::now() + Duration::minutes(29) + Duration::seconds(30);
    sqlx::query("UPDATE devices SET last_state = $2 WHERE id = $1")
        .bind(laptop)
        .bind(json!({ "locked": false, "overrides": { "mia": until } }))
        .execute(&env.st.db)
        .await
        .unwrap();
    let card = family_card(&env, &hub, "Mia").await;
    assert_eq!(card["left_minutes"], 30, "{card}");
    assert_eq!(card["rules"]["allowed"], true);
    assert!(card["rules"]["override_until"].is_string());
    assert_eq!(card["rules"]["utc_offset_secs"], 0);

    // An override that ended counts for nothing.
    sqlx::query("UPDATE devices SET last_state = $2 WHERE id = $1")
        .bind(laptop)
        .bind(json!({ "locked": false,
                      "overrides": { "mia": Utc::now() - Duration::minutes(1) } }))
        .execute(&env.st.db)
        .await
        .unwrap();
    assert_eq!(family_card(&env, &hub, "Mia").await["left_minutes"], 0);
    env.drop_db().await;
}

async fn ask(env: &Env, tenant: Uuid, device: Uuid, task: &str, label: &str) -> Uuid {
    let r = crate::earn::create_request(
        State(env.st.clone()),
        AgentAuth {
            device_id: device,
            tenant_id: tenant,
        },
        Json(crate::earn::EarnRequestReq {
            os_username: "mia".into(),
            task_id: task.into(),
            task_label: label.into(),
            minutes: 15,
        }),
    )
    .await
    .unwrap()
    .0;
    r["request"]["id"].as_str().unwrap().parse().unwrap()
}

async fn status_of(env: &Env, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM earn_requests WHERE id = $1")
        .bind(id)
        .fetch_one(&env.st.db)
        .await
        .unwrap()
}

/// Acceptance, step 6a: Mia asked for more time; the parent pressed Give 15
/// on her — and her ask stayed pending on the console (and "waiting" on her
/// computer). Giving someone time answers their asks.
#[tokio::test]
async fn giving_time_answers_the_ask() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    let laptop = env.computer(tenant, Some(mia), &["mia"], None, None).await;
    let du = du_of(&env, laptop, "mia").await;
    let (_, hub) = session_for(&env, philip, tenant).await;

    let asked = ask(&env, tenant, laptop, "ask", "Asked for more time").await;
    let card = family_card(&env, &hub, "Mia").await;
    assert_eq!(card["pending_requests"], 1);

    let r = crate::earn::credit_time(
        State(env.st.clone()),
        clone(&hub),
        Path(du),
        Json(crate::earn::CreditTimeReq { minutes: 15 }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(r["answered"], json!([asked]));
    assert_eq!(status_of(&env, asked).await, "approved");
    let card = family_card(&env, &hub, "Mia").await;
    assert_eq!(card["pending_requests"], 0, "the ask is answered");
    // Her computer hears it (credit_time clears the ask and tells her), once.
    let grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE device_id = $1 AND type = 'credit_time'",
    )
    .bind(laptop)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    assert_eq!(grants, 1);

    // Two asks waiting: answering one answers both, and credits once.
    let a = ask(&env, tenant, laptop, "ask", "Asked for more time").await;
    let b = ask(&env, tenant, laptop, "reading", "Read for 20 min").await;
    let _ = crate::earn::approve_request(State(env.st.clone()), clone(&hub), Path(b))
        .await
        .unwrap();
    assert_eq!(status_of(&env, a).await, "approved");
    assert_eq!(status_of(&env, b).await, "approved");
    let grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE device_id = $1 AND type = 'credit_time'",
    )
    .bind(laptop)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    assert_eq!(grants, 2, "one more grant, not two");
    env.drop_db().await;
}

async fn policy_of_login(env: &Env, du: Uuid) -> Value {
    sqlx::query_scalar(
        "SELECT p.policy FROM device_users du JOIN profiles p ON p.id = du.profile_id
          WHERE du.id = $1",
    )
    .bind(du)
    .fetch_one(&env.st.db)
    .await
    .unwrap()
}

async fn exists(env: &Env, person: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM admins WHERE id = $1)")
        .bind(person)
        .fetch_one(&env.st.db)
        .await
        .unwrap()
}

/// Acceptance, Who's who: on Mia's computer the `philip` login was created
/// as a person of its own ("philip", Kid, child rules). Pointing it at
/// Philip kept the child rules — 60 minutes and a bedtime on the parent —
/// and left the made-up "philip" on the Family page. The login now takes
/// Philip's own rules (none: no limits), and the leftover goes.
#[tokio::test]
async fn whos_who_gives_a_parent_their_own_rules_and_drops_the_guess() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    let laptop = env
        .computer(
            tenant,
            Some(mia),
            &["mia", "philip"],
            Some("mia"),
            Some("mia"),
        )
        .await;
    let du = du_of(&env, laptop, "philip").await;
    let guess: Uuid = sqlx::query_scalar("SELECT account_id FROM device_users WHERE id = $1")
        .bind(du)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_ne!(guess, philip);
    assert_eq!(
        policy_of_login(&env, du).await["screen_time"]["daily_limit_minutes"],
        60,
        "an unsorted login on a child's computer starts on child rules"
    );
    let (_, hub) = session_for(&env, philip, tenant).await;
    family_card(&env, &hub, "philip").await;

    let r = crate::devices::assign_account(
        State(env.st.clone()),
        clone(&hub),
        Path(du),
        Json(crate::devices::AssignAccountReq { account_id: philip }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(r["removed_person"], true);

    // Philip's own rules: an adult's, which enforce nothing.
    let p = policy_of_login(&env, du).await;
    let policy: openscreentime_policy::Policy = serde_json::from_value(p).unwrap();
    assert!(crate::members::limit_minutes(&policy).is_none());
    assert!(policy.screen_time.bedtime.is_none() || !policy.screen_time.enabled);
    let own: Option<Uuid> = sqlx::query_scalar("SELECT profile_id FROM admins WHERE id = $1")
        .bind(philip)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    let on: Uuid = sqlx::query_scalar("SELECT profile_id FROM device_users WHERE id = $1")
        .bind(du)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(own, Some(on), "the login is on Philip's own rules");
    // His own page says so too.
    let (_, me) = session_for(&env, philip, tenant).await;
    let today = crate::members::today(State(env.st.clone()), me)
        .await
        .unwrap()
        .0;
    assert_eq!(today["limit_minutes"], Value::Null);

    // The made-up "philip" is gone from the family.
    assert!(!exists(&env, guess).await);
    let f = crate::family::get_family(State(env.st.clone()), clone(&hub))
        .await
        .unwrap()
        .0;
    assert!(f["children"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["name"] != "philip"));
    env.drop_db().await;
}

/// A leftover a parent already made their own (renamed, given a face, their
/// rules changed) is a person, not a guess: kept, with no computer.
#[tokio::test]
async fn whos_who_keeps_a_person_someone_made_their_own() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    let laptop = env
        .computer(tenant, Some(mia), &["mia", "sam"], Some("mia"), Some("mia"))
        .await;
    let du = du_of(&env, laptop, "sam").await;
    let sam: Uuid = sqlx::query_scalar("SELECT account_id FROM device_users WHERE id = $1")
        .bind(du)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    sqlx::query("UPDATE admins SET display_name = 'Sam' WHERE id = $1")
        .bind(sam)
        .execute(&env.st.db)
        .await
        .unwrap();
    let (_, hub) = session_for(&env, philip, tenant).await;
    let r = crate::devices::assign_account(
        State(env.st.clone()),
        clone(&hub),
        Path(du),
        Json(crate::devices::AssignAccountReq { account_id: mia }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(r["removed_person"], false);
    assert!(exists(&env, sam).await);
    let card = family_card(&env, &hub, "Sam").await;
    assert_eq!(card["devices"], json!([]), "kept, with no computer");
    // The login is on Mia's rules now.
    let mias: Uuid = sqlx::query_scalar("SELECT profile_id FROM admins WHERE id = $1")
        .bind(mia)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    let on: Uuid = sqlx::query_scalar("SELECT profile_id FROM device_users WHERE id = $1")
        .bind(du)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(on, mias);
    env.drop_db().await;
}
