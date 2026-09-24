//! Agent self-update: once shortly after startup and then once a day, check
//! for a different build and swap it in atomically, then restart the service.
//!
//! **The enrolled server is the release channel.** Its `/api/agent/latest`
//! describes the agent build bundled with the server image, so a device runs
//! what its own server ships — no split between "what GitHub says" and "what
//! the server expects". GitHub releases are only asked when the server has no
//! bundled build at all (a dev server answering 404).
//!
//! **Builds, not version numbers.** A fix merged without a version bump must
//! still reach devices, so the manifest's `build` (a hash of the agent's
//! source, see Containerfile) is compared with the one compiled into this
//! binary (`OST_BUILD_ID`). Without build ids on both sides it falls back to
//! "newer version". Never downgrades; never re-installs identical bytes.
//!
//! Trust model: the manifest's sha256 over TLS from the enrolled server (which
//! can already push root commands) — v2 should pin a signing key.
//!
//! Safety rails:
//!   * only runs when this process IS `/usr/local/bin/openscreentime` (never
//!     self-updates a dev `cargo run`), gated by `auto_update = true` in
//!     agent.toml and the `OST_NO_SELF_UPDATE=1` kill switch, x86_64 only,
//!   * download → verify sha256 of the exact bytes → **preflight**: the staged
//!     binary must run `--version` (a build that can't even load here — say,
//!     it needs a newer glibc — is refused before it replaces anything),
//!   * the old binary is kept as `openscreentime.bak`, and an update-pending
//!     marker is written before the restart. The new build removes the marker
//!     once it has run for a minute. If it crash-loops or stops ticking first,
//!     the watchdog unit puts `.bak` back and restarts — **automatic
//!     rollback** — and that build is skipped from then on,
//!   * a freshly started build refreshes the systemd units if the ones it
//!     carries differ from the installed ones (service.rs),
//!   * restart goes through `Exec` so `--dry-run` is honored end to end.

use crate::client::ServerClient;
use crate::config::AgentConfig;
use crate::protocol::{SEV_INFO, SEV_WARN};
use crate::util::Exec;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

/// Where install.sh / install-service put the managed binary.
const INSTALL_PATH: &str = "/usr/local/bin/openscreentime";
const STAGING_PATH: &str = "/usr/local/bin/.openscreentime.new";
const BACKUP_PATH: &str = "/usr/local/bin/openscreentime.bak";
/// Written just before restarting into a new build; removed by that build once
/// it has proven itself. The watchdog unit reads it (keep the paths in step
/// with systemd/openscreentime-watchdog.service).
const PENDING_PATH: &str = "/var/lib/openscreentime/update-pending.json";
/// A build that was rolled back (by the watchdog) or failed its preflight.
const REJECTED_PATH: &str = "/var/lib/openscreentime/update-rejected.json";

/// First check ~2 minutes after startup (catch up quickly after an offline
/// stretch), then once a day.
pub const FIRST_CHECK: Duration = Duration::from_secs(120);
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// A new build that has run this long is kept.
const CONFIRM_AFTER: Duration = Duration::from_secs(60);

/// Identity of this build's source (set by the Containerfile / CI when
/// building release artifacts; absent in dev builds).
pub const BUILD_ID: Option<&str> = option_env!("OST_BUILD_ID");

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    /// Hash of the agent source the artifacts were built from.
    #[serde(default)]
    build: Option<String>,
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

