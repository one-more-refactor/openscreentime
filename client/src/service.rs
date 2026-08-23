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

/// PAM service that makes `sudo` on a managed machine ask for the unlock code
/// (docs/CONTRACT-0.4.md §8). `pam_exec` runs our `pam-auth` helper with the
/// typed token on stdin; it verifies it offline against the device's
/// unlock-code secret / recovery codes / backup code.
pub const PAM_SERVICE_NAME: &str = "openscreentime-parent";
pub const PAM_SERVICE_PATH: &str = "/etc/pam.d/openscreentime-parent";
/// The sudoers drop-in that routes the *managed* OS users through that PAM
/// service and grants them `sudo` — so a parent can administer the machine by
/// typing their code, and the child cannot (they don't have it). The agent
/// rewrites it on every policy apply with the current managed user list.
pub const SUDOERS_PATH: &str = "/etc/sudoers.d/10-openscreentime";
/// Staging name while validating. sudo ignores files whose name contains a
/// dot, so a half-written or invalid drop-in is never parsed.
const SUDOERS_TMP: &str = "/etc/sudoers.d/.10-openscreentime.tmp";

fn pam_service_body() -> String {
    format!(
        "# Managed by openscreentime — do not edit. Removed by `ost uninstall`.\n\
         # sudo for managed users authenticates with the UNLOCK CODE\n\
         # (read off the OpenScreenTime console, or a recovery code),\n\
         # verified offline by the agent.\n\
         auth     required   pam_exec.so expose_authtok quiet {BIN_TARGET} pam-auth\n\
         account  required   pam_permit.so\n\
         session  required   pam_permit.so\n"
    )
}

/// The sudoers drop-in for a set of managed OS users. Empty list → a file
/// with only comments (valid, inert). Usernames are validated to the POSIX
/// portable set so nothing can smuggle sudoers syntax in through a username.
pub fn sudoers_body(managed_users: &[String]) -> String {
    let mut users: Vec<&str> = managed_users
        .iter()
        .map(String::as_str)
        .filter(|u| {
            !u.is_empty()
                && u.len() <= 32
                && u.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
                && !u.starts_with('-')
        })
        .collect();
    users.sort_unstable();
    users.dedup();
    let mut out = String::from(
        "# Managed by openscreentime — rewritten on every policy apply, do not edit.\n\
         # Managed users may sudo, but the password asked for is the UNLOCK CODE\n\
         # (from the OpenScreenTime console). Removed by `ost uninstall`.\n",
    );
    if users.is_empty() {
        out.push_str("# (no managed users on this device right now)\n");
        return out;
    }
    let list = users.join(",");
    out.push_str(&format!(
        "Defaults:{list} pam_service={PAM_SERVICE_NAME}, timestamp_timeout=0\n\
         Defaults:{list} passprompt=\"Unlock code (OpenScreenTime console): \"\n\
         {list} ALL=(ALL:ALL) ALL\n"
    ));
    out
}

/// Write the sudoers drop-in safely: stage under a dot-name (ignored by sudo),
/// validate with `visudo -c -f`, then rename into place. Never leaves a broken
/// file behind — a syntax error in /etc/sudoers.d locks *everyone* out of sudo.
fn write_sudoers(exec: &Exec, body: &str) -> Result<()> {
    if exec.dry_run() {
        tracing::info!(target: "dry_run", "WOULD WRITE {SUDOERS_PATH}:\n{body}");
        return Ok(());
    }
    if std::fs::read_to_string(SUDOERS_PATH).ok().as_deref() == Some(body) {
        return Ok(()); // unchanged
    }
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::remove_file(SUDOERS_TMP);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o440)
            .open(SUDOERS_TMP)?;
        f.write_all(body.as_bytes())?;
    }
    match exec.try_probe("visudo", &["-c", "-f", SUDOERS_TMP]) {
        Some(out) if out.contains("parsed OK") => {}
        Some(out) => {
            let _ = std::fs::remove_file(SUDOERS_TMP);
            anyhow::bail!(
                "sudoers drop-in did not validate, not installed: {}",
                out.trim()
            );
        }
        // No visudo on this box: the body is static and unit-tested; install it.
        None => tracing::warn!("visudo not found — sudoers drop-in installed unvalidated"),
    }
    std::fs::rename(SUDOERS_TMP, SUDOERS_PATH)?;
    Ok(())
}

/// Install the PAM service and an (initially empty) sudoers drop-in.
pub fn install_parent_sudo(exec: &Exec) -> Result<()> {
    exec.write_file(PAM_SERVICE_PATH, &pam_service_body())?;
    write_sudoers(exec, &sudoers_body(&[]))?;
    tracing::info!("parent-code sudo installed ({SUDOERS_PATH}, {PAM_SERVICE_PATH})");
    Ok(())
}

