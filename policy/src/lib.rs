//! The shared `Policy` document — the single most important shared contract.
//!
//! The server stores it (jsonb), the web edits it (`web/src/types.ts` mirrors
//! this shape), the agent enforces it. Mirrors `docs/API.md` → Policy exactly.
//! Every sub-object is `#[serde(default)]` and unknown fields are ignored, so
//! all components stay forward-compatible with newer policy versions (the docs
//! require this).

use serde::{Deserialize, Serialize};

fn default_version() -> u32 {
    1
}
fn default_deny() -> String {
    "default_deny".to_string()
}
fn default_upstream() -> String {
    "1.1.1.2".to_string()
}
fn default_challenge() -> String {
    "wait".to_string()
}

/// The full per-user policy document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub dns: DnsPolicy,
    #[serde(default)]
    pub firewall: FirewallPolicy,
    #[serde(default)]
    pub screen_time: ScreenTime,
    #[serde(default)]
    pub app_limits: Vec<AppLimit>,
    #[serde(default)]
    pub gamification: Gamification,
    /// Network lockdown toggles (block DoH/DoT/Tor/VPN, force DNS). Absent =
    /// all-off; skipped on serialize so presets that don't set it stay byte-
    /// identical (the drift guard depends on this).
    #[serde(default, skip_serializing_if = "NetworkLockdown::is_default")]
    pub lockdown: NetworkLockdown,
    /// Argon2 hash of the parent PIN, set server-side when an admin saves a PIN
    /// in the profile editor. The agent verifies entered PINs against this hash
    /// locally (works with no server connection). Never the plaintext PIN.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_pin_hash: Option<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            version: 1,
            dns: DnsPolicy::default(),
            firewall: FirewallPolicy::default(),
            screen_time: ScreenTime::default(),
            app_limits: Vec::new(),
            gamification: Gamification::default(),
            lockdown: NetworkLockdown::default(),
            parent_pin_hash: None,
        }
    }
}

/// Network anti-bypass lockdown. Each flag adds explicit firewall/DNS rules on
/// top of the base allowlist. All default off; the whole struct is omitted from
/// serialized output when every flag is off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkLockdown {
    /// Block plaintext DNS (udp/tcp 53) egress to anything but the agent's own
    /// resolver, so a managed user can't point a browser at 8.8.8.8 directly.
    #[serde(default)]
    pub force_dns: bool,
    /// Drop the well-known public DoH resolver IPs (Cloudflare/Google/Quad9/…)
    /// so browsers can't tunnel DNS over HTTPS around the local resolver.
    #[serde(default)]
    pub block_doh: bool,
    /// Block DNS-over-TLS (tcp 853).
    #[serde(default)]
    pub block_dot: bool,
    /// Block Tor (known directory-authority/OR ports + `.onion`).
    #[serde(default)]
    pub block_tor: bool,
    /// Block common commercial-VPN ports (WireGuard 51820, OpenVPN 1194,
    /// IPsec/IKE 500/4500).
    #[serde(default)]
    pub block_vpn: bool,
}

impl NetworkLockdown {
    /// True when every flag is off — used by `skip_serializing_if`.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
    /// True if any lockdown rule is active.
    pub fn any(&self) -> bool {
        self.force_dns || self.block_doh || self.block_dot || self.block_tor || self.block_vpn
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsPolicy {
    #[serde(default = "default_deny")]
    pub mode: String,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default)]
    pub blocklist: Vec<String>,
    #[serde(default)]
    pub safe_search: bool,
    #[serde(default = "default_upstream")]
    pub upstream: String,
}

impl Default for DnsPolicy {
    fn default() -> Self {
        DnsPolicy {
            mode: default_deny(),
            allowlist: Vec::new(),
            blocklist: Vec::new(),
            safe_search: true,
            upstream: default_upstream(),
        }
    }
}

