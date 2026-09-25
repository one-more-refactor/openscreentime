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
