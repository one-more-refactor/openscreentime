//! One computer, one record, and a person's day that outlives the record
//! (`machine.rs`). Each test reproduces a failure from the second acceptance
//! run on a real Debian GNOME computer:
//!
//! * re-running the install one-liner with a new token doubled today's time
//!   ("Mia 32 → 64, Philip 25 → 51");
//! * removing a computer wiped the person's day ("17 min left of 17" after
//!   using 37).
//!
//! Database-backed, same harness and skip rule as `tests_auth`.

use axum::extract::{FromRequestParts, Path, State};
use axum::Json;
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppError;
use crate::ledger::{local_today, upsert_usage, UsageEntry};
use crate::state::{AgentAuth, AuthAdmin};
use crate::tests_auth::{session_for, Env};

/// What `/etc/machine-id` hashes to on the computer in these tests.
const MACHINE: &str = "3f1b9c0e8a7d6c5b4a39281706f5e4d3c2b1a09f8e7d6c5b4a3928170f5e4d3c";

fn clone(a: &AuthAdmin) -> AuthAdmin {
    AuthAdmin {
        admin_id: a.admin_id,
        tenant_id: a.tenant_id,
        role: a.role.clone(),
        blocked: a.blocked,
    }
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

/// "Add a computer" in the console: a pending record for `owner`, and the
/// enroll token the one-liner carries.
async fn new_record(env: &Env, tenant: Uuid, name: &str, owner: Uuid) -> (Uuid, String) {
    let token = crate::auth::gen_token();
    let id = sqlx::query_scalar(
        "INSERT INTO devices (tenant_id, name, status, enroll_token, enroll_token_expires_at,
                              owner_account_id)
         VALUES ($1, $2, 'pending', $3, now() + interval '1 day', $4) RETURNING id",
    )
    .bind(tenant)
    .bind(name)
    .bind(crate::auth::hash_token(&token))
    .bind(owner)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    (id, token)
}

/// The one-liner on the computer: `openscreentime enroll`. Returns the
/// device token the agent is handed.
async fn run_one_liner(
    env: &Env,
    token: &str,
    logins: &[&str],
    owner_login: &str,
    machine: Option<&str>,
) -> String {
    let req = serde_json::from_value(json!({
        "enroll_token": token,
        "hostname": "debian",
        "os": "linux",
        "os_users": logins.iter().map(|l| json!({ "username": l })).collect::<Vec<_>>(),
        "owner_login": owner_login,
        "machine_id": machine,
    }))
    .unwrap();
    let r = crate::agent::enroll(State(env.st.clone()), Json(req))
        .await
        .unwrap()
        .0;
    r["device_token"].as_str().unwrap().to_string()
}

/// The agent's usage report: `login` used `minutes` today.
async fn report(env: &Env, tenant: Uuid, device: Uuid, login: &str, minutes: i64) -> Value {
    let day = local_today(None, Utc::now());
    let answer = upsert_usage(
        &env.st.db,
        tenant,
        device,
        &[UsageEntry {
            os_username: login.into(),
            used_minutes_today: minutes,
            used_seconds_today: Some(minutes * 60),
            day: Some(day),
            utc_offset_secs: Some(0),
        }],
    )
    .await
    .unwrap();
    answer[0].clone()
}

async fn du_of(env: &Env, device: Uuid, login: &str) -> Option<(Uuid, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT id, account_id FROM device_users WHERE device_id = $1 AND os_username = $2",
    )
    .bind(device)
    .bind(login)
    .fetch_optional(&env.st.db)
    .await
    .unwrap()
}

async fn family(env: &Env, hub: &AuthAdmin) -> Value {
    crate::family::get_family(State(env.st.clone()), clone(hub))
        .await
        .unwrap()
        .0
}

async fn card(env: &Env, hub: &AuthAdmin, name: &str) -> Value {
    let f = family(env, hub).await;
    f["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("{name} is not on the Family page: {f}"))
}

async fn agent_auth(env: &Env, token: &str) -> Result<AgentAuth, AppError> {
    let (mut parts, _) = axum::http::Request::builder()
        .uri("/agent/heartbeat")
        .header("authorization", format!("Bearer {token}"))
        .body(())
        .unwrap()
        .into_parts();
    AgentAuth::from_request_parts(&mut parts, &env.st).await
}

async fn remove(env: &Env, hub: &AuthAdmin, device: Uuid) {
    let _ = crate::devices::delete_device(State(env.st.clone()), clone(hub), Path(device))
        .await
        .unwrap();
}

async fn exists(env: &Env, device: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM devices WHERE id = $1)")
        .bind(device)
        .fetch_one(&env.st.db)
        .await
        .unwrap()
}

