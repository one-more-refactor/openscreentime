//! Agent self-update: once shortly after startup and then once a day, fetch
//! `GET {server}/api/agent/latest` and, if the server bundles a newer headless
//! build for this arch, swap it in atomically and restart the service.
//!
//! Trust model v1 (docs/CONTRACT-PROD.md): the manifest's sha256 over TLS from
//! the enrolled server. A compromised server therefore compromises the fleet —
//! which is already true (it can push root commands); v2 should pin a minisign
//! key so binaries are verified independently of the transport.
//!
//! Safety rails:
//!   * only runs when this process IS `/usr/local/bin/openscreentime` (never
//!     self-updates a dev `cargo run`),
//!   * gated by `auto_update = true` in agent.toml AND the
//!     `SENTINEL_NO_SELF_UPDATE=1` env kill switch,
//!   * only for a headless x86_64 build (the only artifact the image ships),
//!   * download → verify sha256 of the exact bytes → chmod 0755 → keep the old
//!     binary as `openscreentime.bak` (manual rollback) → atomic rename,
//!   * restart goes through `Exec` so `--dry-run` is honored end to end.

use crate::client::ServerClient;
use crate::config::AgentConfig;
use crate::protocol::SEV_INFO;
use crate::util::Exec;
use anyhow::{Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Where install.sh / install-service put the managed binary.
const INSTALL_PATH: &str = "/usr/local/bin/openscreentime";
const STAGING_PATH: &str = "/usr/local/bin/.openscreentime.new";
const BACKUP_PATH: &str = "/usr/local/bin/openscreentime.bak";

/// First check ~2 minutes after startup (catch up quickly after an offline
/// stretch), then once a day.
pub const FIRST_CHECK: std::time::Duration = std::time::Duration::from_secs(120);
pub const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    artifacts: Vec<Artifact>,
}

#[derive(Debug, Deserialize)]
struct Artifact {
    target: String,
    features: String,
    url: String,
    sha256: String,
}

/// The (target, features) pair this build must update *from* — a desktop
/// (gui/tray) build must pull the glibc desktop artifact, never the musl
/// headless one, or self-update would silently swap the child's tray and
/// lockout overlay out from under them. Keyed off the compiled features so the
/// two variants can never cross the streams.
#[cfg(any(feature = "gui", feature = "tray"))]
const SELF_TARGET: &str = "x86_64-linux-gnu";
#[cfg(any(feature = "gui", feature = "tray"))]
const SELF_FEATURES: &str = "desktop";
#[cfg(not(any(feature = "gui", feature = "tray")))]
const SELF_TARGET: &str = "x86_64-linux-musl";
#[cfg(not(any(feature = "gui", feature = "tray")))]
const SELF_FEATURES: &str = "headless";

/// Whether this build/process is allowed to self-update at all.
fn enabled(cfg: &AgentConfig) -> bool {
    if !cfg.auto_update {
        return false;
    }
    if std::env::var("SENTINEL_NO_SELF_UPDATE").map(|v| v == "1") == Ok(true) {
        tracing::debug!("self-update disabled via SENTINEL_NO_SELF_UPDATE=1");
        return false;
    }
    // Only x86_64 is built; another arch must never overwrite itself with it.
    // The gui/tray builds DO self-update now (from the desktop artifact) — the
    // desktop build is what the managed laptop actually runs, and pinning it to
    // its install-time version is how devices keep known lockout bugs forever.
    if cfg!(not(target_arch = "x86_64")) {
        return false;
    }
    // Never self-update a dev `cargo run` — only the installed binary.
    match std::env::current_exe() {
        Ok(p) => p == std::path::Path::new(INSTALL_PATH),
        Err(_) => false,
    }
}

