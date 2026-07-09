//! Policy wire types. The shared `Policy` document itself lives in the
//! `sentinel-policy` crate (also used by the server — one definition, no
//! drift); this module re-exports it and adds the agent-side
//! `GET /agent/policy` response envelope.

pub use sentinel_policy::*;

use serde::{Deserialize, Serialize};

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