/// Remove the PAM service and sudoers drop-in.
pub fn remove_parent_sudo(exec: &Exec) {
    if exec.dry_run() {
        tracing::info!(target: "dry_run", "WOULD REMOVE {SUDOERS_PATH}, {PAM_SERVICE_PATH}");
        return;
    }
    let _ = std::fs::remove_file(SUDOERS_PATH);
    let _ = std::fs::remove_file(SUDOERS_TMP);
    let _ = std::fs::remove_file(PAM_SERVICE_PATH);
}

/// Is this profile kind under enforcement (→ its OS user's sudo asks for the
/// parent code)? Adults are not; everything else — including the legacy
/// `kids`/`teen` presets and `custom` — is.
pub fn kind_is_managed(profile_kind: &str) -> bool {
    !matches!(profile_kind, "adult" | "default")
}

/// Re-render the sudoers drop-in for the current managed users. Called on
/// every policy apply. Skipped entirely if `install-service` never ran here
/// (no PAM service → nothing to route through).
pub fn sync_managed_sudoers(exec: &Exec, users_by_kind: &[(String, String)]) {
    if !exec.dry_run() && !std::path::Path::new(PAM_SERVICE_PATH).exists() {
        return;
    }
    let managed: Vec<String> = users_by_kind
        .iter()
        .filter(|(_, kind)| kind_is_managed(kind))
        .map(|(u, _)| u.clone())
        .collect();
    if let Err(e) = write_sudoers(exec, &sudoers_body(&managed)) {
        tracing::warn!("could not update {SUDOERS_PATH}: {e}");
    }
}

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
    // sudo on this machine asks for the parent code (CONTRACT-0.4 §8). A
    // failure here must not abort the install of enforcement itself.
    if let Err(e) = install_parent_sudo(&exec) {
        tracing::warn!("parent-code sudo not installed: {e}");
    }

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

/// `ost uninstall`: stop and remove the units, the sudo/PAM hook and the group.
/// The enrollment config and state are left alone (re-running `install-service`
/// picks them right back up); the binary is left in place too.
pub fn uninstall(ctx: Arc<AgentCtx>) -> Result<()> {
    ctx.require_root_for_enforcement()?;
    let exec = Exec::new(ctx);
    let _ = exec.run("systemctl", &["disable", "--now", WATCHDOG_TIMER_UNIT]);
    let _ = exec.run("systemctl", &["disable", "--now", AGENT_UNIT]);
    let _ = exec.run("systemctl", &["--global", "disable", TRAY_UNIT_NAME]);
    if !exec.dry_run() {
        for p in [
            UNIT_PATH,
            WATCHDOG_SVC_PATH,
            WATCHDOG_TIMER_PATH,
            TRAY_UNIT_PATH,
        ] {
            let _ = std::fs::remove_file(p);
        }
    }
    remove_parent_sudo(&exec);
    let _ = exec.run("systemctl", &["daemon-reload"]);
    println!("Removed the OpenScreenTime units and the parent-code sudo hook.");
    println!(
        "Enrollment config ({}) and state were kept.",
        crate::config::CONFIG_PATH
    );
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
    // The keys to this machine: is an unlock code set up, how many spare
    // one-time recovery codes are left. Readable only as root (the bundle
    // cache is 0600), so non-root just sees a dash.
    if crate::config::is_root() {
        let v = crate::parentcode::Verifier::from_device();
        println!(
            "  unlock      {}",
            if v.configured() {
                format!(
                    "code set up · {} recovery code(s) left on this device",
                    v.recovery_codes_left()
                )
            } else {
                "not set up yet (no policy pulled)".to_string()
            }
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The sudoers drop-in is load-bearing for every sudo on the box: pin its
    /// shape so a stray edit cannot ship a file visudo would reject.
    #[test]
    fn sudoers_and_pam_bodies_are_what_we_mean() {
        let s = sudoers_body(&[
            "vali".into(),
            "kid".into(),
            "vali".into(),
            "bad name".into(),
            "-x".into(),
        ]);
        assert!(
            s.contains("Defaults:kid,vali pam_service=openscreentime-parent, timestamp_timeout=0")
        );
        assert!(s.contains("kid,vali ALL=(ALL:ALL) ALL"));
        assert!(!s.contains("bad name") && !s.contains("-x"));
        assert!(s.lines().all(|l| !l.ends_with(' ')));
        // nobody managed → comments only, still a valid file
        let empty = sudoers_body(&[]);
        assert!(empty.lines().all(|l| l.starts_with('#')));
        let p = pam_service_body();
        assert!(p.contains("auth     required   pam_exec.so expose_authtok quiet /usr/local/bin/openscreentime pam-auth"));
        assert!(p.contains("account  required   pam_permit.so"));
    }

    #[test]
    fn adults_are_not_managed_everyone_else_is() {
        assert!(!kind_is_managed("adult"));
        assert!(!kind_is_managed("default"));
        for k in [
            "little",
            "kid",
            "younger_teen",
            "older_teen",
            "kids",
            "teen",
            "custom",
        ] {
            assert!(kind_is_managed(k), "{k}");
        }
    }
}
