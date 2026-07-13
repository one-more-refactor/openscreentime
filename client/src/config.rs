//! Persisted agent identity (`/etc/sentinel/agent.toml`, root-owned `0600`) plus the
//! process-wide runtime context (`AgentCtx`) that carries `--dry-run`, `--tamper-max`
//! and the root check into every module.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

pub const CONFIG_PATH: &str = "/etc/sentinel/agent.toml";
pub const HEARTBEAT_FILE: &str = "/run/sentinel/heartbeat";

/// On-disk config written at enrollment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub server_url: String,
    pub device_id: String,
    pub device_token: String,
    #[serde(default = "default_poll")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_tamper")]
    pub tamper_level: u8,
}

fn default_poll() -> u64 {
    30
}
fn default_tamper() -> u8 {
    1
}

impl AgentConfig {
    pub fn load() -> Result<Self> {
        Self::load_from(Path::new(CONFIG_PATH))
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: AgentConfig = toml::from_str(&body).context("parsing agent.toml")?;
        Ok(cfg)
    }

    /// Write root-owned `0600`. Best-effort perms; logs if it can't chmod (e.g. dev/non-root).
    pub fn save(&self) -> Result<()> {
        self.save_to(Path::new(CONFIG_PATH))
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let body = toml::to_string_pretty(self).context("serializing config")?;
        std::fs::write(path, &body).with_context(|| format!("writing {}", path.display()))?;
        set_owner_only_600(path);
        Ok(())
    }
}

/// chmod 0600 (best effort — required by the spec for the token file).
pub fn set_owner_only_600(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!("could not chmod 0600 {}: {}", path.display(), e);
        }
    }
}

/// Process-wide runtime context threaded through every module.
#[derive(Debug, Clone)]
pub struct AgentCtx {
    /// Log actions instead of executing them. MUST be honored everywhere.
    pub dry_run: bool,
    /// Effective tamper ceiling (1 or 3). `--tamper-max` raises it to 3.
    pub tamper_max: u8,
    /// True if euid == 0.
    pub is_root: bool,
    /// Accelerate screen-time accounting for local dev (seconds count faster).
    pub time_accel: u32,
}

impl AgentCtx {
    pub fn new(dry_run: bool, tamper_max: bool, time_accel: u32) -> Arc<Self> {
        Arc::new(AgentCtx {
            dry_run,
            tamper_max: if tamper_max { 3 } else { 1 },
            is_root: is_root(),
            time_accel: time_accel.max(1),
        })
    }

    /// Enforcement (non-dry-run) requires root. Returns Err to be surfaced at CLI top level.
    pub fn require_root_for_enforcement(&self) -> Result<()> {
        if !self.dry_run && !self.is_root {
            anyhow::bail!(
                "refusing to enforce as non-root; re-run with sudo, or use --dry-run to simulate"
            );
        }
        Ok(())
    }
}

pub fn is_root() -> bool {
    users::get_effective_uid() == 0
}