/// Simple semver-ish parse: "1.2.3" → (1, 2, 3). Anything unparsable sorts as
/// 0 so a malformed manifest can never look "newer".
fn parse_version(v: &str) -> (u64, u64, u64) {
    let mut it = v.trim().split('.').map(|p| {
        p.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0)
    });
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// One update check. Returns `Ok(true)` when an update was installed and a
/// restart was issued (the current process is about to die).
pub async fn check_and_update(
    cfg: &AgentConfig,
    client: &ServerClient,
    exec: &Exec,
) -> Result<bool> {
    if !enabled(cfg) {
        return Ok(false);
    }

    let base = cfg.server_url.trim_end_matches('/');
    // Own HTTP client: ServerClient's 10 s budget fits API calls, not a binary
    // download on a slow line. Same TLS stack (rustls via reqwest).
    let http = reqwest::Client::builder()
        .user_agent(format!("openscreentime/{}", crate::client::AGENT_VERSION))
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    let manifest: Manifest = http
        .get(format!("{base}/api/agent/latest"))
        .send()
        .await
        .context("GET /api/agent/latest")?
        .error_for_status()?
        .json()
        .await
        .context("decoding agent manifest")?;

    let current = crate::client::AGENT_VERSION;
    if parse_version(&manifest.version) <= parse_version(current) {
        tracing::debug!(
            "self-update: server has {} — already on {current}",
            manifest.version
        );
        return Ok(false);
    }
    let Some(art) = manifest
        .artifacts
        .iter()
        .find(|a| a.target == SELF_TARGET && a.features == SELF_FEATURES)
    else {
        tracing::debug!(
            "self-update: no matching {SELF_FEATURES} {SELF_TARGET} artifact in manifest"
        );
        return Ok(false);
    };

    // The binary download must come from the ENROLLED server over the same
    // origin as the API. A manifest must not be able to point this root-installed
    // fetch at an arbitrary host or downgrade it to plaintext http — that would
    // widen a fleet-wide root-RCE surface well beyond the server we already trust.
    let url = if art.url.starts_with('/') {
        format!("{base}{}", art.url)
    } else {
        let (Ok(want), Ok(got)) = (reqwest::Url::parse(base), reqwest::Url::parse(&art.url)) else {
            anyhow::bail!("self-update: unparseable artifact URL — refusing");
        };
        let same_origin = want.scheme() == got.scheme()
            && want.host_str() == got.host_str()
            && want.port_or_known_default() == got.port_or_known_default();
        if !same_origin {
            anyhow::bail!(
                "self-update: artifact URL {} is not on the enrolled server origin — refusing",
                art.url
            );
        }
        art.url.clone()
    };
    tracing::info!(
        "self-update: {current} → {} — downloading {url}",
        manifest.version
    );
    let bytes = http
        .get(&url)
        .send()
        .await
        .context("downloading agent update")?
        .error_for_status()?
        .bytes()
        .await?;

    // Verify the sha256 of the exact bytes we're about to install.
    let got = hex::encode(Sha256::digest(&bytes));
    if !got.eq_ignore_ascii_case(art.sha256.trim()) {
        anyhow::bail!(
            "self-update sha256 mismatch (manifest {}, downloaded {got}) — refusing",
            art.sha256
        );
    }

    if exec.dry_run() {
        tracing::info!(
            "DRY-RUN: would install agent {} over {INSTALL_PATH} and restart",
            manifest.version
        );
        return Ok(false);
    }

    // Stage next to the target (same filesystem → atomic rename), 0755, keep
    // the old binary as .bak for manual rollback.
    std::fs::write(STAGING_PATH, &bytes).with_context(|| format!("writing {STAGING_PATH}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(STAGING_PATH, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::copy(INSTALL_PATH, BACKUP_PATH).with_context(|| format!("writing {BACKUP_PATH}"))?;
    std::fs::rename(STAGING_PATH, INSTALL_PATH)
        .with_context(|| format!("renaming into {INSTALL_PATH}"))?;

    // Tell the server BEFORE restarting (the restart kills this process).
    let ev = crate::tamper::tamper_event(
        "agent_updated",
        SEV_INFO,
        &format!("agent self-updated {current} → {}", manifest.version),
    );
    if let Err(e) = client.post_events(&[ev]).await {
        tracing::warn!("could not report agent_updated event: {e}");
    }

    tracing::info!(
        "self-update installed {} — restarting service",
        manifest.version
    );
    exec.run("systemctl", &["restart", crate::service::AGENT_UNIT])?;
    Ok(true)
}

/// Background task: first check after [`FIRST_CHECK`], then every
/// [`CHECK_INTERVAL`]. Spawned by `runner::run`.
pub async fn update_loop(cfg: AgentConfig, client: ServerClient, exec: Exec) {
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        match check_and_update(&cfg, &client, &exec).await {
            Ok(true) => return, // restart issued; nothing left to do
            Ok(false) => {}
            Err(e) => tracing::warn!("self-update check failed: {e}"),
        }
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn version_ordering() {
        assert!(parse_version("0.2.0") > parse_version("0.1.0"));
        assert!(parse_version("0.1.10") > parse_version("0.1.9"));
        assert!(parse_version("1.0.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("0.1.0"), parse_version("0.1.0"));
        // Pre-release-ish suffixes only keep the leading digits; garbage → 0.
        assert_eq!(parse_version("0.1.2-rc1"), parse_version("0.1.2"));
        assert_eq!(parse_version("junk"), (0, 0, 0));
    }
}
