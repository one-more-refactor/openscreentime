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
/// App-grid launcher for `ost app` (the on-device window) and its icons —
/// the brand's app icon and its single-colour symbolic version, straight from
/// brand/. Only installed on a GUI build: a headless agent has no window.
const DESKTOP_ENTRY: &str = include_str!("../desktop/openscreentime.desktop");
const DESKTOP_ICON: &str = include_str!("../../brand/app-icon.svg");
const DESKTOP_ICON_SYMBOLIC: &str = include_str!("../../brand/app-icon-symbolic.svg");
/// The companion's autostart: every desktop login, with or without a systemd
/// user session (the tray user unit covers the ones with). Tray build only.
const COMPANION_AUTOSTART: &str = include_str!("../desktop/openscreentime-companion.desktop");
const COMPANION_AUTOSTART_PATH: &str = "/etc/xdg/autostart/openscreentime-companion.desktop";

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
/// System-wide (every user's app grid), so a child never has to install
/// anything to open their own window.
const DESKTOP_ENTRY_PATH: &str = "/usr/share/applications/openscreentime.desktop";
const DESKTOP_ICON_PATH: &str = "/usr/share/icons/hicolor/scalable/apps/openscreentime.svg";
const DESKTOP_ICON_SYMBOLIC_PATH: &str =
    "/usr/share/icons/hicolor/symbolic/apps/openscreentime-symbolic.svg";
/// Where older versions opened the window at every login. The companion is
/// the always-on piece now; the window opens from the launcher, from a
/// notification, and once on first run — so this entry is removed.
const RETIRED_APP_AUTOSTART_PATH: &str = "/etc/xdg/autostart/openscreentime-app.desktop";

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
/// Install the app-grid launcher for `ost app` and its icon, system-wide.
/// Best-effort: a device without a working window (or a distro that keeps
/// applications elsewhere) still enforces perfectly; it just lacks the shortcut.
fn install_desktop_entry(exec: &Exec) {
    if let Err(e) = exec.write_file(DESKTOP_ENTRY_PATH, DESKTOP_ENTRY) {
        tracing::warn!("could not install app launcher {DESKTOP_ENTRY_PATH}: {e}");
        return;
    }
    for (path, body) in [
        (DESKTOP_ICON_PATH, DESKTOP_ICON),
        (DESKTOP_ICON_SYMBOLIC_PATH, DESKTOP_ICON_SYMBOLIC),
    ] {
        if let Err(e) = exec.write_file(path, body) {
            tracing::warn!("could not install app icon {path}: {e}");
        }
    }
    // The window is no longer opened at every login (the companion is).
    if !exec.dry_run() {
        let _ = std::fs::remove_file(RETIRED_APP_AUTOSTART_PATH);
    }
    // Refresh the desktop database + icon cache so the entry shows up without a
    // relogin. Both are optional tools; a miss just means it appears next login.
    let _ = exec.run("update-desktop-database", &["/usr/share/applications"]);
    let _ = exec.run(
        "gtk-update-icon-cache",
        &["-q", "-t", "-f", "/usr/share/icons/hicolor"],
    );
    tracing::info!("app launcher installed ({DESKTOP_ENTRY_PATH})");
}

/// Remove the app-grid launcher and icon (called by `uninstall`).
fn remove_desktop_entry(exec: &Exec) {
    for path in [
        DESKTOP_ENTRY_PATH,
        DESKTOP_ICON_PATH,
        DESKTOP_ICON_SYMBOLIC_PATH,
        RETIRED_APP_AUTOSTART_PATH,
        COMPANION_AUTOSTART_PATH,
    ] {
        let _ = exec.remove_file(path);
    }
}

/// The graphical lock: its unprivileged user, its unit, its PAM session and
/// `cage`. GUI build only. Nothing here is fatal: without any of it the agent
/// draws the text lock instead.
fn install_lock(exec: &Exec) {
    ensure_lock_user(exec);
    for (path, body) in [
        (crate::lock::UNIT_TEMPLATE_PATH, crate::lock::UNIT_TEMPLATE),
        (crate::lock::PAM_PATH, crate::lock::PAM_BODY),
    ] {
        if let Err(e) = exec.write_file(path, body) {
            tracing::warn!("could not install {path}: {e} (the text lock will be used)");
        }
    }
    ensure_cage(exec);
}

