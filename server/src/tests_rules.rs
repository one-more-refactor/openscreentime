//! Database-backed tests for a person's own rules (`/api/me/rules`), what the
//! hub may see of them, and the startup backfill that opens legacy
//! closed-network profiles. Same harness and skip rule as `tests_auth`.

use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::{AgentAuth, AuthAdmin};
use crate::tests_auth::{session_for, Env};

async fn adult_member(env: &Env, tenant: Uuid, name: &str) -> (Uuid, Uuid) {
    let pid = crate::members::create_profile_for(
        &env.st.db,
        tenant,
        openscreentime_policy::AgeBracket::Adult,
        name,
    )
    .await
    .unwrap();
    let id = sqlx::query_scalar(
        "INSERT INTO admins (tenant_id, display_name, role, age_bracket, self_managed, profile_id)
         VALUES ($1, $2, 'member', 'adult', true, $3) RETURNING id",
    )
    .bind(tenant)
    .bind(name)
    .bind(pid)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    (id, pid)
}

async fn get_rules(env: &Env, who: &AuthAdmin) -> Result<Value, AppError> {
    crate::members::my_rules(State(env.st.clone()), clone(who))
        .await
        .map(|j| j.0)
}

async fn put_rules(env: &Env, who: &AuthAdmin, body: Value) -> Result<Value, AppError> {
    crate::members::set_my_rules(State(env.st.clone()), clone(who), Json(body))
        .await
        .map(|j| j.0)
}

fn clone(a: &AuthAdmin) -> AuthAdmin {
    AuthAdmin {
        admin_id: a.admin_id,
        tenant_id: a.tenant_id,
        role: a.role.clone(),
        blocked: a.blocked,
    }
}

fn is_bad(r: &Result<Value, AppError>) -> bool {
    matches!(r, Err(AppError::BadRequest(_)))
}

fn is_forbidden(r: &Result<Value, AppError>) -> bool {
    matches!(r, Err(AppError::ForbiddenForMember(_)))
}

#[tokio::test]
async fn the_hub_keeps_their_own_rules_and_their_computer_enforces_them() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let laptop = env
        .computer(tenant, Some(philip), &["philip"], Some("philip"), None)
        .await;
    let (_, me) = session_for(&env, philip, tenant).await;

    // Nothing set yet: no limit, no focus hours, no sites.
    let none = get_rules(&env, &me).await.unwrap();
    assert_eq!(
        none,
        json!({ "daily_limit_minutes": 0, "focus_hours": null, "sites": [] })
    );

    let saved = put_rules(
        &env,
        &me,
        json!({ "daily_limit_minutes": 180,
                "focus_hours": { "days": [1,2,3,4,5], "start": "09:00", "end": "12:00" },
                "sites": [" Reddit.com ", "reddit.com", "YouTube.com."] }),
    )
    .await
    .unwrap();
    assert_eq!(
        saved,
        json!({ "daily_limit_minutes": 180,
                "focus_hours": { "days": [1,2,3,4,5], "start": "09:00", "end": "12:00" },
                "sites": ["reddit.com", "youtube.com"] })
    );
    assert_eq!(get_rules(&env, &me).await.unwrap(), saved);

    // The computer is told to re-pull, and what it pulls is these rules.
    let told: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE device_id = $1 AND type = 'apply_policy'",
    )
    .bind(laptop)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    assert!(told >= 1);
    let pulled = crate::agent::policy(
        State(env.st.clone()),
        AgentAuth {
            device_id: laptop,
            tenant_id: tenant,
        },
    )
    .await
    .unwrap()
    .0;
    let mine = pulled["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["os_username"] == "philip")
        .expect("philip's login is served");
    assert_eq!(mine["profile_kind"], "adult");
    assert_eq!(mine["policy"]["screen_time"]["enabled"], true);
    assert_eq!(mine["policy"]["screen_time"]["daily_limit_minutes"], 180);
    assert_eq!(
        mine["policy"]["focus"]["sites"],
        json!(["reddit.com", "youtube.com"])
    );
    assert_eq!(mine["policy"]["focus"]["hours"]["start"], "09:00");

    // The trail says it changed, never which sites.
    let trail: Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE type = 'member'
            AND payload->>'action' = 'own_rules_changed' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    assert_eq!(trail["sites"], 2);
    assert!(!trail.to_string().contains("reddit"), "{trail}");

    // "Any time" (no hours) and a window across midnight are both fine; the
    // limit can go back to none.
    let any = put_rules(
        &env,
        &me,
        json!({ "daily_limit_minutes": 0, "focus_hours": null, "sites": ["reddit.com"] }),
    )
    .await
    .unwrap();
    assert_eq!(any["focus_hours"], Value::Null);
    assert_eq!(any["daily_limit_minutes"], 0);
    assert!(put_rules(
        &env,
        &me,
        json!({ "daily_limit_minutes": 60,
                "focus_hours": { "days": [5], "start": "22:00", "end": "02:00" },
                "sites": [] }),
    )
    .await
    .is_ok());
    assert!(put_rules(
        &env,
        &me,
        json!({ "daily_limit_minutes": 60,
                "focus_hours": { "days": [1], "start": "18:00", "end": "00:00" },
                "sites": [] }),
    )
    .await
    .is_ok());

    // What can't mean anything is refused, plainly, and nothing changes.
    for bad in [
        json!({ "daily_limit_minutes": 60,
                "focus_hours": { "days": [1], "start": "15:00", "end": "15:00" }, "sites": [] }),
        json!({ "daily_limit_minutes": 60,
                "focus_hours": { "days": [], "start": "09:00", "end": "12:00" }, "sites": [] }),
        json!({ "daily_limit_minutes": 60,
                "focus_hours": { "days": [1], "start": "late", "end": "12:00" }, "sites": [] }),
        json!({ "daily_limit_minutes": 60, "focus_hours": null, "sites": ["not a domain"] }),
        json!({ "daily_limit_minutes": 60, "focus_hours": null, "sites": ["nodot"] }),
        json!({ "daily_limit_minutes": 2000, "focus_hours": null, "sites": [] }),
        json!({ "daily_limit_minutes": -5, "focus_hours": null, "sites": [] }),
        json!({ "daily_limit_minutes": "lots", "focus_hours": null, "sites": [] }),
    ] {
        assert!(is_bad(&put_rules(&env, &me, bad.clone()).await), "{bad}");
    }
    let still = get_rules(&env, &me).await.unwrap();
    assert_eq!(still["focus_hours"]["end"], "00:00");
    env.drop_db().await;
}

