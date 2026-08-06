//! `install-service` and `status` subcommands. Installs the hardened systemd unit,
//! the watchdog timer, and the polkit rule (TAMPER.md level 1), then enables them.

use crate::config::{AgentConfig, AgentCtx};
use crate::tamper;
use crate::util::Exec;
use anyhow::Result;
use std::sync::Arc;

const UNIT: &str = include_str!("../systemd/openscreentime-agent.service");
const WATCHDOG_SERVICE: &str = include_str!("../systemd/openscreentime-watchdog.service");
const WATCHDOG_TIMER: &str = include_str!("../systemd/openscreentime-watchdog.timer");
const TRAY_UNIT: &str = include_str!("../systemd/openscreentime-tray.service");

/// The unit names, defined once. They are referenced by the self-updater
/// (restart after swapping the binary) and by tamper level 3 (masking
/// `systemctl stop`); a typo in either is silent — the update never restarts,
/// or the mask protects a unit that does not exist.
pub const AGENT_UNIT: &str = "openscreentime-agent.service";
pub const WATCHDOG_UNIT: &str = "openscreentime-watchdog.service";
pub const WATCHDOG_TIMER_UNIT: &str = "openscreentime-watchdog.timer";
pub const TRAY_UNIT_NAME: &str = "openscreentime-tray.service";

const UNIT_PATH: &str = "/etc/systemd/system/openscreentime-agent.service";
const WATCHDOG_SVC_PATH: &str = "/etc/systemd/system/openscreentime-watchdog.service";
const WATCHDOG_TIMER_PATH: &str = "/etc/systemd/system/openscreentime-watchdog.timer";
const TRAY_UNIT_PATH: &str = "/etc/systemd/user/openscreentime-tray.service";

pub const BIN_TARGET: &str = "/usr/local/bin/openscreentime";
/// Short alias, symlinked next to the binary. `ost time` is what a person (or
/// a plugin shelling out) actually types.
pub const BIN_ALIAS: &str = "/usr/local/bin/ost";
/// The name the binary had when the product was called Sentinel. Kept as a
/// symlink so anything already invoking it — a cron entry, a script, muscle
/// memory — keeps working.
pub const LEGACY_BIN: &str = "/usr/local/bin/sentinel-agent";

/// Units installed under the previous product name.
///
/// These MUST be stopped and removed during install: their ExecStart still
/// points at the old binary path, and two agents enforcing on one host means
/// two processes fighting over nftables, resolv.conf and the cgroup freezer.
/// That is not a cosmetic leftover — it is a device that locks and unlocks
/// itself in a loop.
const LEGACY_SYSTEM_UNITS: &[&str] = &["sentinel-agent.service", "sentinel-watchdog.timer"];
const LEGACY_SYSTEM_UNIT_PATHS: &[&str] = &[
    "/etc/systemd/system/sentinel-agent.service",
    "/etc/systemd/system/sentinel-watchdog.service",
    "/etc/systemd/system/sentinel-watchdog.timer",
];
const LEGACY_TRAY_UNIT: &str = "sentinel-tray.service";
const LEGACY_TRAY_UNIT_PATH: &str = "/etc/systemd/user/sentinel-tray.service";

/// Retire the previous name's units before installing the new ones.
fn retire_legacy_units(exec: &Exec) {
    let mut found = false;
    for unit in LEGACY_SYSTEM_UNITS {
        if std::path::Path::new(&format!("/etc/systemd/system/{unit}")).exists() {
            found = true;
            let _ = exec.run("systemctl", &["disable", "--now", unit]);
        }
    }
    if std::path::Path::new(LEGACY_TRAY_UNIT_PATH).exists() {
        found = true;
        let _ = exec.run("systemctl", &["--global", "disable", LEGACY_TRAY_UNIT]);
    }
    if exec.dry_run() {
        return;
    }
    for path in LEGACY_SYSTEM_UNIT_PATHS {
        let _ = std::fs::remove_file(path);
    }
    let _ = std::fs::remove_file(LEGACY_TRAY_UNIT_PATH);
    if found {
        tracing::info!("retired the previous name's systemd units");
    }
}

/// Point `ost` and the old `sentinel-agent` name at the installed binary.
///
/// Removes whatever is there first: after an upgrade `sentinel-agent` is a real
/// file (the previous release), and symlink() will not overwrite it.
fn link_aliases(exec: &Exec) {
    for alias in [BIN_ALIAS, LEGACY_BIN] {
        if exec.dry_run() {
            tracing::info!(target: "dry_run", "WOULD LINK {alias} → {BIN_TARGET}");
            continue;
        }
        let _ = std::fs::remove_file(alias);
        if let Err(e) = std::os::unix::fs::symlink(BIN_TARGET, alias) {
            tracing::warn!("could not link {alias} → {BIN_TARGET}: {e}");
        }
    }
}