/// `ost-lock`: a system account (below uid 1000, so it is never listed as a
/// person on this computer), no home, no shell, no password.
fn ensure_lock_user(exec: &Exec) {
    let name = crate::lock::LOCK_USER;
    if users::get_user_by_name(name).is_some() {
        return;
    }
    let nologin = ["/usr/sbin/nologin", "/sbin/nologin", "/usr/bin/nologin"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("/bin/false");
    match exec.run(
        "useradd",
        &[
            "--system",
            "--user-group",
            "--no-create-home",
            "--home-dir",
            "/nonexistent",
            "--shell",
            nologin,
            "--comment",
            "OpenScreenTime lock screen",
            name,
        ],
    ) {
        Ok(_) => tracing::info!("created the {name} system user (the lock screen runs as it)"),
        Err(e) => tracing::warn!("could not create {name}: {e} (the text lock will be used)"),
    }
}

/// Install `cage` (the lock's kiosk compositor) where the distro packages it.
/// Missing cage is not an error: the lock falls back to text mode.
fn ensure_cage(exec: &Exec) {
    if crate::lock::which("cage") {
        return;
    }
    match install_packages(exec, &["cage"]) {
        Some(true) => tracing::info!("installed cage (the graphical lock's compositor)"),
        Some(false) => {
            tracing::warn!("could not install cage; the lock will use its text mode")
        }
        None => tracing::info!(
            "no apt/pacman/dnf here: install `cage` for the graphical lock (the text lock works without it)"
        ),
    }
}

/// The package manager here, and its non-interactive install command.
fn package_manager(exec: &Exec) -> Option<(&'static str, Vec<&'static str>)> {
    let managers: [(&str, &[&str]); 4] = [
        // Waits for a running unattended-upgrade instead of failing on its lock.
        (
            "apt-get",
            &[
                "-o",
                "DPkg::Lock::Timeout=180",
                "install",
                "-y",
                "--no-install-recommends",
            ],
        ),
        ("dnf", &["install", "-y"]),
        ("pacman", &["-S", "--noconfirm", "--needed"]),
        ("zypper", &["--non-interactive", "install"]),
    ];
    managers
        .into_iter()
        .find(|(tool, _)| exec.has(tool))
        .map(|(tool, args)| (tool, args.to_vec()))
}

/// Install distro packages without asking. `None`: no known package manager.
fn install_packages(exec: &Exec, pkgs: &[&str]) -> Option<bool> {
    let (tool, mut args) = package_manager(exec)?;
    args.extend_from_slice(pkgs);
    if exec.dry_run() {
        return Some(exec.run(tool, &args).is_ok());
    }
    let run = |args: &[&str]| {
        std::process::Command::new(tool)
            .args(args)
            .env("DEBIAN_FRONTEND", "noninteractive")
            .stdin(std::process::Stdio::null())
            .output()
            .is_ok_and(|o| o.status.success())
    };
    // A fresh apt box may have no package lists yet.
    Some(run(&args) || (tool == "apt-get" && run(&["update"]) && run(&args)))
}

/// What the network rules shell out to, as (program, package): dnsmasq
/// serves the website rules, nft loads the firewall. A stock desktop (Debian
/// GNOME) has neither — without them a computer is screen-time-only, which
/// it reports as degraded rather than pretending.
const ENFORCEMENT_DEPS: [(&str, &str); 2] = [("dnsmasq", "dnsmasq"), ("nft", "nftables")];

fn installed_marker() -> String {
    crate::paths::state("installed-packages")
        .to_string_lossy()
        .into_owned()
}

/// The packages `install-service` installed here (not ones the computer
/// already had).
pub fn installed_by_us(exec: &Exec) -> Vec<String> {
    exec.read_file(&installed_marker())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// The enforcement packages this computer lacks.
fn missing_enforcement_deps(exec: &Exec) -> Vec<&'static str> {
    ENFORCEMENT_DEPS
        .into_iter()
        .filter(|(program, _)| !exec.has(program))
        .map(|(_, package)| package)
        .collect()
}

/// Install what the network rules need. Returns whether anything was
/// installed. Never fatal: the agent reports what is still missing.
fn ensure_enforcement_deps(exec: &Exec) -> bool {
    let missing = missing_enforcement_deps(exec);
    if missing.is_empty() {
        return false;
    }
    if missing.contains(&"dnsmasq") {
        // The package starts dnsmasq at once; give it a config that can start
        // next to systemd-resolved and reads the agent's rules.
        if let Err(e) = crate::enforce::dns::preseed(exec) {
            tracing::warn!("could not prepare dnsmasq's config: {e}");
        }
    }
    let list = missing.join(" and ");
    match install_packages(exec, &missing) {
        Some(true) => {
            // dnf and pacman leave a new service disabled.
            if missing.contains(&"dnsmasq") {
                let _ = exec.run("systemctl", &["enable", "dnsmasq"]);
            }
            // Remembered, so a retired computer switches off what only we
            // brought (crate::retire) instead of leaving a resolver running.
            let mut ours = installed_by_us(exec);
            ours.extend(missing.iter().map(|p| p.to_string()));
            ours.sort();
            ours.dedup();
            let _ = exec.write_file(&installed_marker(), &(ours.join("\n") + "\n"));
            println!("Installed {list} (for this computer's website and firewall rules).");
            true
        }
        failed => {
            let why = if failed.is_none() {
                "no apt, dnf, pacman or zypper here"
            } else {
                "the package install failed"
            };
            println!(
                "Could not install {list} ({why}). Screen time works; websites and the \
                 firewall are not filtered on this computer until {list} are installed — \
                 the console says so."
            );
            false
        }
    }
}

/// Logins with a desktop open right now (people, not the greeter).
fn graphical_users(exec: &Exec) -> Vec<String> {
    let mut users = Vec::new();
    for line in exec
        .probe("loginctl", &["list-sessions", "--no-legend"])
        .lines()
    {
        let mut cols = line.split_whitespace();
        let (Some(id), Some(uid), Some(user)) = (cols.next(), cols.next(), cols.next()) else {
            continue;
        };
        let person = uid.parse::<u32>().is_ok_and(|u| (1000..65534).contains(&u));
        if !person || users.iter().any(|u| u == user) {
            continue;
        }
        let kind = exec.probe("loginctl", &["show-session", id, "-p", "Type", "--value"]);
        if matches!(kind.trim(), "wayland" | "x11") {
            users.push(user.to_string());
        }
    }
    users
}

/// Start the companion (warnings, "You're back") for everyone already signed
/// in to a desktop. Its autostart only fires at the next login, so a computer
/// set up while a child is using it would otherwise give no warning before
/// the first stop. The companion keeps a single instance, so a second start
/// is harmless.
fn start_companions(exec: &Exec) {
    for user in graphical_users(exec) {
        let machine = format!("{user}@");
        let _ = exec.run("systemctl", &["--user", "-M", &machine, "daemon-reload"]);
        if exec
            .run(
                "systemctl",
                &["--user", "-M", &machine, "start", TRAY_UNIT_NAME],
            )
            .is_ok()
        {
            tracing::info!("started the companion for {user}");
            continue;
        }
        // No systemd user manager to ask: start it in their session directly.
        if exec.dry_run() {
            continue;
        }
        match crate::logincode::spawn_in_session(&user, &["tray"]) {
            Ok(()) => tracing::info!("started the companion in {user}'s session"),
            Err(e) => tracing::warn!("could not start the companion for {user}: {e}"),
        }
    }
}

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
    if exec.dry_run() {
        tracing::info!(target: "dry_run", "WOULD LINK {BIN_ALIAS} → {BIN_TARGET}");
        tracing::info!(target: "dry_run", "WOULD REMOVE legacy alias {LEGACY_BIN}");
        return;
    }
    // The Sentinel→OpenScreenTime rebrand is done: stop carrying the old
    // `sentinel-agent` alias and remove it if a previous install left one.
    let _ = std::fs::remove_file(LEGACY_BIN);
    let _ = std::fs::remove_file(BIN_ALIAS);
    if let Err(e) = std::os::unix::fs::symlink(BIN_TARGET, BIN_ALIAS) {
        tracing::warn!("could not link {BIN_ALIAS} → {BIN_TARGET}: {e}");
    }
}

