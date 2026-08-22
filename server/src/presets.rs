//! Preset profiles — one per age bracket (docs/CONTRACT-0.4.md §3).
//!
//! Every tenant owns five `is_preset = true` rows: little, kid, younger_teen,
//! older_teen, adult. A new member's rules start as a copy of their bracket's
//! preset. The older `kids`/`teen`/`default` presets are no longer seeded;
//! existing rows stay valid and editable.
//!
//! Design carried over from the old `kids` preset, because every line of it
//! was a real failure once: filtering happens at the resolver (allow_all
//! through a family upstream + the catalog's blocks), never via a hand-kept
//! allowlist that breaks apt and Minecraft; inbound 22 stays open so a
//! self-inflicted lockout has a way in; `block_vpn` stays off so a managed
//! tunnel is not killed by its own firewall; `offline_lockdown_days` stays 0 so
//! a device that cannot reach the server does not brick itself.

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use openscreentime_policy::AgeBracket;

/// A preset to be seeded into `profiles`.
pub struct Preset {
    pub name: &'static str,
    pub kind: &'static str,
    pub policy: Value,
}

fn family_dns() -> Value {
    json!({ "mode": "allow_all", "allowlist": ["*"], "blocklist": [],
            "safe_search": true, "upstream": "1.1.1.3" })
}

fn open_dns(safe_search: bool) -> Value {
    json!({ "mode": "allow_all", "allowlist": ["*"], "blocklist": [],
            "safe_search": safe_search, "upstream": "1.1.1.2" })
}

/// Permissive so games, printers, school software and apt simply work; the
/// lockdown flags still emit targeted drops on top.
fn open_firewall() -> Value {
    json!({ "mode": "allow_all", "allow_outbound_ports": [], "allow_inbound_ports": [22] })
}

fn lockdown(force_dns: bool, block_doh: bool, block_dot: bool, block_tor: bool) -> Value {
    json!({ "force_dns": force_dns, "block_doh": block_doh, "block_dot": block_dot,
            "block_tor": block_tor, "block_vpn": false, "offline_lockdown_days": 0 })
}

fn no_screen_time() -> Value {
    json!({ "enabled": false, "daily_limit_minutes": 0, "schedule": [], "bedtime": null })
}

fn no_gamification() -> Value {
    json!({ "earn_time": { "enabled": false, "tasks": [] },
            "lockout": { "enabled": false, "unlock_challenge": "wait" } })
}

/// 0–6: curated, parent does everything. Hard daily limit, hard stop, no
/// request UI and no earning — the overlay just says it stopped.
pub fn little_policy() -> Value {
    json!({
        "version": 1,
        "dns": family_dns(),
        "firewall": open_firewall(),
        "screen_time": { "enabled": true, "daily_limit_minutes": 45,
            "schedule": [ {"days":[0,1,2,3,4,5,6],"start":"08:00","end":"19:00"} ],
            "bedtime": { "start":"19:00","end":"07:00" } },
        "gamification": {
            "earn_time": { "enabled": false, "tasks": [] },
            "lockout": { "enabled": true, "unlock_challenge": "parent_pin" } },
        "lockdown": lockdown(true, true, true, true),
        "blocks": {
            "apps": ["youtube"],
            "categories": ["social","video_streaming","games","messaging","adult",
                           "gambling","dating","ai_chat","proxies"],
            "custom_domains": [] }
    })
}

