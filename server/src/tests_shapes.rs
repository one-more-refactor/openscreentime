//! What the person page reads, as the server really sends it.
//!
//! Acceptance round 4: every per-person action on a real server addressed
//! `undefined` — `/api/family` never sent the `account_id` the console's
//! types declare, and only the console's mock data had it, so the console's
//! own tests passed. Edit and Remove went to `/api/members/undefined` (400),
//! and "Where the time went" fell back to the parent's own day.
//!
//! This test seeds a household the way it looks in real life (Mia's laptop,
//! Philip's login next to hers, a day of use, an ask, the computer's own
//! moments), calls each endpoint the person page calls, and:
//!
//! * follows the ids the page follows — the card's `account_id` into
//!   "where the time went", Edit and Remove — to the right person;
//! * records the **shape** of each response (keys, nested, with the JSON
//!   type of each leaf) in `web/src/test/server-shapes.json`. The console's
//!   `shapes.test.ts` checks its TypeScript types and its mock against that
//!   file, so a field the console declares but the server doesn't send (or a
//!   mock that sends more than the server) fails a test instead of a family.
//!
//! A changed response fails here until the file is rewritten:
//! `OST_WRITE_SHAPES=1 cargo test shapes` (then run the web tests).
//! Same harness and skip rule as `tests_auth`.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::Utc;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::state::{AgentAuth, AuthAdmin};
use crate::tests_auth::{session_for, Env};

const SHAPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../web/src/test/server-shapes.json"
);

fn clone(a: &AuthAdmin) -> AuthAdmin {
    AuthAdmin {
        admin_id: a.admin_id,
        tenant_id: a.tenant_id,
        role: a.role.clone(),
        blocked: a.blocked,
    }
}

/// The shape of a JSON value: an object's keys, each with its shape; an
/// array's elements merged into one; a leaf's JSON type.
fn shape(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                // An event's payload is free-form by design (the console
                // reads it as `Record<string, unknown>`).
                .map(|(k, v)| match (k.as_str(), v) {
                    ("payload", Value::Object(_)) => (k.clone(), json!("object")),
                    _ => (k.clone(), shape(v)),
                })
                .collect(),
        ),
        Value::Array(a) => match a.iter().map(shape).reduce(merge) {
            Some(one) => json!([one]),
            None => json!([]),
        },
        Value::String(_) => json!("string"),
        Value::Number(_) => json!("number"),
        Value::Bool(_) => json!("boolean"),
        Value::Null => json!("null"),
    }
}

/// Two elements of one array as one shape: every key either has; where one
/// is `null` the other's shape wins.
fn merge(a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::Object(mut x), Value::Object(y)) => {
            for (k, v) in y {
                let merged = match x.remove(&k) {
                    Some(cur) => merge(cur, v),
                    None => v,
                };
                x.insert(k, merged);
            }
            Value::Object(x)
        }
        (Value::Array(x), Value::Array(y)) => match x.into_iter().chain(y).reduce(merge) {
            Some(one) => json!([one]),
            None => json!([]),
        },
        (a, b) if a == "null" => b,
        (a, _) => a,
    }
}

