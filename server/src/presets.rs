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

/// The `kids` preset — filtered, not walled off.
///
/// Deliberately NOT a zero-trust allowlist. The previous version permitted five
/// domains and denied the rest, which meant a 9-year-old's laptop could not
/// reach Minecraft, Steam, a school portal, or `deb.debian.org` — so `apt`
/// failed too. Every real-world use ended with an adult widening the allowlist
/// or switching enforcement off, which is worse than filtering properly.
///
/// The model here: forward everything to a resolver that already filters by
/// category (Cloudflare 1.1.1.3 = malware + adult), block the specific things
/// that slip past it or exist to bypass it, and keep the firewall permissive
/// while still dropping the DNS-bypass paths (DoH/DoT/Tor). Strict where it
/// counts, invisible everywhere else.
pub fn kids_policy() -> Value {
    json!({
        "version": 1,
        // allow_all + a filtering upstream. `force_dns` below pins every query
        // to that upstream, and `block_doh`/`block_dot` close the encrypted
        // bypasses, so "allow_all" is not "unfiltered".
        "dns": { "mode": "allow_all",
            "allowlist": ["*"],
            "blocklist": [
                // Web proxies and bypass services — the actual way a kid gets
                // around a category filter, and not something 1.1.1.3 blocks.
                "croxyproxy.com","proxysite.com","kproxy.com","hidester.com",
                "4everproxy.com","whoer.net","hide.me","vpnbook.com",
                // Torrent indexes.
                "thepiratebay.org","1337x.to","torrentz2.eu","rarbg.to",
                // Adult — belt and braces over the upstream's category filter.
                "pornhub.com","xvideos.com","xnxx.com","onlyfans.com",
                // Gambling.
                "stake.com","bet365.com","roobet.com",
                // Anonymous stranger-chat.
                "omegle.com","chatroulette.com"
            ],
            "safe_search": true,
            // 1.1.1.3 = Cloudflare for Families: malware AND adult content.
            // 1.1.1.2 (the old value) blocks malware only.
            "upstream": "1.1.1.3" },
        // Permissive by default so games, printers, school software and apt
        // simply work. The lockdown flags below still emit targeted drops —
        // chain policy and lockdown rules are independent.
        "firewall": { "mode": "allow_all", "allow_outbound_ports": [], "allow_inbound_ports": [22] },
        "screen_time": { "enabled": true, "daily_limit_minutes": 60,
            "schedule": [ {"days":[1,2,3,4,5],"start":"07:00","end":"20:00"},
                          {"days":[0,6],"start":"09:00","end":"20:00"} ],
            "bedtime": { "start":"20:00","end":"07:00" } },
        "app_limits": [],
        // block_vpn stays OFF: a parent-managed WireGuard profile is a supported
        // feature, and turning both on means the agent applies a tunnel its own
        // firewall then kills — which is exactly how a laptop lost its network.
        // offline_lockdown_days 0: a device that cannot reach the server must
        // not brick itself. Screen time still applies from the cached policy.
        "lockdown": { "force_dns": true, "block_doh": true, "block_dot": true, "block_tor": true, "block_vpn": false, "offline_lockdown_days": 0 },
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
        "lockdown": { "force_dns": false, "block_doh": true, "block_dot": true, "block_tor": true, "block_vpn": false, "offline_lockdown_days": 0 },
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
    /// The kids profile must stay usable. Each of these was a real failure:
    /// a five-domain allowlist that broke apt and Minecraft, no inbound port
    /// so SSH recovery was impossible, block_vpn fighting a managed tunnel,
    /// and an offline lockdown that bricked a device it could not reach.
    #[test]
    fn kids_preset_cannot_brick_a_device() {
        let p = kids_policy();

        // Filtering happens at the resolver, not via a hand-maintained allowlist.
        assert_eq!(p["dns"]["mode"], "allow_all");
        assert_eq!(
            p["dns"]["upstream"], "1.1.1.3",
            "must be Cloudflare for Families (malware AND adult); 1.1.1.2 is malware only"
        );
        assert_eq!(p["dns"]["safe_search"], true);
        assert!(
            p["dns"]["blocklist"].as_array().unwrap().len() >= 10,
            "the blocklist is what catches proxies and bypass services the upstream misses"
        );

        // A device must always be reachable for recovery.
        assert!(
            p["firewall"]["allow_inbound_ports"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(22)),
            "no inbound SSH means a self-inflicted lockout needs physical access"
        );

        // The two settings that turned lockouts into bricks.
        assert_eq!(
            p["lockdown"]["block_vpn"], false,
            "block_vpn + a managed VPN profile makes the agent kill its own tunnel"
        );
        assert_eq!(
            p["lockdown"]["offline_lockdown_days"], 0,
            "a device that cannot reach the server must not lock itself permanently"
        );

        // Bypass paths stay shut — permissive is not unfiltered.
        for flag in ["force_dns", "block_doh", "block_dot", "block_tor"] {
            assert_eq!(p["lockdown"][flag], true, "{flag} must stay on");
        }
    }

    /// Screen time should govern *when* and *how much*, not lock a child out of
    /// a machine they need for school at 08:00.
    #[test]
    fn kids_windows_cover_a_normal_day() {
        let p = kids_policy();
        let sched = p["screen_time"]["schedule"].as_array().unwrap();
        let weekday = sched
            .iter()
            .find(|w| {
                w["days"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!(1))
            })
            .expect("a weekday window");
        assert_eq!(
            weekday["start"], "07:00",
            "mornings must not read as bedtime"
        );
        assert_eq!(p["screen_time"]["daily_limit_minutes"], 60);
    }

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