/// 6–12: hard limit, hard stop, can ask for time and earn it.
pub fn kid_policy() -> Value {
    json!({
        "version": 1,
        "dns": family_dns(),
        "firewall": open_firewall(),
        "screen_time": { "enabled": true, "daily_limit_minutes": 60,
            "schedule": [ {"days":[1,2,3,4,5],"start":"07:00","end":"20:00"},
                          {"days":[0,6],"start":"09:00","end":"20:00"} ],
            "bedtime": { "start":"20:00","end":"07:00" } },
        "gamification": {
            "earn_time": { "enabled": true, "tasks": [
                {"id":"reading","label":"Read for 20 min","reward_minutes":15},
                {"id":"chores","label":"Finish chores","reward_minutes":15} ] },
            "lockout": { "enabled": true, "unlock_challenge": "parent_pin" } },
        "lockdown": lockdown(true, true, true, true),
        "blocks": {
            "apps": ["tiktok","snapchat","instagram","discord","twitch","omegle"],
            "categories": ["social","adult","gambling","dating","proxies"],
            "custom_domains": [] }
    })
}

/// 12–16: goals + limit, hard stop after a short wind-down.
pub fn younger_teen_policy() -> Value {
    json!({
        "version": 1,
        "dns": family_dns(),
        "firewall": open_firewall(),
        "screen_time": { "enabled": true, "daily_limit_minutes": 150,
            "schedule": [ {"days":[1,2,3,4,5],"start":"07:00","end":"21:00"},
                          {"days":[0,6],"start":"09:00","end":"22:00"} ],
            "bedtime": { "start":"22:00","end":"06:30" } },
        "gamification": {
            "earn_time": { "enabled": true, "tasks": [
                {"id":"homework","label":"Finish homework","reward_minutes":20} ] },
            "lockout": { "enabled": true, "unlock_challenge": "parent_pin" } },
        "lockdown": lockdown(false, true, true, true),
        "blocks": {
            "apps": ["tiktok"],
            "categories": ["adult","gambling","dating","proxies"],
            "custom_domains": [] }
    })
}

/// 16–18: mostly self-set; the parent can still cap. No limit out of the box.
pub fn older_teen_policy() -> Value {
    json!({
        "version": 1,
        "dns": open_dns(false),
        "firewall": open_firewall(),
        "screen_time": no_screen_time(),
        "gamification": no_gamification(),
        "lockdown": lockdown(false, true, true, true),
        "blocks": {
            "apps": [],
            "categories": ["adult","gambling","proxies"],
            "custom_domains": [] }
    })
}

/// 18+: private self-tracking, nothing enforced by anyone else.
pub fn adult_policy() -> Value {
    json!({
        "version": 1,
        "dns": open_dns(false),
        "firewall": open_firewall(),
        "screen_time": no_screen_time(),
        "gamification": no_gamification()
    })
}

/// The preset policy for a bracket.
pub fn policy_for(bracket: AgeBracket) -> Value {
    match bracket {
        AgeBracket::Little => little_policy(),
        AgeBracket::Kid => kid_policy(),
        AgeBracket::YoungerTeen => younger_teen_policy(),
        AgeBracket::OlderTeen => older_teen_policy(),
        AgeBracket::Adult => adult_policy(),
    }
}

/// The five presets seeded into every tenant, in bracket order.
pub fn all_presets() -> Vec<Preset> {
    AgeBracket::ALL
        .into_iter()
        .map(|b| Preset {
            name: b.label(),
            kind: b.id(),
            policy: policy_for(b),
        })
        .collect()
}