/// The unit files whose installed copies must follow the binary.
const MANAGED_UNITS: [(&str, &str); 4] = [
    (UNIT_PATH, UNIT),
    (WATCHDOG_SVC_PATH, WATCHDOG_SERVICE),
    (WATCHDOG_TIMER_PATH, WATCHDOG_TIMER),
    (TRAY_UNIT_PATH, TRAY_UNIT),
];

/// Installed units that differ from the ones this build carries. A unit that
/// was never installed here (a manual `run`, no `install-service`) is left
/// alone — this only ever refreshes, it never installs.
fn stale_units() -> Vec<(&'static str, &'static str)> {
    MANAGED_UNITS
        .into_iter()
        .filter(|(path, body)| matches!(std::fs::read_to_string(path), Ok(cur) if cur != *body))
        .collect()
}

/// What a desktop build needs beyond the units `install-service` wrote before
/// the lock existed: the lock's user, unit and PAM session (GUI build) and the
/// companion's autostart (tray build). A self-updated device gets them here,
/// so the new lock and the warnings arrive with the update — no reinstall.
/// Only on a device `install-service` set up.
fn desktop_setup_missing() -> bool {
    if !std::path::Path::new(UNIT_PATH).exists() {
        return false;
    }
    let differs =
        |path: &str, body: &str| std::fs::read_to_string(path).ok().as_deref() != Some(body);
    let lock = cfg!(feature = "gui")
        && (users::get_user_by_name(crate::lock::LOCK_USER).is_none()
            || differs(crate::lock::UNIT_TEMPLATE_PATH, crate::lock::UNIT_TEMPLATE)
            || differs(crate::lock::PAM_PATH, crate::lock::PAM_BODY));
    let companion =
        cfg!(feature = "tray") && differs(COMPANION_AUTOSTART_PATH, COMPANION_AUTOSTART);
    // The launcher and its icons as this build carries them (and no window
    // opening at every login) — on a device that has the launcher at all.
    let app = cfg!(feature = "gui")
        && std::path::Path::new(DESKTOP_ENTRY_PATH).exists()
        && (differs(DESKTOP_ENTRY_PATH, DESKTOP_ENTRY)
            || differs(DESKTOP_ICON_PATH, DESKTOP_ICON)
            || differs(DESKTOP_ICON_SYMBOLIC_PATH, DESKTOP_ICON_SYMBOLIC)
            || std::path::Path::new(RETIRED_APP_AUTOSTART_PATH).exists());
    lock || companion || app
}