/// Where two shapes differ, as `path: committed → now` lines.
fn diff(path: &str, was: &Value, now: &Value, out: &mut Vec<String>) {
    match (was, now) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for k in keys {
                let p = format!("{path}.{k}");
                match (a.get(k), b.get(k)) {
                    (Some(x), Some(y)) => diff(&p, x, y, out),
                    (Some(_), None) => out.push(format!("{p}: no longer sent")),
                    (None, Some(y)) => out.push(format!("{p}: new ({y})")),
                    (None, None) => {}
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if let (Some(x), Some(y)) = (a.first(), b.first()) {
                diff(&format!("{path}[]"), x, y, out);
            } else if a.len() != b.len() {
                out.push(format!("{path}: {was} → {now}"));
            }
        }
        // A field that is `null` in one snapshot and a value in the other is
        // one nullable field, not a change of shape: `rules.resume_at` is null
        // or a time depending on the hour the test runs (CI at 22:10 UTC saw a
        // string where the snapshot, taken by day, had null).
        _ if was == "null" || now == "null" => {}
        _ if was != now => out.push(format!("{path}: {was} → {now}")),
        _ => {}
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

#[tokio::test]
async fn the_person_page_reads_what_the_server_sends() {
    let Some(env) = Env::new().await else { return };
    let (tenant, philip) = env.household("Philip").await;
    let mia = env.member(tenant, "Mia").await;
    let leo = env.member(tenant, "Leo").await;
    // Mia's laptop, with Philip's own login on it (Who's who: his).
    let laptop = env
        .computer(tenant, Some(mia), &["mia", "philip"], None, None)
        .await;
    sqlx::query(
        "UPDATE device_users SET account_id = $2, unsorted = false
          WHERE device_id = $1 AND os_username = 'philip'",
    )
    .bind(laptop)
    .bind(philip)
    .execute(&env.st.db)
    .await
    .unwrap();
    // …and the made-up kid the unknown login was first linked to is gone.
    sqlx::query(
        "DELETE FROM admins WHERE tenant_id = $1 AND role = 'member' AND display_name = 'philip'",
    )
    .bind(tenant)
    .execute(&env.st.db)
    .await
    .unwrap();
    let mia_du = du_of(&env, laptop, "mia").await;
    let agent = AgentAuth {
        device_id: laptop,
        tenant_id: tenant,
    };

    // Their day: used time on the ledger, apps (hers and his), the
    // computer's lookups.
    sqlx::query(
        "INSERT INTO screen_time_ledger (device_user_id, day, used_seconds, earned_seconds)
         VALUES ($1, $2, 1380, 900)",
    )
    .bind(mia_du)
    .bind(crate::ledger::local_today(None, Utc::now()))
    .execute(&env.st.db)
    .await
    .unwrap();
    let hour = Utc::now().format("%Y-%m-%dT%H:00:00Z").to_string();
    let slices: crate::usage::IngestReq = serde_json::from_value(json!({ "slices": [
        { "os_username": "mia", "hour": hour, "kind": "app", "key": "Firefox ESR", "amount": 1150 },
        { "os_username": "mia", "hour": hour, "kind": "app", "key": "Text Editor", "amount": 780 },
        { "os_username": "philip", "hour": hour, "kind": "app", "key": "Text Editor", "amount": 1320 },
        { "os_username": "philip", "hour": hour, "kind": "app", "key": "Firefox ESR", "amount": 120 },
        { "os_username": "", "hour": hour, "kind": "site", "key": "wikipedia.org", "amount": 7 },
    ]}))
    .unwrap();
    let _ = crate::usage::ingest(State(env.st.clone()), agent, Json(slices))
        .await
        .unwrap();
    // An ask waiting on a parent.
    sqlx::query(
        "INSERT INTO earn_requests (tenant_id, device_id, device_user_id, task_id, task_label, minutes)
         VALUES ($1, $2, $3, 'more', 'Asked for more time', 15)",
    )
    .bind(tenant)
    .bind(laptop)
    .bind(mia_du)
    .execute(&env.st.db)
    .await
    .unwrap();
    // Moments: her unlock code, and the computer's own pause.
    let batch: crate::agent::PushEventsReq = serde_json::from_value(json!({ "events": [
        { "id": Uuid::new_v4(), "type": "parent_code_ok", "severity": "info", "device_user": "mia",
          "payload": { "via": "lock_screen", "user": "mia", "detail": "unlock code accepted" } },
        { "id": Uuid::new_v4(), "type": "lock", "severity": "info",
          "payload": { "message": "paused by a parent" } },
    ]}))
    .unwrap();
    crate::agent::push_events(State(env.st.clone()), agent, Json(batch))
        .await
        .unwrap();

    let (_, hub) = session_for(&env, philip, tenant).await;

    // GET /api/family — Mia's card carries the id the page addresses her by.
    let family = crate::family::get_family(State(env.st.clone()), clone(&hub))
        .await
        .unwrap()
        .0;
    let card = family["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Mia")
        .cloned()
        .unwrap_or_else(|| panic!("Mia is not on the family page: {family}"));
    assert_eq!(card["account_id"], json!(mia), "{card}");
    assert_eq!(card["key"], card["account_id"], "{card}");
    let id: Uuid = serde_json::from_value(card["account_id"].clone()).unwrap();

    // GET /api/usage/where?account_id= — her apps, never her father's.
    let where_ = crate::usage::where_api(
        State(env.st.clone()),
        clone(&hub),
        Query(crate::usage::WhereQuery { account_id: id }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(
        where_["apps"],
        json!([
            { "key": "Firefox ESR", "seconds": 1150 },
            { "key": "Text Editor", "seconds": 780 },
        ]),
        "{where_}"
    );

    // GET /api/events?device_id= — the computer's moments.
    let events = crate::events::list_events(
        State(env.st.clone()),
        clone(&hub),
        Query(crate::events::EventsQuery {
            device_id: Some(laptop),
            r#type: None,
            severity: None,
            limit: Some(30),
        }),
    )
    .await
    .unwrap()
    .0;
    let types: Vec<&str> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["type"].as_str())
        .collect();
    assert!(
        types.contains(&"parent_code_ok") && types.contains(&"lock"),
        "{events}"
    );

    // The shapes, before Edit and Remove change the household.
    let now = json!({
        "GET /api/family": shape(&family),
        "GET /api/usage/where": shape(&where_),
        "GET /api/events": shape(&events),
    });

    // PATCH /api/members/{account_id} — Edit reaches her.
    let edited = crate::members::patch_member(
        State(env.st.clone()),
        clone(&hub),
        Path(id),
        Json(serde_json::from_value(json!({ "display_name": "Mia R" })).unwrap()),
    )
    .await
    .unwrap()
    .0;
    assert!(edited.to_string().contains("Mia R"), "{edited}");
    // DELETE /api/members/{account_id} — Remove removes her, and only her.
    let _ = crate::members::delete_member(State(env.st.clone()), clone(&hub), Path(id))
        .await
        .unwrap();
    let left: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM admins WHERE tenant_id = $1 AND role = 'member' ORDER BY display_name",
    )
    .bind(tenant)
    .fetch_all(&env.st.db)
    .await
    .unwrap();
    assert_eq!(left, vec![leo]);
    env.drop_db().await;

    let written = format!("{}\n", serde_json::to_string_pretty(&now).unwrap());
    if std::env::var_os("OST_WRITE_SHAPES").is_some() {
        std::fs::write(SHAPES, written).unwrap();
        return;
    }
    let was: Value = serde_json::from_str(&std::fs::read_to_string(SHAPES).unwrap_or_default())
        .unwrap_or(Value::Object(Map::new()));
    let mut changed = Vec::new();
    diff("", &was, &now, &mut changed);
    assert!(
        changed.is_empty(),
        "the person page's responses changed shape — check web/src/types.ts and the \
         mock, then rewrite {SHAPES} with `OST_WRITE_SHAPES=1 cargo test shapes`:\n  {}",
        changed.join("\n  ")
    );
}

#[test]
fn a_nullable_field_is_not_a_change_of_shape() {
    let mut out = Vec::new();
    diff(
        "x",
        &json!({"resume_at": "null"}),
        &json!({"resume_at": "string"}),
        &mut out,
    );
    diff(
        "x",
        &json!({"resume_at": "string"}),
        &json!({"resume_at": "null"}),
        &mut out,
    );
    assert!(out.is_empty(), "{out:?}");
    diff(
        "x",
        &json!({"n": "number"}),
        &json!({"n": "string"}),
        &mut out,
    );
    assert_eq!(out, vec!["x.n: \"number\" → \"string\"".to_string()]);
}

#[test]
fn shapes_merge_elements_and_prefer_a_real_value_over_null() {
    let s = shape(&json!({
        "list": [ { "a": 1, "b": null }, { "b": { "c": "x" } } ],
        "none": [],
    }));
    assert_eq!(
        s,
        json!({
            "list": [ { "a": "number", "b": { "c": "string" } } ],
            "none": [],
        })
    );
    let mut out = Vec::new();
    diff(
        "",
        &json!({ "a": "string" }),
        &json!({ "b": "string" }),
        &mut out,
    );
    assert_eq!(out, vec![".a: no longer sent", ".b: new (\"string\")"]);
}