/// The update-pending marker, and (renamed by the watchdog) the rejected one.
#[derive(Debug, Serialize, Deserialize)]
struct Pending {
    from: String,
    to: String,
    sha256: String,
    /// Whether the server has been told about a rejection.
    #[serde(default)]
    reported: bool,
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

/// Where releases live — only consulted when the enrolled server bundles no
/// agent. A fork can point its fleet elsewhere with `OST_UPDATE_REPO`.
const GITHUB_REPO: &str = "one-more-refactor/openscreentime";

fn github_repo() -> String {
    std::env::var("OST_UPDATE_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| GITHUB_REPO.to_string())
}

/// The newest GitHub release's manifest and its download base
/// (`https://github.com/<repo>/releases/download/<tag>`).
async fn github_manifest(http: &reqwest::Client) -> Result<(Manifest, String)> {
    let repo = github_repo();
    let releases: serde_json::Value = http
        .get(format!(
            "https://api.github.com/repos/{repo}/releases?per_page=1"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("GET github releases")?
        .error_for_status()?
        .json()
        .await?;
    let rel = releases
        .get(0)
        .ok_or_else(|| anyhow::anyhow!("no releases on {repo}"))?;
    let tag = rel
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("release without a tag"))?;
    let murl = rel
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|a| {
                (a.get("name").and_then(|n| n.as_str()) == Some("manifest.json"))
                    .then(|| a.get("browser_download_url").and_then(|u| u.as_str()))
                    .flatten()
            })
        })
        .ok_or_else(|| anyhow::anyhow!("release {tag} has no manifest.json"))?;
    let manifest: Manifest = http
        .get(murl)
        .send()
        .await
        .context("GET release manifest")?
        .error_for_status()?
        .json()
        .await
        .context("decoding release manifest")?;
    Ok((
        manifest,
        format!("https://github.com/{repo}/releases/download/{tag}"),
    ))
}

/// The enrolled server's manifest; GitHub's only if the server has none.
/// Returns the GitHub download base when that is where it came from.
async fn fetch_manifest(http: &reqwest::Client, base: &str) -> Result<(Manifest, Option<String>)> {
    let resp = http
        .get(format!("{base}/api/agent/latest"))
        .send()
        .await
        .context("GET /api/agent/latest")?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        tracing::debug!("the server bundles no agent build; asking GitHub releases");
        let (m, dl_base) = github_manifest(http).await?;
        return Ok((m, Some(dl_base)));
    }
    let m = resp
        .error_for_status()?
        .json()
        .await
        .context("decoding agent manifest")?;
    Ok((m, None))
}

/// Whether this build/process is allowed to self-update at all.
fn enabled(cfg: &AgentConfig) -> bool {
    if !cfg.auto_update {
        return false;
    }
    if std::env::var("OST_NO_SELF_UPDATE").map(|v| v == "1") == Ok(true) {
        tracing::debug!("self-update disabled via OST_NO_SELF_UPDATE=1");
        return false;
    }
    // Only x86_64 is built; another arch must never overwrite itself with it.
    if cfg!(not(target_arch = "x86_64")) {
        return false;
    }
    is_installed_binary()
}

/// Never touch a dev `cargo run` — only the installed binary.
fn is_installed_binary() -> bool {
    matches!(std::env::current_exe(), Ok(p) if p == std::path::Path::new(INSTALL_PATH))
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

/// What this device runs, as far as the update decision is concerned.
struct Running<'a> {
    version: &'a str,
    build: Option<&'a str>,
    sha256: Option<&'a str>,
}

/// Should the offered artifact replace what is running?
fn should_update(
    running: &Running,
    offered_version: &str,
    offered_build: Option<&str>,
    offered_sha: &str,
) -> bool {
    // Identical bytes: nothing to do, whatever the labels say.
    if running
        .sha256
        .is_some_and(|s| s.eq_ignore_ascii_case(offered_sha.trim()))
    {
        return false;
    }
    // Never step back (a server rolled back to an older image, a stale mirror).
    if parse_version(offered_version) < parse_version(running.version) {
        return false;
    }
    match (running.build, offered_build) {
        // Same version number, different source: a fix without a version bump.
        (Some(mine), Some(theirs)) => !mine.eq_ignore_ascii_case(theirs.trim()),
        _ => parse_version(offered_version) > parse_version(running.version),
    }
}

fn sha256_file(path: &str) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|b| hex::encode(Sha256::digest(&b)))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &str) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_json<T: Serialize>(path: &str, v: &T) -> Result<()> {
    std::fs::write(path, serde_json::to_vec(v)?).with_context(|| format!("writing {path}"))
}

