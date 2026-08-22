//! The shared `Policy` document — the single most important shared contract.
//!
//! The server stores it (jsonb), the web edits it (`web/src/types.ts` mirrors
//! this shape), the agent enforces it. Mirrors `docs/API.md` → Policy exactly.
//! Every sub-object is `#[serde(default)]` and unknown fields are ignored, so
//! all components stay forward-compatible with newer policy versions (the docs
//! require this).

use serde::{Deserialize, Serialize};

pub mod catalog;

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
    /// One-click app / category blocks from the built-in catalog (see
    /// `catalog`). Absent = nothing blocked; skipped on serialize when empty
    /// so older presets stay byte-identical.
    #[serde(default, skip_serializing_if = "AppBlocks::is_default")]
    pub blocks: AppBlocks,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            version: 1,
            dns: DnsPolicy::default(),
            firewall: FirewallPolicy::default(),
            screen_time: ScreenTime::default(),
            gamification: Gamification::default(),
            lockdown: NetworkLockdown::default(),
            parent_pin_hash: None,
            blocks: AppBlocks::default(),
        }
    }
}

/// One-click app / category blocks. Ids refer to the built-in catalog
/// (`catalog::apps()` / `catalog::categories()`); unknown ids are kept as-is
/// (forward-compat with a newer catalog) and simply expand to nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppBlocks {
    /// Catalog app ids, e.g. `"youtube"`, `"tiktok"`.
    #[serde(default)]
    pub apps: Vec<String>,
    /// Catalog category ids, e.g. `"social"`, `"adult"`.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Extra domains the parent typed by hand (subdomains included).
    #[serde(default)]
    pub custom_domains: Vec<String>,
}

impl AppBlocks {
    pub fn is_default(&self) -> bool {
        self.apps.is_empty() && self.categories.is_empty() && self.custom_domains.is_empty()
    }
    pub fn is_empty(&self) -> bool {
        self.is_default()
    }
}

/// Age bracket — autonomy scales with age (docs/OPENSCREENTIME.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgeBracket {
    /// 0–6: curated allowlist, parent does everything, no request UI.
    Little,
    /// 6–12: hard limits, can request / earn time.
    Kid,
    /// 12–16: goals + limits, wind-down before a hard stop.
    YoungerTeen,
    /// 16–18: mostly self-set, parent can still cap.
    OlderTeen,
    /// 18+: private self-tracking, self-imposed limits only.
    Adult,
}

impl AgeBracket {
    pub const ALL: [AgeBracket; 5] = [
        AgeBracket::Little,
        AgeBracket::Kid,
        AgeBracket::YoungerTeen,
        AgeBracket::OlderTeen,
        AgeBracket::Adult,
    ];

    /// The wire / DB id (`"younger_teen"`).
    pub fn id(&self) -> &'static str {
        match self {
            AgeBracket::Little => "little",
            AgeBracket::Kid => "kid",
            AgeBracket::YoungerTeen => "younger_teen",
            AgeBracket::OlderTeen => "older_teen",
            AgeBracket::Adult => "adult",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.id() == s)
    }

    pub fn label(&self) -> &'static str {
        match self {
            AgeBracket::Little => "Little",
            AgeBracket::Kid => "Kid",
            AgeBracket::YoungerTeen => "Younger teen",
            AgeBracket::OlderTeen => "Older teen",
            AgeBracket::Adult => "Adult",
        }
    }

    /// Human range, e.g. `"6–12"`.
    pub fn range(&self) -> &'static str {
        match self {
            AgeBracket::Little => "0–6",
            AgeBracket::Kid => "6–12",
            AgeBracket::YoungerTeen => "12–16",
            AgeBracket::OlderTeen => "16–18",
            AgeBracket::Adult => "18+",
        }
    }

    /// Bracket from a birthdate, on a given day. Boundaries: the day you turn
    /// 6 you are a Kid, 12 a YoungerTeen, 16 an OlderTeen, 18 an Adult.
    pub fn from_birthdate(birth: chrono::NaiveDate, today: chrono::NaiveDate) -> Self {
        let years = chrono::Datelike::year(&today) - chrono::Datelike::year(&birth);
        let had_birthday = (
            chrono::Datelike::month(&today),
            chrono::Datelike::day(&today),
        ) >= (
            chrono::Datelike::month(&birth),
            chrono::Datelike::day(&birth),
        );
        let age = if had_birthday { years } else { years - 1 };
        match age {
            i32::MIN..=5 => AgeBracket::Little,
            6..=11 => AgeBracket::Kid,
            12..=15 => AgeBracket::YoungerTeen,
            16..=17 => AgeBracket::OlderTeen,
            _ => AgeBracket::Adult,
        }
    }

    /// The console theme a bracket gets when the parent hasn't picked one.
    pub fn default_theme(&self) -> Theme {
        match self {
            AgeBracket::Little | AgeBracket::Kid => Theme::Playful,
            AgeBracket::YoungerTeen | AgeBracket::OlderTeen => Theme::Calm,
            AgeBracket::Adult => Theme::Plain,
        }
    }

    /// Whether a person in this bracket can ask the parent for more time.
    pub fn can_request_time(&self) -> bool {
        !matches!(self, AgeBracket::Little | AgeBracket::Adult)
    }

    /// Whether the hub enforces anything on this person at all.
    pub fn is_managed(&self) -> bool {
        !matches!(self, AgeBracket::Adult)
    }

    /// Seconds of wind-down countdown before a hard stop (0 = stop at once).
    pub fn wind_down_secs(&self) -> u32 {
        match self {
            AgeBracket::YoungerTeen | AgeBracket::OlderTeen => 120,
            _ => 0,
        }
    }
}

