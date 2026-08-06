//! Parent companion pairing + client.
//!
//! A parent runs `openscreentime pair --server <url> --token <token>` on their
//! OWN machine (as themselves, no root) to store a scoped parent access token.
//! The tray companion (feature `tray`) then reads it and, in parent mode, polls
//! the server's `/api/parent/*` surface to show pending time requests and
//! recent alerts — and lets the parent approve/deny right from the tray menu.
//!
//! The token is minted in the web console (Settings → Parent access) and can be
//! revoked there at any time. It's stored `0600` in `~/.config/sentinel/`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The paired parent credential + where to reach the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParentConfig {
    pub server_url: String,
    pub token: String,
}

/// `~/.config/sentinel/parent.toml` (honoring `XDG_CONFIG_HOME`).
///
/// An empty env var counts as unset — otherwise `XDG_CONFIG_HOME=""` (which some
/// launchers export) would resolve to a *relative* path and drop the config in
/// the current directory instead of the home config dir.
pub fn config_path() -> Option<PathBuf> {
    let non_empty = |k: &str| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    };
    let base =
        non_empty("XDG_CONFIG_HOME").or_else(|| non_empty("HOME").map(|h| h.join(".config")))?;
    Some(base.join("sentinel").join("parent.toml"))
}

impl ParentConfig {
    #[cfg(feature = "tray")]
    pub fn load() -> Option<Self> {
        let body = std::fs::read_to_string(config_path()?).ok()?;
        toml::from_str(&body).ok()
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path().context("no HOME/XDG_CONFIG_HOME to write the parent config")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))?;
        crate::config::set_owner_only_600(&path);
        Ok(())
    }
}

/// `openscreentime pair` — store a parent pairing token for the tray companion.
pub fn pair(server: &str, token: &str) -> Result<()> {
    let server_url = server.trim().trim_end_matches('/').to_string();
    let token = token.trim().to_string();
    if server_url.is_empty() || token.is_empty() {
        anyhow::bail!("both --server and --token are required");
    }
    ParentConfig { server_url, token }.save()?;
    let path = config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    println!("Paired — saved to {path}");
    println!("Start the companion:  systemctl --user enable --now sentinel-tray");
    Ok(())
}

/// The HTTP client for the parent surface. Only needed by the tray.
#[cfg(feature = "tray")]
pub mod api {
    use super::ParentConfig;
    use serde::Deserialize;

    /// A pending time request, from `GET /api/parent/earn-requests`.
    #[derive(Debug, Clone, Deserialize)]
    pub struct PendingReq {
        pub id: String,
        pub os_username: String,
        #[serde(default)]
        pub user_display_name: Option<String>,
        pub task_label: String,
        pub minutes: i64,
        pub device_name: String,
    }

    impl PendingReq {
        /// A short who-line for the menu/notification.
        pub fn who(&self) -> &str {
            self.user_display_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(&self.os_username)
        }
    }

    /// A recent alert, from `GET /api/parent/alerts` (warn + critical events).
    #[derive(Debug, Clone, Deserialize)]
    pub struct Alert {
        pub id: String,
        #[serde(rename = "type")]
        pub etype: String,
        pub severity: String,
        #[serde(default)]
        pub payload: serde_json::Value,
    }

    pub async fn pending(
        client: &reqwest::Client,
        cfg: &ParentConfig,
    ) -> anyhow::Result<Vec<PendingReq>> {
        #[derive(Deserialize)]
        struct Wrap {
            requests: Vec<PendingReq>,
        }
        let url = format!("{}/api/parent/earn-requests", cfg.server_url);
        let wrap: Wrap = client
            .get(url)
            .bearer_auth(&cfg.token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(wrap.requests)
    }

    pub async fn alerts(
        client: &reqwest::Client,
        cfg: &ParentConfig,
    ) -> anyhow::Result<Vec<Alert>> {
        #[derive(Deserialize)]
        struct Wrap {
            alerts: Vec<Alert>,
        }
        let url = format!("{}/api/parent/alerts", cfg.server_url);
        let wrap: Wrap = client
            .get(url)
            .bearer_auth(&cfg.token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(wrap.alerts)
    }

    pub async fn decide(
        client: &reqwest::Client,
        cfg: &ParentConfig,
        id: &str,
        approve: bool,
    ) -> anyhow::Result<()> {
        let verb = if approve { "approve" } else { "deny" };
        let url = format!("{}/api/parent/earn-requests/{id}/{verb}", cfg.server_url);
        client
            .post(url)
            .bearer_auth(&cfg.token)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}