/// `ost __refresh-units` (hidden): rewrite the stale units, set up what the
/// desktop build is missing, reload systemd. Runs in a transient unit (see
/// [`refresh_units_if_stale`]), outside the agent's sandbox.
pub fn refresh_units() -> Result<()> {
    let stale = stale_units();
    for (path, body) in &stale {
        std::fs::write(path, body).map_err(|e| anyhow::anyhow!("writing {path}: {e}"))?;
    }
    let exec = Exec::new(AgentCtx::new(false, false, 1));
    // A computer installed before the installer brought dnsmasq/nftables.
    let deps = ensure_enforcement_deps(&exec);
    let companion_new = cfg!(feature = "tray")
        && std::fs::read_to_string(COMPANION_AUTOSTART_PATH)
            .ok()
            .as_deref()
            != Some(COMPANION_AUTOSTART);
    if desktop_setup_missing() {
        if cfg!(feature = "gui") {
            install_lock(&exec);
            if std::path::Path::new(DESKTOP_ENTRY_PATH).exists() {
                install_desktop_entry(&exec);
            }
        }
        if cfg!(feature = "tray") {
            if let Err(e) = exec.write_file(COMPANION_AUTOSTART_PATH, COMPANION_AUTOSTART) {
                tracing::warn!("could not install {COMPANION_AUTOSTART_PATH}: {e}");
            }
        }
        println!("set up the lock screen, the launcher and the companion for this desktop build");
    }
    if companion_new {
        start_companions(&exec);
    }
    if !stale.is_empty() {
        let ok = std::process::Command::new("systemctl")
            .arg("daemon-reload")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        anyhow::ensure!(ok, "systemctl daemon-reload failed");
        println!("refreshed {} systemd unit(s)", stale.len());
    }
    if deps {
        // The agent's sandbox only sees /etc/dnsmasq.d if it existed when the
        // agent started: start it again, now that it does.
        let _ = exec.run("systemctl", &["restart", AGENT_UNIT]);
    }
    Ok(())
}

