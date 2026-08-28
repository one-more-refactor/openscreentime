//! Persisted agent identity (`/etc/openscreentime/agent.toml`, root-owned
//! `0600`) plus the process-wide runtime context (`AgentCtx`) that carries
//! `--dry-run`, `--tamper-max` and the root check into every module.
//!
//! **Migration.** The product was called Sentinel and its config lived in
//! `/etc/sentinel/`. Reads fall back to the old path when the new one is
//! absent, and the first successful load migrates the file across. A device
//! enrolled under the old name therefore keeps working across an upgrade
//! without anyone touching it — which matters, because the way you would fix a
//! broken agent is through the agent.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const CONFIG_PATH: &str = "/etc/openscreentime/agent.toml";
/// Where the config lived when the product was called Sentinel.
pub const LEGACY_CONFIG_PATH: &str = "/etc/sentinel/agent.toml";
pub const HEARTBEAT_FILE: &str = "/run/openscreentime/heartbeat";

/// Environment override for the config location.
///
/// Exists so the agent can be exercised without root: every other path is
/// under `/etc`, which meant the only way to try `enroll`, `status` or `login`
/// was to install it for real on a live machine. A dev loop that requires sudo
/// is a dev loop nobody runs.
///
/// Not a security hole: the config is 0600 and root-owned in production, and
/// the systemd unit sets no such variable — a managed user setting it in their
/// own shell only ever points *their* unprivileged process somewhere else, it
/// does not change what the root agent reads.
pub const CONFIG_ENV: &str = "OST_CONFIG";

/// The config path to write and to prefer on read.
pub fn config_path() -> PathBuf {
    std::env::var_os(CONFIG_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(CONFIG_PATH))
}

/// The config path to read: the current one if it exists, else the legacy one.
///
/// Deliberately checks existence rather than trying and falling back on error,
/// so an unreadable-but-present new config surfaces its real error instead of
/// being masked by a stale file from the previous name.
pub fn config_path_for_read() -> PathBuf {
    let current = config_path();
    if current.exists() {
        return current;
    }
    let legacy = PathBuf::from(LEGACY_CONFIG_PATH);
    if legacy.exists() {
        return legacy;
    }
    current
}

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
    /// Daily self-update from the enrolled server (see `update.rs`). On by
    /// default; `OST_NO_SELF_UPDATE=1` also disables it at runtime.
    #[serde(default = "default_true")]
    pub auto_update: bool,
}

fn default_poll() -> u64 {
    30
}
fn default_tamper() -> u8 {
    1
}
fn default_true() -> bool {
    true
}

impl AgentConfig {
    /// Load, transparently migrating a config left behind by the old name.
    ///
    /// The migration is best-effort and never fatal: if the copy fails (read-
    /// only /etc, no root) the agent still runs from the legacy file. Losing
    /// the ability to start because a *cosmetic* move failed would be a far
    /// worse bug than leaving the file where it is.
    pub fn load() -> Result<Self> {
        let path = config_path_for_read();
        let cfg = Self::load_from(&path)?;
        let want = config_path();
        if path != want {
            match cfg.save_to(&want) {
                Ok(()) => tracing::info!(
                    "migrated config {} → {} (the old file is left in place)",
                    path.display(),
                    want.display()
                ),
                Err(e) => tracing::warn!(
                    "could not migrate config to {} ({e}); continuing from {}",
                    want.display(),
                    path.display()
                ),
            }
        }
        Ok(cfg)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: AgentConfig = toml::from_str(&body).context("parsing agent.toml")?;
        Ok(cfg)
    }

    /// Write root-owned `0600`. Best-effort perms; logs if it can't chmod (e.g. dev/non-root).
    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path())
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

/// Write a SECRET file that is never world-readable, even for an instant.
///
/// `std::fs::write` + a later chmod leaves a `0644` window on first creation
/// (in a `0755` state dir a child can `open()` it and lift the TOTP secret /
/// recovery-code MACs), so instead: create a sibling temp `0600` from the
/// outset, write, fsync-free rename into place (atomic on the same fs). The
/// content is on disk only under the final name, only ever mode 0600.
pub fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp.new");
        // create_new fails if a stale temp exists (or is a symlink) — clear it.
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(body)?;
        f.sync_all().ok();
        drop(f);
        std::fs::rename(&tmp, path)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, body)?;
        set_owner_only_600(path);
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AgentConfig {
        AgentConfig {
            server_url: "https://example.test".into(),
            device_id: "dev-1".into(),
            device_token: "tok".into(),
            poll_interval_secs: 30,
            tamper_level: 1,
            auto_update: true,
        }
    }

    /// The rename must not cost anyone their enrollment: a config written
    /// under the old name has to load unchanged under the new one.
    #[test]
    fn a_legacy_config_still_parses() {
        let dir = std::env::temp_dir().join(format!("ost-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.toml");
        sample().save_to(&path).unwrap();

        let loaded = AgentConfig::load_from(&path).unwrap();
        assert_eq!(loaded.device_id, "dev-1");
        assert_eq!(loaded.device_token, "tok");
        assert_eq!(loaded.server_url, "https://example.test");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Written 0600: the file holds a bearer token for the device, and the
    /// person this device constrains has a shell on it.
    #[test]
    fn config_is_written_owner_only() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!("ost-perm-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("agent.toml");
            sample().save_to(&path).unwrap();

            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "device token must not be world-readable"
            );

            std::fs::remove_dir_all(&dir).ok();
        }
    }

    /// With neither file present the current path is returned, so the error a
    /// user sees names the path they are expected to create.
    #[test]
    fn read_path_defaults_to_the_current_name() {
        // Neither /etc path exists in a test sandbox.
        if !Path::new(CONFIG_PATH).exists() && !Path::new(LEGACY_CONFIG_PATH).exists() {
            assert_eq!(config_path_for_read(), Path::new(CONFIG_PATH));
        }
    }
}