pub fn install_service(ctx: Arc<AgentCtx>) -> Result<()> {
    ctx.require_root_for_enforcement()?;
    let exec = Exec::new(ctx.clone());

    // Before anything else: never leave the old agent running alongside the new
    // one, and never let an upgrade start from an empty usage ledger.
    retire_legacy_units(&exec);
    if !exec.dry_run() {
        crate::paths::migrate_state_dir();
    }

    // Copy our own binary into place so ExecStart path is stable.
    if let Ok(self_exe) = std::env::current_exe() {
        let self_exe = self_exe.to_string_lossy().to_string();
        if self_exe != BIN_TARGET {
            let _ = exec.run("install", &["-m", "0755", &self_exe, BIN_TARGET]);
        }
    }
    link_aliases(&exec);

    exec.write_file(UNIT_PATH, UNIT)?;
    exec.write_file(WATCHDOG_SVC_PATH, WATCHDOG_SERVICE)?;
    exec.write_file(WATCHDOG_TIMER_PATH, WATCHDOG_TIMER)?;
    // Drop the per-user tray unit. On a desktop (tray-featured) build, enable
    // it GLOBALLY so it starts in every user's graphical session at next login
    // — the child must not have to run `systemctl --user enable` to see their
    // own time meter and notifications; on a headless build the `tray`
    // subcommand doesn't exist, so the unit is installed but left disabled.
    if let Err(e) = exec.write_file(TRAY_UNIT_PATH, TRAY_UNIT) {
        tracing::warn!("could not install {TRAY_UNIT_PATH}: {e}");
    }
    tamper::install_polkit(&exec, 1)?;

    exec.run("systemctl", &["daemon-reload"])?;
    exec.run("systemctl", &["enable", "--now", AGENT_UNIT])?;
    exec.run("systemctl", &["enable", "--now", WATCHDOG_TIMER_UNIT])?;
    // `--global` writes the enable symlink into /etc/systemd/user/…wants, so it
    // applies to every user session without one being active during install.
    // Not `--now`: there may be no logged-in user to start it for right now.
    if cfg!(feature = "tray") {
        if let Err(e) = exec.run("systemctl", &["--global", "enable", TRAY_UNIT_NAME]) {
            tracing::warn!("could not globally enable the tray unit: {e}");
        } else {
            tracing::info!("tray unit enabled globally (starts in each graphical session)");
        }
    }

    tracing::info!("hardened unit + watchdog + polkit installed and enabled");
    println!("Installed openscreentime-agent.service (hardened) + watchdog timer.");
    println!("Try `ost time` to see today's screen time.");
    Ok(())
}

pub fn status() -> Result<()> {
    match AgentConfig::load() {
        Ok(cfg) => {
            println!("Enrolled");
            println!("  server      {}", cfg.server_url);
            println!("  device      {}", cfg.device_id);
            println!("  tamper      level {}", cfg.tamper_level);
            println!("  poll        {}s", cfg.poll_interval_secs);
        }
        Err(_) => {
            println!(
                "Not enrolled — no {}. Run `ost enroll --server … --token …`.",
                crate::config::CONFIG_PATH
            );
        }
    }
    println!("  root        {}", crate::config::is_root());
    // Best-effort service state (read-only; safe even non-root).
    let out = std::process::Command::new("systemctl")
        .args(["is-active", AGENT_UNIT])
        .output();
    if let Ok(o) = out {
        println!(
            "  service     {}",
            String::from_utf8_lossy(&o.stdout).trim()
        );
    }
    Ok(())
}

/// Machine-readable `status`, for scripts and plugin integrations.
///
/// Deliberately never includes `device_token`: this is the one subcommand a
/// user is most likely to pipe somewhere, and the token is a bearer credential.
pub fn status_json() -> serde_json::Value {
    let cfg = AgentConfig::load().ok();
    let service = std::process::Command::new("systemctl")
        .args(["is-active", AGENT_UNIT])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    serde_json::json!({
        "enrolled": cfg.is_some(),
        "server_url": cfg.as_ref().map(|c| c.server_url.clone()),
        "device_id": cfg.as_ref().map(|c| c.device_id.clone()),
        "tamper_level": cfg.as_ref().map(|c| c.tamper_level),
        "poll_interval_secs": cfg.as_ref().map(|c| c.poll_interval_secs),
        "config_path": crate::config::config_path_for_read().to_string_lossy(),
        "root": crate::config::is_root(),
        "service": service,
    })
}