#[tokio::test]
async fn an_adults_rules_are_theirs_and_the_hub_cannot_see_them() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let (jonas, jonas_rules) = adult_member(&env, tenant, "Jonas").await;
    env.computer(tenant, Some(jonas), &["jonas"], None, None)
        .await;
    let (_, as_jonas) = session_for(&env, jonas, tenant).await;
    let (_, as_hub) = session_for(&env, philip, tenant).await;

    let saved = put_rules(
        &env,
        &as_jonas,
        json!({ "daily_limit_minutes": 120, "focus_hours": null, "sites": ["news.ycombinator.com"] }),
    )
    .await
    .unwrap();
    assert_eq!(saved["daily_limit_minutes"], 120);
    // His own page says so.
    let today = crate::members::today(State(env.st.clone()), clone(&as_jonas))
        .await
        .unwrap()
        .0;
    assert_eq!(today["self_managed"], true);
    // …and tells him what the hub sees: his minutes, nothing more.
    assert_eq!(
        today["parent_sees"],
        json!({ "apps": false, "sites": false })
    );
    assert_eq!(today["limit_minutes"], 120);
    assert_eq!(today["focus"]["sites"], json!(["news.ycombinator.com"]));

    // The hub can't read or change them…
    let got =
        crate::profiles::get_profile(State(env.st.clone()), clone(&as_hub), Path(jonas_rules))
            .await
            .map(|j| j.0);
    assert!(is_forbidden(&got));
    let put = crate::profiles::update_profile(
        State(env.st.clone()),
        clone(&as_hub),
        Path(jonas_rules),
        Json(serde_json::from_value(json!({ "policy": {} })).unwrap()),
    )
    .await
    .map(|j| j.0);
    assert!(is_forbidden(&put));
    let del =
        crate::profiles::delete_profile(State(env.st.clone()), clone(&as_hub), Path(jonas_rules))
            .await
            .map(|j| j.0);
    assert!(is_forbidden(&del));
    let listed = crate::profiles::list_profiles(State(env.st.clone()), clone(&as_hub))
        .await
        .unwrap()
        .0;
    assert!(!listed.to_string().contains(&jonas_rules.to_string()));

    // …and the family page shows his minutes, not his rules.
    let fam = crate::family::get_family(State(env.st.clone()), clone(&as_hub))
        .await
        .unwrap()
        .0;
    assert!(!fam["profiles"]
        .to_string()
        .contains(&jonas_rules.to_string()));
    assert!(!fam.to_string().contains("ycombinator"));
    let card = fam["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == jonas.to_string())
        .unwrap();
    assert_eq!(card["limit_minutes"], Value::Null);
    assert_eq!(card["left_minutes"], Value::Null);
    assert_eq!(card["rules"], Value::Null);
    assert_eq!(card["used_minutes"], 0);

    // A child's rules are a parent's: the page, and the hub's view of them,
    // are unchanged.
    let mia = env.member(tenant, "Mia").await;
    let (_, as_mia) = session_for(&env, mia, tenant).await;
    assert!(is_forbidden(&get_rules(&env, &as_mia).await));
    assert!(is_forbidden(
        &put_rules(
            &env,
            &as_mia,
            json!({ "daily_limit_minutes": 0, "focus_hours": null, "sites": [] }),
        )
        .await
    ));
    let today = crate::members::today(State(env.st.clone()), clone(&as_mia))
        .await
        .unwrap()
        .0;
    assert_eq!(today["self_managed"], false);
    assert_eq!(today["parent_sees"], json!({ "apps": true, "sites": true }));
    let fam = crate::family::get_family(State(env.st.clone()), clone(&as_hub))
        .await
        .unwrap()
        .0;
    let card = fam["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == mia.to_string())
        .unwrap();
    assert_eq!(card["limit_minutes"], 60);
    env.drop_db().await;
}

async fn profile_as(env: &Env, who: &AuthAdmin, id: Uuid) -> Result<Value, AppError> {
    crate::profiles::get_profile(State(env.st.clone()), clone(who), Path(id))
        .await
        .map(|j| j.0)
}

async fn edit_profile_as(env: &Env, who: &AuthAdmin, id: Uuid) -> Result<Value, AppError> {
    crate::profiles::update_profile(
        State(env.st.clone()),
        clone(who),
        Path(id),
        Json(serde_json::from_value(json!({ "name": "renamed" })).unwrap()),
    )
    .await
    .map(|j| j.0)
}

async fn rules_id(env: &Env, account: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT profile_id FROM admins WHERE id = $1")
        .bind(account)
        .fetch_one(&env.st.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_parents_own_rules_are_theirs_even_from_another_parent() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    // A second parent in the household, with rules of her own.
    let anna_pid = crate::members::create_profile_for(
        &env.st.db,
        tenant,
        openscreentime_policy::AgeBracket::Adult,
        "Anna",
    )
    .await
    .unwrap();
    let anna: Uuid = sqlx::query_scalar(
        "INSERT INTO admins (tenant_id, display_name, role, age_bracket, profile_id)
         VALUES ($1, 'Anna', 'parent', 'adult', $2) RETURNING id",
    )
    .bind(tenant)
    .bind(anna_pid)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    let (_, as_philip) = session_for(&env, philip, tenant).await;
    let (_, as_anna) = session_for(&env, anna, tenant).await;

    let his = put_rules(
        &env,
        &as_philip,
        json!({ "daily_limit_minutes": 90, "focus_hours": null, "sites": ["reddit.com"] }),
    )
    .await
    .unwrap();
    put_rules(
        &env,
        &as_anna,
        json!({ "daily_limit_minutes": 45, "focus_hours": null, "sites": [] }),
    )
    .await
    .unwrap();
    let philip_rules = rules_id(&env, philip).await;
    let anna_rules = rules_id(&env, anna).await;

    // Neither parent can read, change or delete the other's own rules (403)…
    for (who, theirs) in [(&as_anna, philip_rules), (&as_philip, anna_rules)] {
        assert!(is_forbidden(&profile_as(&env, who, theirs).await));
        assert!(is_forbidden(&edit_profile_as(&env, who, theirs).await));
        let del = crate::profiles::delete_profile(State(env.st.clone()), clone(who), Path(theirs))
            .await
            .map(|j| j.0);
        assert!(is_forbidden(&del));
        let listed = crate::profiles::list_profiles(State(env.st.clone()), clone(who))
            .await
            .unwrap()
            .0;
        assert!(!listed.to_string().contains(&theirs.to_string()));
    }
    // …nothing changed, and each still has their own through /api/me/rules.
    assert_eq!(get_rules(&env, &as_philip).await.unwrap(), his);
    assert!(profile_as(&env, &as_philip, philip_rules).await.is_ok());
    assert_eq!(
        get_rules(&env, &as_anna).await.unwrap()["daily_limit_minutes"],
        45
    );

    // A child's rules are still every parent's.
    let mia = env.member(tenant, "Mia").await;
    let mia_rules = rules_id(&env, mia).await;
    assert!(profile_as(&env, &as_anna, mia_rules).await.is_ok());
    assert!(edit_profile_as(&env, &as_anna, mia_rules).await.is_ok());
    assert!(edit_profile_as(&env, &as_philip, mia_rules).await.is_ok());
    env.drop_db().await;
}

/// The login `name` on `device`, tied to `account` (the linking heuristics
/// aren't what these tests are about).
async fn login_of(env: &Env, device: Uuid, name: &str, account: Uuid) -> Uuid {
    sqlx::query_scalar(
        "UPDATE device_users SET account_id = $3
          WHERE device_id = $1 AND os_username = $2 RETURNING id",
    )
    .bind(device)
    .bind(name)
    .bind(account)
    .fetch_one(&env.st.db)
    .await
    .unwrap()
}

async fn events_as(env: &Env, who: &AuthAdmin, device: Option<Uuid>) -> Vec<Value> {
    let got = crate::events::list_events(
        State(env.st.clone()),
        clone(who),
        axum::extract::Query(crate::events::EventsQuery {
            device_id: device,
            r#type: None,
            severity: None,
            limit: None,
        }),
    )
    .await
    .unwrap()
    .0;
    got["events"].as_array().unwrap().clone()
}

#[tokio::test]
async fn an_adults_moments_are_theirs_the_hub_sees_minutes_only() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let (jonas, _) = adult_member(&env, tenant, "Jonas").await;
    let desk = env
        .computer(tenant, Some(jonas), &["jonas"], None, None)
        .await;
    let jonas_login = login_of(&env, desk, "jonas", jonas).await;
    let mia = env.member(tenant, "Mia").await;
    let laptop = env.computer(tenant, Some(mia), &["mia"], None, None).await;
    let mia_login = login_of(&env, laptop, "mia", mia).await;
    let (_, as_hub) = session_for(&env, philip, tenant).await;
    let (_, as_jonas) = session_for(&env, jonas, tenant).await;

    let db = &env.st.db;
    let ev = |device, login, etype: &'static str, severity: &'static str| {
        crate::events::insert(db, tenant, Some(device), login, etype, severity, json!({}))
    };
    ev(desk, Some(jonas_login), "screen_time_exceeded", "info")
        .await
        .unwrap();
    ev(desk, Some(jonas_login), "app_blocked", "info")
        .await
        .unwrap();
    // The computer's own event: no login, about the machine.
    ev(desk, None, "tamper", "warn").await.unwrap();
    ev(laptop, Some(mia_login), "screen_time_exceeded", "info")
        .await
        .unwrap();

    let from = |evs: &[Value], login: Uuid| {
        evs.iter()
            .filter(|e| e["device_user_id"] == login.to_string())
            .count()
    };

    // The hub: Mia's moments, the computer's own — none of Jonas's.
    let all = events_as(&env, &as_hub, None).await;
    assert_eq!(from(&all, jonas_login), 0, "{all:?}");
    assert_eq!(from(&all, mia_login), 1);
    assert!(all.iter().any(|e| e["type"] == "tamper"));
    let on_desk = events_as(&env, &as_hub, Some(desk)).await;
    assert_eq!(on_desk.len(), 1);
    assert_eq!(on_desk[0]["type"], "tamper");
    // …and not through the computer's own page either.
    let page = crate::devices::get_device(State(env.st.clone()), clone(&as_hub), Path(desk))
        .await
        .unwrap()
        .0;
    let recent = page["recent_events"].as_array().unwrap();
    assert_eq!(from(recent, jonas_login), 0, "{recent:?}");
    assert_eq!(recent.len(), 1);

    // Jonas is never hidden from himself.
    let his = events_as(&env, &as_jonas, Some(desk)).await;
    assert_eq!(from(&his, jonas_login), 2);
    env.drop_db().await;
}

#[tokio::test]
async fn own_rules_never_land_on_a_shared_profile() {
    let Some(env) = Env::new().await else { return };
    let (tenant, _) = env.household("Philip").await;
    let (ada, _) = adult_member(&env, tenant, "Ada").await;
    let preset: Uuid = sqlx::query_scalar(
        "SELECT id FROM profiles WHERE tenant_id = $1 AND kind = 'adult' AND is_preset",
    )
    .bind(tenant)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    sqlx::query("UPDATE admins SET profile_id = $2 WHERE id = $1")
        .bind(ada)
        .bind(preset)
        .execute(&env.st.db)
        .await
        .unwrap();
    let before: Value = sqlx::query_scalar("SELECT policy FROM profiles WHERE id = $1")
        .bind(preset)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    let (_, as_ada) = session_for(&env, ada, tenant).await;
    put_rules(
        &env,
        &as_ada,
        json!({ "daily_limit_minutes": 90, "focus_hours": null, "sites": [] }),
    )
    .await
    .unwrap();
    let now: Uuid = sqlx::query_scalar("SELECT profile_id FROM admins WHERE id = $1")
        .bind(ada)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_ne!(now, preset, "a copy, not the preset");
    let after: Value = sqlx::query_scalar("SELECT policy FROM profiles WHERE id = $1")
        .bind(preset)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(before, after, "the preset is untouched");
    env.drop_db().await;
}

#[tokio::test]
async fn legacy_closed_networks_open_up_and_keep_their_blocks() {
    let Some(env) = Env::new().await else { return };
    let (tenant, _) = env.household("Philip").await;
    let legacy: Uuid = sqlx::query_scalar(
        "INSERT INTO profiles (tenant_id, name, kind, is_preset, policy)
         VALUES ($1, 'Old kids', 'custom', false, $2) RETURNING id",
    )
    .bind(tenant)
    .bind(json!({
        "version": 1,
        "dns": { "mode": "default_deny", "allowlist": ["wikipedia.org"],
                 "blocklist": ["x.com"], "safe_search": true, "upstream": "1.1.1.3" },
        "firewall": { "mode": "default_deny", "allow_outbound_ports": [53, 80, 443],
                      "allow_inbound_ports": [22] },
        "screen_time": { "enabled": true, "daily_limit_minutes": 60, "schedule": [],
                         "bedtime": null },
        "blocks": { "apps": ["tiktok"], "categories": ["adult"], "custom_domains": [] }
    }))
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    let stamp = |db: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT updated_at FROM profiles WHERE id = $1",
        )
        .bind(legacy)
        .fetch_one(&db)
        .await
        .unwrap()
    };
    let before = stamp(env.st.db.clone()).await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    assert_eq!(
        crate::profiles::open_legacy_networks(&env.st.db)
            .await
            .unwrap(),
        1
    );
    let p: Value = sqlx::query_scalar("SELECT policy FROM profiles WHERE id = $1")
        .bind(legacy)
        .fetch_one(&env.st.db)
        .await
        .unwrap();
    assert_eq!(p["dns"]["mode"], "allow_all");
    assert_eq!(p["dns"]["allowlist"], json!(["*"]));
    assert_eq!(p["firewall"]["mode"], "allow_all");
    assert_eq!(p["firewall"]["allow_outbound_ports"], json!([]));
    // Every block survives.
    assert_eq!(p["dns"]["blocklist"], json!(["x.com"]));
    assert_eq!(p["dns"]["safe_search"], true);
    assert_eq!(p["dns"]["upstream"], "1.1.1.3");
    assert_eq!(p["blocks"]["apps"], json!(["tiktok"]));
    assert_eq!(p["blocks"]["categories"], json!(["adult"]));
    assert_eq!(p["screen_time"]["daily_limit_minutes"], 60);
    // Agents re-pull.
    assert!(stamp(env.st.db.clone()).await > before);
    let parsed: openscreentime_policy::Policy = serde_json::from_value(p).unwrap();
    assert!(!parsed.dns.is_default_deny() && !parsed.firewall.is_default_deny());

    // Once.
    assert_eq!(
        crate::profiles::open_legacy_networks(&env.st.db)
            .await
            .unwrap(),
        0
    );
    env.drop_db().await;
}
