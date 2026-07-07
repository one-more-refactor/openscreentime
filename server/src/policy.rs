//! Shared `Policy` type — the single most important shared contract.
//!
//! Server stores it (jsonb), web edits it, agent enforces it. Mirrors the
//! document in `docs/API.md` → Policy exactly. Every sub-object is
//! `#[serde(default)]` so unknown/missing fields are tolerated for
//! forward-compat (the same shape must deserialize on web `types.ts` and the
//! agent).

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub dns: Dns,
    #[serde(default)]
    pub firewall: Firewall,
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
            dns: Dns::default(),
            firewall: Firewall::default(),
            screen_time: ScreenTime::default(),
            app_limits: Vec::new(),
            gamification: Gamification::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dns {
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

impl Default for Dns {
    fn default() -> Self {
        Dns {
            mode: default_deny(),
            allowlist: Vec::new(),
            blocklist: Vec::new(),
            safe_search: true,
            upstream: default_upstream(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Firewall {
    #[serde(default = "default_deny")]
    pub mode: String,
    #[serde(default)]
    pub allow_outbound_ports: Vec<u16>,
    #[serde(default)]
    pub allow_inbound_ports: Vec<u16>,
}

impl Default for Firewall {
    fn default() -> Self {
        Firewall {
            mode: default_deny(),
            allow_outbound_ports: vec![53, 80, 443],
            allow_inbound_ports: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScreenTime {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub daily_limit_minutes: u32,
    #[serde(default)]
    pub schedule: Vec<ScheduleWindow>,
    #[serde(default)]
    pub bedtime: Option<Bedtime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleWindow {
    #[serde(default)]
    pub days: Vec<u8>, // 0 = Sunday
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
    #[serde(default = "default_challenge")]
    pub unlock_challenge: String, // "math" | "wait" | "parent_pin"
}

fn default_challenge() -> String {
    "wait".to_string()
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