/// Acceptance 7c: the one-liner for "Philip's computer", run on the machine
/// that was "Mia's computer". Before: the old record kept Mia's 32 and the
/// agent reported them again under the new one — 64 on the console, and 32
/// "used elsewhere" handed back to the agent on top of its own count.
#[tokio::test]
async fn the_same_machine_enrolled_again_is_counted_once() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    set_limit(&env, mia, 60).await;
    let (_, hub) = session_for(&env, philip, tenant).await;

    // Mia's computer: her login and one for Philip (a person of its own).
    let (old, token) = new_record(&env, tenant, "Mia's computer", mia).await;
    let old_token = run_one_liner(&env, &token, &["mia", "philip"], "mia", Some(MACHINE)).await;
    report(&env, tenant, old, "mia", 32).await;
    report(&env, tenant, old, "philip", 25).await;
    let made_up = du_of(&env, old, "philip").await.unwrap().1.unwrap();
    assert_ne!(made_up, philip);

    // "Add my computer", and the one-liner on the same machine.
    let (new, token) = new_record(&env, tenant, "Philip's computer", philip).await;
    let new_token = run_one_liner(&env, &token, &["mia", "philip"], "philip", Some(MACHINE)).await;

    // One record for one machine: the old one is folded in and gone.
    assert!(!exists(&env, old).await, "the old record should be gone");
    assert!(exists(&env, new).await);
    let computers = family(&env, &hub).await["devices"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(computers, 1);

    // Mia's day is her 32 minutes, once — on the console…
    let c = card(&env, &hub, "Mia").await;
    assert_eq!(c["used_minutes"], 32, "{c}");
    assert_eq!(c["left_minutes"], 28, "{c}");
    // …and for the agent, whose own ledger holds them: nothing "elsewhere".
    let answer = report(&env, tenant, new, "mia", 33).await;
    assert_eq!(answer["used_elsewhere_secs"], 0, "{answer}");
    assert_eq!(card(&env, &hub, "Mia").await["used_minutes"], 33);

    // The philip login is Philip's now, with its 25 minutes; the person made
    // up for it on the old record is gone with it.
    let (_, who) = du_of(&env, new, "philip").await.unwrap();
    assert_eq!(who, Some(philip));
    let (_, me) = session_for(&env, philip, tenant).await;
    let today = crate::members::today(State(env.st.clone()), me)
        .await
        .unwrap()
        .0;
    assert_eq!(today["used_minutes"], 25, "{today}");
    let left: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM admins WHERE id = $1)")
        .bind(made_up)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert!(!left, "the made-up philip person should be gone");

    // The agent still running with the old token (until the installer
    // restarts it) is heard as the new record — never told it was removed,
    // which would make it take itself off the computer.
    let heard = agent_auth(&env, &old_token).await.unwrap();
    assert_eq!(heard.device_id, new);
    assert_eq!(agent_auth(&env, &new_token).await.unwrap().device_id, new);

    // The enrollment says what it replaced.
    let took: Value = sqlx::query_scalar(
        "SELECT payload->'took_over' FROM events WHERE type = 'enrolled' AND device_id = $1
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(new)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    assert_eq!(took, json!(["Mia's computer"]));

    // Removing the computer frees it, whichever token its agent holds.
    remove(&env, &hub, new).await;
    for t in [&old_token, &new_token] {
        let err = agent_auth(&env, t).await.err().expect("retired");
        assert!(matches!(err, AppError::DeviceRetired(_)), "{err:?}");
    }
    env.drop_db().await;
}

/// A login a parent sorted by hand (Who's who) stays that person when the
/// machine is enrolled again — its minutes don't move to a stranger.
#[tokio::test]
async fn a_sorted_login_stays_its_person() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    let (_, hub) = session_for(&env, philip, tenant).await;
    let (old, token) = new_record(&env, tenant, "Family computer", mia).await;
    run_one_liner(&env, &token, &["mia", "kiddo"], "mia", Some(MACHINE)).await;
    // `kiddo` is Mia too, a parent said.
    let (du, _) = du_of(&env, old, "kiddo").await.unwrap();
    let _ = crate::devices::assign_account(
        State(env.st.clone()),
        clone(&hub),
        Path(du),
        Json(serde_json::from_value(json!({ "account_id": mia })).unwrap()),
    )
    .await
    .unwrap();
    report(&env, tenant, old, "kiddo", 20).await;

    let (new, token) = new_record(&env, tenant, "Family computer", mia).await;
    run_one_liner(&env, &token, &["mia", "kiddo"], "mia", Some(MACHINE)).await;
    assert_eq!(du_of(&env, new, "kiddo").await.unwrap().1, Some(mia));
    assert_eq!(card(&env, &hub, "Mia").await["used_minutes"], 20);
    let people: i64 =
        sqlx::query_scalar("SELECT count(*) FROM admins WHERE tenant_id = $1 AND role = 'member'")
            .bind(tenant)
            .fetch_one(&env.st.db)
            .await
            .unwrap();
    assert_eq!(people, 1, "no made-up kiddo person");
    env.drop_db().await;
}

