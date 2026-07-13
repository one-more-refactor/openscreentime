//! Policy wire types. The shared `Policy` document itself lives in the
//! `sentinel-policy` crate (also used by the server — one definition, no
//! drift); this module re-exports it and adds the agent-side
//! `GET /agent/policy` response envelope.

pub use sentinel_policy::*;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Where the last-applied effective policy is cached on disk. Enforcement
/// itself never depends on this file (it's an in-memory `HashMap<String,
/// Policy>` in the running `Agent`); it exists purely so a separate CLI
/// invocation — `sentinel-agent unlock` — can verify the parent PIN and know
/// what to tear down even without a live agent process or server connection.
pub const POLICY_CACHE_PATH: &str = "/etc/sentinel/policy_cache.json";

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
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
    crate::config::set_owner_only_600(path);
    Ok(())
}

/// Load the cached effective policy (used by `sentinel-agent unlock`).
pub fn load_cache() -> Result<Policy> {
    let body = std::fs::read_to_string(POLICY_CACHE_PATH).with_context(|| {
        format!(
            "reading {POLICY_CACHE_PATH} (no cached policy — has the agent ever applied one?)"
        )
    })?;
    serde_json::from_str(&body).context("parsing cached policy")
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