/// Make sure a tenant has every bracket preset, inserting the missing ones.
/// Idempotent; called at tenant creation, at startup for every existing
/// tenant, and lazily wherever a preset id is needed.
pub async fn ensure_presets(db: &PgPool, tenant_id: Uuid) -> Result<(), sqlx::Error> {
    for p in all_presets() {
        sqlx::query(
            "INSERT INTO profiles (tenant_id, name, kind, is_preset, policy)
             SELECT $1, $2, $3, true, $4
             WHERE NOT EXISTS (
                 SELECT 1 FROM profiles WHERE tenant_id = $1 AND kind = $3 AND is_preset)",
        )
        .bind(tenant_id)
        .bind(p.name)
        .bind(p.kind)
        .bind(&p.policy)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// The preset profile id for a bracket in a tenant (seeding it if missing).
pub async fn preset_id(db: &PgPool, tenant_id: Uuid, bracket: AgeBracket) -> Result<Uuid, sqlx::Error> {
    let find = || async {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM profiles WHERE tenant_id = $1 AND kind = $2 AND is_preset
             ORDER BY created_at LIMIT 1",
        )
        .bind(tenant_id)
        .bind(bracket.id())
        .fetch_optional(db)
        .await
    };
    if let Some(id) = find().await? {
        return Ok(id);
    }
    ensure_presets(db, tenant_id).await?;
    find().await?.ok_or(sqlx::Error::RowNotFound)
}

/// Startup backfill: every existing tenant gets the bracket presets it lacks.
pub async fn backfill_all_tenants(db: &PgPool) -> Result<(), sqlx::Error> {
    let tenants: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tenants")
        .fetch_all(db)
        .await?;
    for t in tenants {
        ensure_presets(db, t).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use openscreentime_policy::{catalog, Policy};

    /// Drift guard: every preset must parse into the shared `Policy` type and
    /// re-serialize to exactly its normalized form. If a preset ever carries a
    /// field the policy crate doesn't model (which normalization would
    /// silently drop), this fails.
    #[test]
    fn presets_round_trip_through_policy_without_loss() {
        for preset in all_presets() {
            let parsed: Policy = serde_json::from_value(preset.policy.clone())
                .unwrap_or_else(|e| panic!("preset '{}' does not parse: {e}", preset.name));
            let normalized = serde_json::to_value(&parsed).unwrap();
            assert_eq!(
                normalized, preset.policy,
                "preset '{}' drifts from openscreentime_policy::Policy",
                preset.name
            );
            let reparsed: Policy = serde_json::from_value(normalized.clone()).unwrap();
            assert_eq!(serde_json::to_value(&reparsed).unwrap(), normalized);
        }
    }

    /// Each of these was a real failure once (see module docs).
    #[test]
    fn managed_presets_cannot_brick_a_device() {
        for b in [AgeBracket::Little, AgeBracket::Kid, AgeBracket::YoungerTeen] {
            let p = policy_for(b);
            assert_eq!(p["dns"]["mode"], "allow_all", "{b:?}");
            assert_eq!(p["dns"]["upstream"], "1.1.1.3", "{b:?}");
            assert_eq!(p["dns"]["safe_search"], true, "{b:?}");
            assert!(p["firewall"]["allow_inbound_ports"]
                .as_array()
                .unwrap()
                .contains(&json!(22)));
            assert_eq!(p["lockdown"]["block_vpn"], false, "{b:?}");
            assert_eq!(p["lockdown"]["offline_lockdown_days"], 0, "{b:?}");
            assert_eq!(p["lockdown"]["block_doh"], true, "{b:?}");
            assert_eq!(p["screen_time"]["enabled"], true, "{b:?}");
            assert_eq!(p["gamification"]["lockout"]["unlock_challenge"], "parent_pin", "{b:?}");
        }
    }

    /// Every block id a preset names must exist in the catalog, or the
    /// one-click it represents silently does nothing on the device.
    #[test]
    fn preset_blocks_are_real_catalog_ids() {
        for preset in all_presets() {
            let p: Policy = serde_json::from_value(preset.policy.clone()).unwrap();
            for a in &p.blocks.apps {
                assert!(catalog::app(a).is_some(), "{}: unknown app {a}", preset.name);
            }
            for c in &p.blocks.categories {
                assert!(catalog::category(c).is_some(), "{}: unknown category {c}", preset.name);
            }
        }
    }

    #[test]
    fn adults_and_older_teens_are_not_limited_and_little_cannot_ask() {
        assert_eq!(adult_policy()["screen_time"]["enabled"], false);
        assert!(adult_policy().get("blocks").is_none());
        assert_eq!(older_teen_policy()["screen_time"]["enabled"], false);
        assert_eq!(little_policy()["gamification"]["earn_time"]["enabled"], false);
        assert_eq!(all_presets().len(), 5);
    }
}