/// A self-update replaces the binary, but the units were written once by
/// `install-service` — so unit fixes (like the watchdog that rolls a bad
/// update back) would never reach an installed fleet. A freshly started agent
/// calls this: if the installed units differ from the ones it carries, it
/// rewrites them through `systemd-run`, because its own sandbox
/// (ProtectSystem=strict) cannot write /etc/systemd. Takes effect at the next
/// restart; nothing is restarted here.
pub fn refresh_units_if_stale(exec: &Exec) {
    if exec.dry_run()
        || !crate::config::is_root()
        || (stale_units().is_empty()
            && !desktop_setup_missing()
            && missing_enforcement_deps(exec).is_empty())
    {
        return;
    }
    match exec.run(
        "systemd-run",
        &[
            "--quiet",
            "--collect",
            "--unit=openscreentime-refresh-units",
            BIN_TARGET,
            "__refresh-units",
        ],
    ) {
        Ok(_) => tracing::info!(
            "setting up what this build needs (its units, the desktop pieces, dnsmasq/nftables)"
        ),
        Err(e) => tracing::warn!("could not refresh the systemd units: {e}"),
    }
}

pub fn install_service(ctx: Arc<AgentCtx>) -> Result<()> {
    ctx.require_root_for_enforcement()?;
    install_service_with(&Exec::new(ctx))
}

fn install_service_with(exec: &Exec) -> Result<()> {
    let exec = exec.clone();

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

    // What the website and firewall rules need, before the agent (re)starts:
    // its sandbox sees /etc/dnsmasq.d only if it exists at start.
    ensure_enforcement_deps(&exec);

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
    // The on-device window's launcher: an app-grid entry + icon, system-wide so
    // it appears for every user with no per-user setup. GUI build only — a
    // headless agent's `app` subcommand just bails.
    if cfg!(feature = "gui") {
        install_desktop_entry(&exec);
        install_lock(&exec);
    }
    // The companion (warnings, "You're back") starts on every desktop login,
    // with or without a systemd user session.
    if cfg!(feature = "tray") {
        if let Err(e) = exec.write_file(COMPANION_AUTOSTART_PATH, COMPANION_AUTOSTART) {
            tracing::warn!("could not install {COMPANION_AUTOSTART_PATH}: {e}");
        }
    }
    tamper::install_polkit(&exec, 1)?;
    // sudo on this machine asks for the parent code (CONTRACT-0.4 §8). A
    // failure here must not abort the install of enforcement itself.
    if let Err(e) = install_parent_sudo(&exec) {
        tracing::warn!("parent-code sudo not installed: {e}");
    }

    exec.run("systemctl", &["daemon-reload"])?;
    exec.run("systemctl", &["enable", AGENT_UNIT])?;
    // `restart`, not `enable --now`: re-running the one-liner (a new enroll
    // token, a computer moved to another person) has to reach the agent that
    // is already running — `--now` leaves a running unit alone, still holding
    // the old, removed device's token. restart also starts a stopped one.
    exec.run("systemctl", &["restart", AGENT_UNIT])?;
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
        // …and now, for whoever is already signed in.
        start_companions(&exec);
    }

    tracing::info!("hardened unit + watchdog + polkit installed and enabled");
    println!("Installed openscreentime-agent.service (hardened) + watchdog timer.");
    println!("Try `ost time` to see today's screen time.");
    Ok(())
}