impl DnsPolicy {
    /// Zero-trust default-deny is on unless the policy explicitly opts out.
    pub fn is_default_deny(&self) -> bool {
        self.mode != "allow_all"
    }
    /// `["*"]` means "forward everything to the filtered upstream" (see the
    /// `default` profile in docs/PROFILES.md).
    pub fn allows_everything(&self) -> bool {
        self.allowlist.iter().any(|d| d == "*")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirewallPolicy {
    #[serde(default = "default_deny")]
    pub mode: String,
    #[serde(default)]
    pub allow_outbound_ports: Vec<u16>,
    #[serde(default)]
    pub allow_inbound_ports: Vec<u16>,
}

impl Default for FirewallPolicy {
    fn default() -> Self {
        FirewallPolicy {
            mode: default_deny(),
            allow_outbound_ports: vec![53, 80, 443],
            allow_inbound_ports: Vec::new(),
        }
    }
}

impl FirewallPolicy {
    pub fn is_default_deny(&self) -> bool {
        self.mode != "allow_all"
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScreenTime {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub daily_limit_minutes: u32,
    #[serde(default)]
    pub schedule: Vec<Window>,
    #[serde(default)]
    pub bedtime: Option<Bedtime>,
}

/// An allowed window. `days` uses 0=Sunday .. 6=Saturday (matches the docs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    #[serde(default)]
    pub days: Vec<u8>,
    #[serde(default)]
    pub start: String, // "HH:MM"
    #[serde(default)]
    pub end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bedtime {
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLimit {
    #[serde(default)]
    pub r#match: String,
    #[serde(default)]
    pub daily_limit_minutes: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Gamification {
    #[serde(default)]
    pub earn_time: EarnTime,
    #[serde(default)]
    pub lockout: Lockout,
    #[serde(default)]
    pub streaks: Streaks,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EarnTime {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub tasks: Vec<EarnTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EarnTask {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub reward_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockout {
    #[serde(default)]
    pub enabled: bool,
    /// "math" | "wait" | "parent_pin"
    #[serde(default = "default_challenge")]
    pub unlock_challenge: String,
}

impl Default for Lockout {
    fn default() -> Self {
        Lockout {
            enabled: false,
            unlock_challenge: default_challenge(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Streaks {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub nudges: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kids_preset_and_ignores_unknown_fields() {
        let raw = r#"{
            "version": 1,
            "dns": { "mode": "default_deny", "allowlist": ["wikipedia.org"], "safe_search": true, "upstream": "1.1.1.2" },
            "firewall": { "mode": "default_deny", "allow_outbound_ports": [53,80,443] },
            "screen_time": { "enabled": true, "daily_limit_minutes": 60, "schedule": [], "bedtime": {"start":"20:00","end":"07:00"} },
            "future_field": { "nope": 1 }
        }"#;
        let p: Policy = serde_json::from_str(raw).unwrap();
        assert!(p.dns.is_default_deny());
        assert_eq!(p.screen_time.daily_limit_minutes, 60);
        assert_eq!(p.dns.upstream, "1.1.1.2");
    }

    #[test]
    fn empty_object_gets_safe_defaults() {
        let p: Policy = serde_json::from_str("{}").unwrap();
        assert!(p.dns.is_default_deny());
        assert!(p.firewall.is_default_deny());
        assert_eq!(p.dns.mode, "default_deny");
    }

    #[test]
    fn lockdown_and_pin_absent_by_default_but_parse_when_present() {
        // Absent: skipped on serialize so presets stay byte-identical.
        let p: Policy = serde_json::from_str("{}").unwrap();
        assert!(p.lockdown.is_default());
        assert!(!p.lockdown.any());
        assert!(p.parent_pin_hash.is_none());
        let s = serde_json::to_string(&p).unwrap();
        assert!(!s.contains("lockdown"), "empty lockdown must not serialize");
        assert!(!s.contains("parent_pin_hash"));

        // Present: round-trips.
        let raw = r#"{ "lockdown": { "block_tor": true, "block_doh": true },
                      "parent_pin_hash": "argon2$abc" }"#;
        let p: Policy = serde_json::from_str(raw).unwrap();
        assert!(p.lockdown.block_tor && p.lockdown.block_doh);
        assert!(!p.lockdown.block_vpn);
        assert!(p.lockdown.any());
        assert_eq!(p.parent_pin_hash.as_deref(), Some("argon2$abc"));
    }

    #[test]
    fn round_trip_is_stable() {
        // The server normalizes stored policies by round-tripping through this
        // type; serialize(deserialize(x)) must itself re-deserialize cleanly.
        let p: Policy = serde_json::from_str("{}").unwrap();
        let s = serde_json::to_string(&p).unwrap();
        let p2: Policy = serde_json::from_str(&s).unwrap();
        assert_eq!(serde_json::to_string(&p2).unwrap(), s);
    }
}
