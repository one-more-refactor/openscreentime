//! Preset profiles, mirrored verbatim from `docs/PROFILES.md`.
//!
//! Every new tenant is seeded with three `is_preset = true` rows: kids, teen,
//! default. Keep this file and `docs/PROFILES.md` in sync — the policies below
//! are copied exactly from that document.

use serde_json::{json, Value};

/// A preset to be seeded into `profiles` on tenant creation.
pub struct Preset {
    pub name: &'static str,
    pub kind: &'static str,
    pub policy: Value,
}

/// The `kids` preset — locked down, playful.
pub fn kids_policy() -> Value {
    json!({
        "version": 1,
        "dns": { "mode": "default_deny",
            "allowlist": ["wikipedia.org","khanacademy.org","pbskids.org","scratch.mit.edu","duolingo.com"],
            "blocklist": [], "safe_search": true, "upstream": "1.1.1.2" },
        "firewall": { "mode": "default_deny", "allow_outbound_ports": [53,80,443], "allow_inbound_ports": [] },
        "screen_time": { "enabled": true, "daily_limit_minutes": 60,
            "schedule": [ {"days":[1,2,3,4,5],"start":"15:00","end":"19:00"},
                          {"days":[0,6],"start":"09:00","end":"19:00"} ],
            "bedtime": { "start":"20:00","end":"07:00" } },
        "app_limits": [],
        "lockdown": { "force_dns": true, "block_doh": true, "block_dot": true, "block_tor": true, "block_vpn": true },
        "gamification": {
            "earn_time": { "enabled": true, "tasks": [
                {"id":"reading","label":"Read for 20 min","reward_minutes":15},
                {"id":"chores","label":"Finish chores","reward_minutes":15} ] },
            "lockout": { "enabled": true, "unlock_challenge": "math" },
            "streaks": { "enabled": true, "nudges": ["bedtime","breaks"] } }
    })
}

/// The `teen` preset — trusted-but-guarded.
pub fn teen_policy() -> Value {
    json!({
        "version": 1,
        "dns": { "mode": "default_deny",
            "allowlist": ["*.wikipedia.org","github.com","google.com","youtube.com","duolingo.com","*.edu"],
            "blocklist": [], "safe_search": true, "upstream": "1.1.1.2" },
        "firewall": { "mode": "default_deny", "allow_outbound_ports": [53,80,443,123], "allow_inbound_ports": [] },
        "screen_time": { "enabled": true, "daily_limit_minutes": 180,
            "schedule": [ {"days":[1,2,3,4,5],"start":"07:00","end":"21:00"},
                          {"days":[0,6],"start":"08:00","end":"22:00"} ],
            "bedtime": { "start":"22:30","end":"06:30" } },
        "app_limits": [],
        "lockdown": { "force_dns": false, "block_doh": true, "block_dot": true, "block_tor": true, "block_vpn": false },
        "gamification": {
            "earn_time": { "enabled": true, "tasks": [
                {"id":"homework","label":"Finish homework","reward_minutes":20} ] },
            "lockout": { "enabled": true, "unlock_challenge": "wait" },
            "streaks": { "enabled": true, "nudges": ["breaks"] } }
    })
}

/// The `default` preset — baseline for every newly-enrolled user.
pub fn default_policy() -> Value {
    json!({
        "version": 1,
        "dns": { "mode": "default_deny",
            "allowlist": ["*"], "blocklist": [], "safe_search": true, "upstream": "1.1.1.2" },
        "firewall": { "mode": "default_deny", "allow_outbound_ports": [53,80,443,123], "allow_inbound_ports": [] },
        "screen_time": { "enabled": false, "daily_limit_minutes": 0, "schedule": [], "bedtime": null },
        "app_limits": [],
        "gamification": {
            "earn_time": { "enabled": false, "tasks": [] },
            "lockout": { "enabled": false, "unlock_challenge": "wait" },
            "streaks": { "enabled": false, "nudges": [] } }
    })
}

/// The three presets seeded verbatim into every new tenant.
pub fn all_presets() -> Vec<Preset> {
    vec![
        Preset {
            name: "Kids",
            kind: "kids",
            policy: kids_policy(),
        },
        Preset {
            name: "Teen",
            kind: "teen",
            policy: teen_policy(),
        },
        Preset {
            name: "Default",
            kind: "default",
            policy: default_policy(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_policy::Policy;

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
                "preset '{}' drifts from sentinel_policy::Policy \
                 (a field is being dropped or defaulted during normalization)",
                preset.name
            );

            // And normalization itself must be a fixpoint.
            let reparsed: Policy = serde_json::from_value(normalized.clone()).unwrap();
            assert_eq!(
                serde_json::to_value(&reparsed).unwrap(),
                normalized,
                "preset '{}' is not stable under repeated normalization",
                preset.name
            );
        }
    }
}