/// What `install-service` put on this computer besides the binary: the
/// units (the companion stopped for whoever is signed in), the lock's unit,
/// user and PAM session, the launcher and autostart, the unlock-code sudo.
/// The agent's own units are stopped by the caller first.
pub fn remove_installed(exec: &Exec) {
    for user in graphical_users(exec) {
        let _ = exec.run(
            "systemctl",
            &["--user", "-M", &format!("{user}@"), "stop", TRAY_UNIT_NAME],
        );
    }
    let _ = exec.run("systemctl", &["--global", "disable", TRAY_UNIT_NAME]);
    let _ = exec.run(
        "systemctl",
        &["stop", &crate::lock::unit_name(crate::lock::LOCK_VT)],
    );
    for p in [
        UNIT_PATH,
        WATCHDOG_SVC_PATH,
        WATCHDOG_TIMER_PATH,
        TRAY_UNIT_PATH,
        crate::lock::UNIT_TEMPLATE_PATH,
        crate::lock::PAM_PATH,
    ] {
        let _ = exec.remove_file(p);
    }
    remove_desktop_entry(exec);
    if users::get_user_by_name(crate::lock::LOCK_USER).is_some() {
        let _ = exec.run("userdel", &[crate::lock::LOCK_USER]);
    }
    remove_parent_sudo(exec);
    let _ = exec.run("systemctl", &["daemon-reload"]);
}

/// `ost uninstall`: stop and remove the units, the sudo/PAM hook and the group.
/// The enrollment config and state are left alone (re-running `install-service`
/// picks them right back up); the binary is left in place too.
pub fn uninstall(ctx: Arc<AgentCtx>) -> Result<()> {
    ctx.require_root_for_enforcement()?;
    let exec = Exec::new(ctx);
    let _ = exec.run("systemctl", &["disable", "--now", WATCHDOG_TIMER_UNIT]);
    let _ = exec.run("systemctl", &["disable", "--now", AGENT_UNIT]);
    remove_installed(&exec);
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

    fn pos(log: &[String], entry: &str) -> usize {
        log.iter()
            .position(|l| l == entry)
            .unwrap_or_else(|| panic!("{entry:?} not in {log:#?}"))
    }

    /// Re-running the one-liner on a computer whose agent is running must
    /// restart it (the new token), install what enforcement needs first, and
    /// start the companion for whoever is signed in right now.
    #[test]
    fn install_restarts_the_agent_after_bringing_its_tools() {
        let exec = Exec::simulated(
            &["dnsmasq", "nft", "dnf", "pacman", "zypper"],
            &[
                (
                    "loginctl list-sessions --no-legend",
                    "c1 120 Debian-gdm seat0 tty1\n2 1000 mia seat0 tty2\n3 1001 philip - pts/0\n",
                ),
                ("loginctl show-session c1 -p Type --value", "wayland\n"),
                ("loginctl show-session 2 -p Type --value", "wayland\n"),
                ("loginctl show-session 3 -p Type --value", "tty\n"),
            ],
        );
        install_service_with(&exec).unwrap();
        let log = exec.log();
        let restart = pos(&log, "run systemctl restart openscreentime-agent.service");
        assert!(!log
            .iter()
            .any(|l| l.contains("--now openscreentime-agent.service")));
        let apt = pos(
            &log,
            "run apt-get -o DPkg::Lock::Timeout=180 install -y --no-install-recommends dnsmasq nftables",
        );
        // dnsmasq's first start finds a config it can start with.
        assert!(pos(&log, "write /etc/dnsmasq.d/00-openscreentime.conf") < apt);
        assert!(apt < restart);
        // What it brought is remembered (a retired computer switches it off).
        pos(&log, "write /var/lib/openscreentime/installed-packages");
        if cfg!(feature = "tray") {
            // Only mia has a desktop open (the greeter and an ssh login don't).
            let started = pos(
                &log,
                "run systemctl --user -M mia@ start openscreentime-tray.service",
            );
            assert!(started > restart);
            assert!(!log
                .iter()
                .any(|l| l.contains("Debian-gdm@") || l.contains("philip@")));
        }
    }

    #[test]
    fn nothing_is_installed_when_the_tools_are_there() {
        let exec = Exec::simulated(&[], &[]);
        assert!(missing_enforcement_deps(&exec).is_empty());
        assert!(!ensure_enforcement_deps(&exec));
        assert!(exec.log().is_empty());
        let bare = Exec::simulated(&["nft", "apt-get", "dnf", "pacman", "zypper"], &[]);
        assert_eq!(missing_enforcement_deps(&bare), vec!["nftables"]);
        assert!(
            !ensure_enforcement_deps(&bare),
            "no package manager: nothing installed"
        );
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
