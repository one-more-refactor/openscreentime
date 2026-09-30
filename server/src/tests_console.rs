//! Database-backed tests for what the console says about a day: whose
//! moment a computer's event is, and where the time went. Each reproduces a
//! failure from acceptance round 3 (a Debian 12 GNOME computer, Mia on it,
//! Philip's own login next to hers). Same harness and skip rule as
//! `tests_auth`.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::state::AgentAuth;
use crate::tests_auth::Env;

fn agent(device: Uuid, tenant: Uuid) -> AgentAuth {
    AgentAuth {
        device_id: device,
        tenant_id: tenant,
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

/// The event rows of a type, as (login they were filed under, payload).
async fn filed(env: &Env, device: Uuid, etype: &str) -> Vec<(Option<Uuid>, Value)> {
    sqlx::query_as(
        "SELECT device_user_id, payload FROM events
          WHERE device_id = $1 AND type = $2 ORDER BY created_at, payload::text",
    )
    .bind(device)
    .bind(etype)
    .fetch_all(&env.st.db)
    .await
    .unwrap()
}

/// Acceptance round 3: Philip's own snoozes showed on Mia's page as "Got
/// some more minutes", because the agent sent them with no login and the
/// console told a login-less event on every person of that computer. The
/// events below are what that agent sent; each must be filed under the
/// person it is about, and the computer's own under nobody.
#[tokio::test]
async fn a_computers_events_are_filed_under_their_person() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    let laptop = env
        .computer(tenant, Some(mia), &["mia", "philip"], None, None)
        .await;
    // Who's who: the philip login is Philip.
    sqlx::query(
        "UPDATE device_users SET account_id = $2, unsorted = false
          WHERE device_id = $1 AND os_username = 'philip'",
    )
    .bind(laptop)
    .bind(philip)
    .execute(&env.st.db)
    .await
    .unwrap();
    let mia_du = du_of(&env, laptop, "mia").await;
    let philip_du = du_of(&env, laptop, "philip").await;

    let batch: crate::agent::PushEventsReq = serde_json::from_value(json!({ "events": [
        // A 0.6.1 agent's self-given snooze: the login only in the payload.
        { "id": Uuid::new_v4(), "type": "screen_time_earned", "severity": "info",
          "payload": { "user": "philip", "minutes": 15, "via": "self", "today": 1, "of": 3 } },
        // The unlock code at Mia's lock: named.
        { "id": Uuid::new_v4(), "type": "parent_code_ok", "severity": "info", "device_user": "mia",
          "payload": { "via": "lock_screen", "user": "mia", "detail": "unlock code accepted" } },
        // The computer's own: the resolver stopped (and came back).
        { "id": Uuid::new_v4(), "type": "enforcement_degraded", "severity": "critical",
          "payload": { "kind": "dns_resolver_stopped", "message": "dnsmasq stopped" } },
        // A login this computer doesn't have is nobody's.
        { "id": Uuid::new_v4(), "type": "screen_time_earned", "severity": "info",
          "payload": { "user": "zz-leo", "minutes": 15, "via": "self" } },
    ]}))
    .unwrap();
    crate::agent::push_events(State(env.st.clone()), agent(laptop, tenant), Json(batch))
        .await
        .unwrap();

    let earned = filed(&env, laptop, "screen_time_earned").await;
    let whose = |user: &str| earned.iter().find(|(_, p)| p["user"] == user).unwrap().0;
    assert_eq!(whose("philip"), Some(philip_du), "the snooze is Philip's");
    assert_eq!(whose("zz-leo"), None, "not a login of this computer");
    assert_eq!(
        filed(&env, laptop, "parent_code_ok").await[0].0,
        Some(mia_du)
    );
    assert_eq!(filed(&env, laptop, "enforcement_degraded").await[0].0, None);

    // A second parent reading the computer's events doesn't get Philip's
    // snooze at all (an adult's moments are his), but does get Mia's unlock
    // and the computer's own.
    let ann: Uuid = sqlx::query_scalar(
        "INSERT INTO admins (tenant_id, display_name, role, age_bracket)
         VALUES ($1, 'Ann', 'parent', 'adult') RETURNING id",
    )
    .bind(tenant)
    .fetch_one(&env.st.db)
    .await
    .unwrap();
    let (_, as_ann) = crate::tests_auth::session_for(&env, ann, tenant).await;
    let seen = crate::events::list_events(
        State(env.st.clone()),
        as_ann,
        axum::extract::Query(crate::events::EventsQuery {
            device_id: Some(laptop),
            r#type: None,
            severity: None,
            limit: None,
        }),
    )
    .await
    .unwrap()
    .0;
    let told: Vec<(String, Option<String>)> = seen["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["type"].as_str().unwrap().to_string(),
                e["payload"]["user"].as_str().map(str::to_string),
            )
        })
        .collect();
    let has = |t: &str, u: Option<&str>| told.iter().any(|(a, b)| a == t && b.as_deref() == u);
    assert!(!has("screen_time_earned", Some("philip")), "{told:?}");
    assert!(has("parent_code_ok", Some("mia")), "{told:?}");
    assert!(has("enforcement_degraded", None), "{told:?}");
    env.drop_db().await;
}