/// Only the same machine in the same household is folded in: an agent that
/// doesn't say which machine it is (older than this) and another household's
/// record of an identical hash are left alone.
#[tokio::test]
async fn only_the_same_machine_in_the_same_household_is_folded_in() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    let (other, _) =
        crate::auth::create_tenant_with_admin(&env.st.db, None, "stranger", "Stranger", false)
            .await
            .unwrap();
    let theo = env.member(other, "Theo").await;

    let (a, token) = new_record(&env, tenant, "Mia's computer", mia).await;
    run_one_liner(&env, &token, &["mia"], "mia", Some(MACHINE)).await;
    let (b, token) = new_record(&env, other, "Theo's computer", theo).await;
    run_one_liner(&env, &token, &["theo"], "theo", Some(MACHINE)).await;
    let (c, token) = new_record(&env, tenant, "Old agent", philip).await;
    run_one_liner(&env, &token, &["philip"], "philip", None).await;
    for d in [a, b, c] {
        assert!(exists(&env, d).await);
    }
    env.drop_db().await;
}

/// The enroll preview hands out the household's salt: the same for every
/// token of one household, different in another — so the agent's machine
/// identity matches nothing outside its household.
#[tokio::test]
async fn the_machine_salt_is_per_household() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let (other, stranger) =
        crate::auth::create_tenant_with_admin(&env.st.db, None, "stranger", "Stranger", false)
            .await
            .unwrap();
    async fn salt(env: &Env, token: String) -> String {
        let r = crate::agent::enroll_preview(
            State(env.st.clone()),
            Json(serde_json::from_value(json!({ "enroll_token": token })).unwrap()),
        )
        .await
        .unwrap()
        .0;
        r["machine_salt"].as_str().unwrap().to_string()
    }
    let one = salt(&env, new_record(&env, tenant, "a", philip).await.1).await;
    let two = salt(&env, new_record(&env, tenant, "b", philip).await.1).await;
    let three = salt(&env, new_record(&env, other, "c", stranger).await.1).await;
    assert_eq!(one, two);
    assert_ne!(one, three);
    assert!(one.len() >= 32);
    env.drop_db().await;
}

/// Acceptance 11: removing the stale "Mia's computer" record left Mia at
/// "17 min left of 17" — the 37 minutes she used today went with it. Her day
/// stays hers: on the console, for her other computers, in her week — and
/// when the same machine comes back, it isn't counted twice.
#[tokio::test]
async fn removing_a_computer_keeps_the_persons_day() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    set_limit(&env, mia, 17).await;
    let (_, hub) = session_for(&env, philip, tenant).await;

    let (laptop, token) = new_record(&env, tenant, "Mia's computer", mia).await;
    run_one_liner(&env, &token, &["mia"], "mia", Some(MACHINE)).await;
    report(&env, tenant, laptop, "mia", 37).await;
    let c = card(&env, &hub, "Mia").await;
    assert_eq!(
        (c["used_minutes"].clone(), c["left_minutes"].clone()),
        (json!(37), json!(0))
    );

    remove(&env, &hub, laptop).await;
    let c = card(&env, &hub, "Mia").await;
    assert_eq!(c["used_minutes"], 37, "her day went with the computer: {c}");
    assert_eq!(c["left_minutes"], 0, "{c}");
    assert_eq!(c["devices"], json!([]));

    // Another computer of hers counts them as used elsewhere…
    let (desktop, token) = new_record(&env, tenant, "Desktop", mia).await;
    run_one_liner(&env, &token, &["mia"], "mia", Some(&"b".repeat(64))).await;
    let answer = report(&env, tenant, desktop, "mia", 1).await;
    assert_eq!(answer["used_elsewhere_secs"], 37 * 60, "{answer}");
    assert_eq!(card(&env, &hub, "Mia").await["used_minutes"], 38);
    // …and her week has them.
    let (_, her) = session_for(&env, mia, tenant).await;
    let week = crate::members::history(State(env.st.clone()), her)
        .await
        .unwrap()
        .0;
    let today = week["days"].as_array().unwrap().last().cloned().unwrap();
    assert_eq!(today["used_minutes"], 38, "{week}");
    let names: Vec<&str> = week["today_by_device"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Mia's computer", "Desktop"]);

    // The same machine enrolled again: its agent's ledger still has the 37,
    // and the kept minutes go back onto its login instead of on top.
    let (again, token) = new_record(&env, tenant, "Mia's computer", mia).await;
    run_one_liner(&env, &token, &["mia"], "mia", Some(MACHINE)).await;
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM retired_usage")
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(kept, 0);
    report(&env, tenant, again, "mia", 37).await;
    assert_eq!(card(&env, &hub, "Mia").await["used_minutes"], 38);
    env.drop_db().await;
}