/// How a person's own page looks. `Playful` is the Duolingo-energy one for
/// small children, `Calm` the quieter teen stats page, `Plain` the compact
/// private dashboard for adults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Playful,
    Calm,
    Plain,
}

impl Theme {
    pub fn id(&self) -> &'static str {
        match self {
            Theme::Playful => "playful",
            Theme::Calm => "calm",
            Theme::Plain => "plain",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "playful" => Some(Theme::Playful),
            "calm" => Some(Theme::Calm),
            "plain" => Some(Theme::Plain),
            _ => None,
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
    /// Days the agent may run without reaching the command server before it
    /// escalates to a full parent-PIN lockdown (0 = never escalate). A device
    /// that's been silently cut off is a tamper signal; the parent PIN always
    /// unlocks, so a server/VPS outage can't permanently brick the device.
    #[serde(default)]
    pub offline_lockdown_days: u32,
}

impl NetworkLockdown {
    /// True when every field is at its default — used by `skip_serializing_if`.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
    /// True if any network-filtering rule is active. (Note: this is about the
    /// firewall/DNS bypass rules; `offline_lockdown_days` is a separate
    /// escalation knob and is intentionally excluded.)
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

/// Earn-time tasks and the lockout challenge.
///
/// Streak nudges used to live here too. They were removed deliberately: an app
/// that fires "KEEP YOUR STREAK 🔥" at a child is engagement bait, and the
/// product brief is explicit that this one stays silent unless a human must
/// act. Wind-down warnings ("2 min left") are not streaks and still ship — see
/// `runner::warn_user`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Gamification {
    #[serde(default)]
    pub earn_time: EarnTime,
    #[serde(default)]
    pub lockout: Lockout,
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
    fn blocks_absent_by_default_and_round_trip() {
        let p: Policy = serde_json::from_str("{}").unwrap();
        assert!(p.blocks.is_empty());
        assert!(!serde_json::to_string(&p).unwrap().contains("blocks"));
        let raw = r#"{ "blocks": { "apps": ["youtube"], "categories": ["adult"], "custom_domains": ["example.org"] } }"#;
        let p: Policy = serde_json::from_str(raw).unwrap();
        assert_eq!(p.blocks.apps, vec!["youtube"]);
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("\"blocks\""));
    }

    #[test]
    fn brackets_from_birthdate() {
        use chrono::NaiveDate;
        let today = NaiveDate::from_ymd_opt(2026, 8, 22).unwrap();
        let b =
            |y, m, d| AgeBracket::from_birthdate(NaiveDate::from_ymd_opt(y, m, d).unwrap(), today);
        assert_eq!(b(2022, 1, 1), AgeBracket::Little);
        assert_eq!(b(2020, 8, 22), AgeBracket::Kid); // turns 6 today
        assert_eq!(b(2020, 8, 23), AgeBracket::Little); // turns 6 tomorrow
        assert_eq!(b(2012, 1, 1), AgeBracket::YoungerTeen);
        assert_eq!(b(2009, 1, 1), AgeBracket::OlderTeen);
        assert_eq!(b(2000, 1, 1), AgeBracket::Adult);
        assert_eq!(
            AgeBracket::parse("younger_teen"),
            Some(AgeBracket::YoungerTeen)
        );
        assert_eq!(
            serde_json::to_string(&AgeBracket::YoungerTeen).unwrap(),
            "\"younger_teen\""
        );
        assert_eq!(
            serde_json::to_string(&Theme::Playful).unwrap(),
            "\"playful\""
        );
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