/// Run the staged binary's `--version`: it must load and answer. Catches a
/// build that needs a newer glibc (or a library this machine lacks) before it
/// replaces the one that works.
async fn preflight(path: &str) -> Result<()> {
    let out = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(path)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("the new build did not answer --version within 15 s")?
    .context("the new build could not be started")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !stdout.contains("openscreentime") {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!(
            "the new build does not run here ({}): {}",
            out.status,
            stderr.trim().chars().take(300).collect::<String>()
        );
    }
    Ok(())
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
        .timeout(Duration::from_secs(300))
        .build()?;

    let (manifest, github_base) = fetch_manifest(&http, base).await?;
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

    let current = crate::client::AGENT_VERSION;
    let my_sha = sha256_file(INSTALL_PATH);
    let running = Running {
        version: current,
        build: BUILD_ID,
        sha256: my_sha.as_deref(),
    };
    if !should_update(
        &running,
        &manifest.version,
        manifest.build.as_deref(),
        &art.sha256,
    ) {
        tracing::debug!(
            "self-update: server offers {} ({:?}) — keeping {current} ({:?})",
            manifest.version,
            manifest.build,
            BUILD_ID
        );
        return Ok(false);
    }
    if let Some(rej) = read_json::<Pending>(REJECTED_PATH) {
        if rej.sha256.eq_ignore_ascii_case(art.sha256.trim()) {
            tracing::info!(
                "self-update: {} was rolled back here before — waiting for a newer build",
                manifest.version
            );
            return Ok(false);
        }
    }

    // The binary must come from the origin the manifest did — GitHub's
    // release download path, or the ENROLLED server. A manifest must not be
    // able to point this root-installed fetch at an arbitrary host or
    // downgrade it to plaintext http — that would widen a fleet-wide
    // root-RCE surface beyond the origins we already trust.
    let url = if let Some(dl_base) = &github_base {
        // Only the artifact's basename is taken from the manifest; the base
        // is pinned to this release's download path on github.com.
        let file = art.url.rsplit('/').next().unwrap_or_default();
        if file.is_empty()
            || !file
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
        {
            anyhow::bail!("self-update: suspicious artifact filename — refusing");
        }
        format!("{dl_base}/{file}")
    } else if art.url.starts_with('/') {
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

    // Stage next to the target (same filesystem → atomic rename), 0755.
    std::fs::write(STAGING_PATH, &bytes).with_context(|| format!("writing {STAGING_PATH}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(STAGING_PATH, std::fs::Permissions::from_mode(0o755))?;
    }
    let pending = Pending {
        from: current.to_string(),
        to: manifest.version.clone(),
        sha256: got.clone(),
        reported: false,
    };
    if let Err(e) = preflight(STAGING_PATH).await {
        let _ = std::fs::remove_file(STAGING_PATH);
        // Remember it, so this build isn't downloaded (and refused) daily.
        let _ = write_json(
            REJECTED_PATH,
            &Pending {
                reported: true,
                ..pending
            },
        );
        report(
            client,
            "agent_update_refused",
            SEV_WARN,
            &format!(
                "update to {} not installed — it does not run on this device: {e:#}",
                manifest.version
            ),
        )
        .await;
        return Err(e);
    }

    // Keep the old binary for the rollback, mark the update pending, swap.
    std::fs::copy(INSTALL_PATH, BACKUP_PATH).with_context(|| format!("writing {BACKUP_PATH}"))?;
    write_json(PENDING_PATH, &pending)?;
    if let Err(e) = std::fs::rename(STAGING_PATH, INSTALL_PATH) {
        let _ = std::fs::remove_file(PENDING_PATH);
        return Err(e).with_context(|| format!("renaming into {INSTALL_PATH}"));
    }

    // Tell the server BEFORE restarting (the restart kills this process).
    report(
        client,
        "agent_updated",
        SEV_INFO,
        &format!("agent self-updated {current} → {}", manifest.version),
    )
    .await;

    tracing::info!(
        "self-update installed {} — restarting service",
        manifest.version
    );
    exec.run("systemctl", &["restart", crate::service::AGENT_UNIT])?;
    Ok(true)
}

/// Best-effort audit line to the server.
async fn report(client: &ServerClient, kind: &str, severity: &str, message: &str) {
    let ev = crate::tamper::tamper_event(kind, severity, message);
    if let Err(e) = client.post_events(&[ev]).await {
        tracing::warn!("could not report {kind}: {e}");
    }
}

/// Right after starting: if this build was just swapped in, keep it once it
/// has been up for [`CONFIRM_AFTER`]; and tell the server (once) if the
/// watchdog had to roll an update back.
async fn settle_previous_update(client: &ServerClient) {
    if std::path::Path::new(PENDING_PATH).exists() {
        tokio::time::sleep(CONFIRM_AFTER).await;
        // Still here: the new build works. (If it had crash-looped, the
        // watchdog would have restored .bak and moved the marker aside.)
        if std::fs::remove_file(PENDING_PATH).is_ok() {
            tracing::info!("self-update confirmed: this build has been running fine");
        }
    }
    if let Some(mut rej) = read_json::<Pending>(REJECTED_PATH) {
        if !rej.reported {
            report(
                client,
                "agent_update_rolled_back",
                SEV_WARN,
                &format!(
                    "update {} → {} did not start properly on this device and was rolled back",
                    rej.from, rej.to
                ),
            )
            .await;
            rej.reported = true;
            let _ = write_json(REJECTED_PATH, &rej);
        }
    }
}

/// Background task: settle a just-installed update, refresh stale systemd
/// units, then check after [`FIRST_CHECK`] and every [`CHECK_INTERVAL`].
/// Spawned by `runner::run`.
pub async fn update_loop(cfg: AgentConfig, client: ServerClient, exec: Exec) {
    if is_installed_binary() && !exec.dry_run() {
        crate::service::refresh_units_if_stale(&exec);
        settle_previous_update(&client).await;
    }
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        match check_and_update(&cfg, &client, &exec).await {
            Ok(true) => return, // restart issued; nothing left to do
            Ok(false) => {}
            Err(e) => tracing::warn!("self-update check failed: {e:#}"),
        }
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_version, should_update, Running};

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

    fn running<'a>(v: &'a str, b: Option<&'a str>, s: Option<&'a str>) -> Running<'a> {
        Running {
            version: v,
            build: b,
            sha256: s,
        }
    }

    #[test]
    fn a_fix_without_a_version_bump_is_installed() {
        let r = running("0.6.1", Some("aaaa"), Some("11"));
        assert!(should_update(&r, "0.6.1", Some("bbbb"), "22"));
        // Same source rebuilt elsewhere: no churn.
        assert!(!should_update(&r, "0.6.1", Some("aaaa"), "22"));
    }

    #[test]
    fn identical_bytes_are_never_reinstalled() {
        let r = running("0.6.1", Some("aaaa"), Some("abcd"));
        assert!(!should_update(&r, "0.7.0", Some("bbbb"), "ABCD"));
    }

    #[test]
    fn never_downgrades() {
        let r = running("0.7.0", Some("aaaa"), Some("11"));
        assert!(!should_update(&r, "0.6.9", Some("bbbb"), "22"));
    }

    #[test]
    fn without_build_ids_it_goes_by_version() {
        let r = running("0.6.1", None, Some("11"));
        assert!(should_update(&r, "0.6.2", None, "22"));
        assert!(!should_update(&r, "0.6.1", None, "22"));
        // An older server's manifest (no build field) and a new agent.
        let r = running("0.6.1", Some("aaaa"), Some("11"));
        assert!(!should_update(&r, "0.6.1", None, "22"));
        assert!(should_update(&r, "0.6.2", None, "22"));
    }
}
