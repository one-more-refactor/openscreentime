//! Policy wire types. The shared `Policy` document itself lives in the
//! `openscreentime-policy` crate (also used by the server — one definition, no
//! drift); this module re-exports it and adds the agent-side
//! `GET /agent/policy` response envelope.

pub use openscreentime_policy::*;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Where the last-applied effective policy is cached on disk. Enforcement
/// itself never depends on this file (it's an in-memory `HashMap<String,
/// Policy>` in the running `Agent`); it exists purely so a separate CLI
/// invocation — `openscreentime unlock` — can verify the parent PIN and know
/// what to tear down even without a live agent process or server connection.
pub const POLICY_CACHE_PATH: &str = "/etc/openscreentime/policy_cache.json";

/// Best-effort persist of the effective policy (called after every successful
/// `apply_bundle`). Never fails the caller — logs and moves on.
pub fn save_cache(policy: &Policy) {
    if let Err(e) = save_cache_to(policy, std::path::Path::new(POLICY_CACHE_PATH)) {
        tracing::warn!("could not persist policy cache: {e}");
    }
}

fn save_cache_to(policy: &Policy, path: &std::path::Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let body = serde_json::to_string_pretty(policy).context("serializing policy cache")?;
    // 0600 from creation — the cache carries the parent-code TOTP secret and
    // recovery-code MACs; a write-then-chmod left a world-readable window.
    crate::config::write_private(path, body.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Load the cached effective policy (used by `openscreentime unlock`).
pub fn load_cache() -> Result<Policy> {
    let body = std::fs::read_to_string(POLICY_CACHE_PATH).with_context(|| {
        format!("reading {POLICY_CACHE_PATH} (no cached policy — has the agent ever applied one?)")
    })?;
    serde_json::from_str(&body).context("parsing cached policy")
}

/// Where the last-applied *bundle* is cached — the whole per-user map, not just
/// the merged network policy in [`POLICY_CACHE_PATH`].
///
/// This one enforcement DOES depend on. Without it the agent boots with an
/// empty policy map, and if the server is unreachable at that moment it
/// enforces nothing at all: no bedtime, no daily limit, no freezes. Worse,
/// `offline_lockdown_days` — the escalation that exists precisely for "this
/// device has been cut off from the server" — is read out of that same empty
/// map and evaluates to 0, so the countermeasure is disabled by the very
/// condition it was written to catch. Pull the plug on the router, power-cycle
/// the machine, and enforcement is gone until the server comes back.
pub const BUNDLE_CACHE_PATH: &str = "/etc/openscreentime/policy_bundle.json";

/// Best-effort persist of the last applied bundle. Never fails the caller.
pub fn save_bundle_cache(bundle: &PolicyBundle) {
    if let Err(e) = save_bundle_cache_to(bundle, std::path::Path::new(BUNDLE_CACHE_PATH)) {
        tracing::warn!("could not persist policy bundle cache: {e}");
    }
}

fn save_bundle_cache_to(bundle: &PolicyBundle, path: &std::path::Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let body = serde_json::to_string_pretty(bundle).context("serializing policy bundle cache")?;
    // The bundle carries the parent-code secret and VPN private keys — 0600
    // from creation, never a world-readable window.
    crate::config::write_private(path, body.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Load the last applied bundle, for the fail-closed boot path.
pub fn load_bundle_cache() -> Result<PolicyBundle> {
    let body = std::fs::read_to_string(BUNDLE_CACHE_PATH)
        .with_context(|| format!("reading {BUNDLE_CACHE_PATH}"))?;
    serde_json::from_str(&body).context("parsing cached policy bundle")
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
    /// Device-level VPN profile (admin-uploaded wg/ovpn client config), or
    /// `None` when no profile is set — which means "tear the tunnel down",
    /// not "leave it alone" (the bundle is declarative).
    #[serde(default)]
    pub vpn: Option<VpnProfile>,
    /// The device's unlock code: the TOTP secret behind the 6-digit code the
    /// console shows a parent, plus the keyed MACs of the unused one-time
    /// recovery codes — both verified offline by `parentcode`. `None` on a
    /// server that predates 0.4 — a profile's backup code (`parent_pin_hash`)
    /// still works.
    #[serde(default)]
    pub parent_code: Option<ParentCode>,
}

/// Per-device unlock-code material. Transported only over the authenticated
/// agent channel; cached root-only with the bundle. Nobody but this agent and
/// the server ever holds the secret — the parent reads codes off the console.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParentCode {
    /// Base32 TOTP secret (RFC 6238, SHA1, 6 digits, 30 s).
    #[serde(default)]
    pub totp_secret: String,
    /// Unused recovery codes, as `{id, mac}`; the code itself is never sent.
    #[serde(default)]
    pub recovery_codes: Vec<RecoveryCode>,
}

/// One unused recovery code: its server id (reported back when it is spent)
/// and hex HMAC-SHA256(key = decoded TOTP secret, msg = the 8 digits).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryCode {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub mac: String,
}

/// An admin-uploaded VPN client config for this device. The `config` body
/// carries private keys: it is only ever transported over the authenticated
/// agent channel and written to disk `0600` root-only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VpnProfile {
    /// Server-side profile id — test verdicts report back against it.
    #[serde(default)]
    pub id: Option<String>,
    /// `"wireguard"` or `"openvpn"`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub config: String,
    /// `"testing"` asks this agent to verify-then-report before enforcing.
    #[serde(default)]
    pub status: Option<String>,
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

    /// The boot fallback is only as good as what the cache preserves. Rebuilding
    /// the bundle from the runner's `HashMap<String, Policy>` would drop
    /// `profile_kind` and the VPN profile — this pins that we cache verbatim.
    #[test]
    fn bundle_cache_round_trips_every_field() {
        let dir = std::env::temp_dir().join("openscreentime-bundle-cache-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("policy_bundle.json");

        let bundle = PolicyBundle {
            policy_version: "1785818811212".into(),
            device_tamper_level: 3,
            users: vec![UserPolicy {
                os_username: "vali".into(),
                profile_kind: "kids".into(),
                policy: Policy::default(),
            }],
            vpn: Some(VpnProfile {
                id: Some("p1".into()),
                kind: "wireguard".into(),
                config: "[Interface]".into(),
                status: Some("testing".into()),
            }),
            parent_code: Some(ParentCode {
                totp_secret: "GEZDGNBVGY3TQOJQ".into(),
                recovery_codes: vec![RecoveryCode {
                    id: "rc-1".into(),
                    mac: "00".into(),
                }],
            }),
        };

        save_bundle_cache_to(&bundle, &path).expect("write");
        let body = std::fs::read_to_string(&path).expect("read");
        let back: PolicyBundle = serde_json::from_str(&body).expect("parse");

        assert_eq!(back.policy_version, "1785818811212");
        assert_eq!(back.device_tamper_level, 3);
        assert_eq!(back.users.len(), 1);
        assert_eq!(back.users[0].os_username, "vali");
        // The field a hand-rebuilt bundle would have silently lost.
        assert_eq!(back.users[0].profile_kind, "kids");
        assert_eq!(
            back.vpn.as_ref().map(|v| v.kind.as_str()),
            Some("wireguard")
        );
        assert_eq!(
            back.parent_code.as_ref().map(|p| p.totp_secret.as_str()),
            Some("GEZDGNBVGY3TQOJQ")
        );
        assert_eq!(
            back.parent_code.as_ref().map(|p| p.recovery_codes.len()),
            Some(1)
        );

        let _ = std::fs::remove_file(&path);
    }
}