#[test]
fn an_events_login_is_named_or_in_its_payload() {
    use crate::agent::event_login;
    let named = event_login(Some("mia".into()), &json!({ "user": "philip" }));
    assert_eq!(named.as_deref(), Some("mia"));
    assert_eq!(
        event_login(None, &json!({ "user": "philip" })).as_deref(),
        Some("philip")
    );
    assert_eq!(
        event_login(None, &json!({ "os_username": "mia" })).as_deref(),
        Some("mia")
    );
    assert_eq!(event_login(Some(" ".into()), &json!({})), None);
    assert_eq!(
        event_login(None, &json!({ "kind": "dns_resolver_stopped" })),
        None
    );
}

/// Acceptance round 3: "Where the time went: Nothing yet today" after 37
/// minutes of Firefox and Text Editor. The agent now names desktop apps
/// (`client/src/attrib.rs`); the slices below are exactly what it posts
/// (`Attrib::drain`) — a desktop app's name as the key, a catalog id, the
/// computer's site lookups — and they must land and come back out.
#[tokio::test]
async fn the_agents_slices_land_and_show() {
    let Some(env) = Env::new().await else { return };
    let (tenant, _philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    let laptop = env.computer(tenant, Some(mia), &["mia"], None, None).await;
    let hour = chrono::Utc::now().format("%Y-%m-%dT%H:00:00Z").to_string();
    let body: Value = json!({ "slices": [
        { "os_username": "mia", "hour": hour, "kind": "app", "key": "Firefox ESR", "amount": 600 },
        { "os_username": "mia", "hour": hour, "kind": "app", "key": "Text Editor", "amount": 120 },
        { "os_username": "mia", "hour": hour, "kind": "app", "key": "steam", "amount": 60 },
        { "os_username": "", "hour": hour, "kind": "site", "key": "wikipedia.org", "amount": 7 },
    ]});
    let req: crate::usage::IngestReq = serde_json::from_value(body).unwrap();
    let _ = crate::usage::ingest(State(env.st.clone()), agent(laptop, tenant), Json(req))
        .await
        .unwrap();
    let w =
        crate::usage::where_for_account(&env.st.db, tenant, mia, crate::usage::Exposure::HUB_FULL)
            .await
            .unwrap();
    assert_eq!(
        w["apps"],
        json!([
            { "key": "Firefox ESR", "seconds": 600 },
            { "key": "Text Editor", "seconds": 120 },
            { "key": "steam", "seconds": 60 },
        ]),
        "{w}"
    );
    assert_eq!(
        w["sites"],
        json!([{ "key": "wikipedia.org", "hits": 7 }]),
        "{w}"
    );
    env.drop_db().await;
}
