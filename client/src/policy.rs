//! The shared `Policy` document, mirrored byte-for-byte from `docs/API.md` and
//! `docs/PROFILES.md`. The server stores it, the web edits it, the agent enforces it.
//!
//! Every sub-object is `#[serde(default)]` and unknown fields are ignored, so the
//! agent stays forward-compatible with newer policy versions (the docs require this).

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
        }
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
    /// `["*"]` means "forward everything to the filtered upstream" (see `default` profile).
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
    pub start: String,
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

/// The `GET /agent/policy` response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyBundle {
    #[serde(default)]
    pub policy_version: String,
    #[serde(default)]
    pub device_tamper_level: u8,
    #[serde(default)]
    pub users: Vec<UserPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPolicy {
    #[serde(default)]
    pub os_username: String,
    #[serde(default)]
    pub profile_kind: String,
    #[serde(default)]
    pub policy: Policy,
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
}
