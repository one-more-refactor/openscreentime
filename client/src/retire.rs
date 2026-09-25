//! A computer removed from its household frees itself.
//!
//! The console promises "no more limits there" when a parent removes a
//! computer. The server keeps the removed device's token as a tombstone and
//! answers it with `410 device_retired` (`client::Retired`) — and on that
//! answer, confirmed once more and only from the configured server, the agent
//! takes itself off the machine:
//!
//! 1. in the running agent, at once: everyone is thawed and the lock comes
//!    down (back to their own session), then the network rules go — our nft
//!    table, the resolv.conf pin (the computer gets its own DNS back), the
//!    dnsmasq include, the polkit rule and the level-3 logind drop-in, the
//!    unlock-code sudo — and the secrets cached with the rules;
//! 2. a marker is written, so an agent that starts again never enforces;
//! 3. `ost __retire` runs in a transient unit, outside the agent's sandbox:
//!    the same teardown again (it can restore a symlinked resolv.conf, which
//!    the sandbox can't), then the units are disabled and stopped, the
//!    companion and any app window are stopped for everyone signed in, the
//!    files `install-service` put down are removed, and so is the rest of
//!    it: the config and the enrollment, the state (the ledger, the unlock
//!    code's state, the marker), the runtime files, the binary. Packages it
//!    brought (dnsmasq, nftables, cage) stay installed, with their own
//!    config back. Re-running the install one-liner sets it up afresh.
//!
//! A plain 401, a network error or an answer from anywhere else never does
//! any of this: the agent keeps enforcing its last rules, as it must.

use crate::config::AgentCtx;
use crate::enforce::{dns, firewall};
use crate::util::Exec;
use anyhow::Result;

/// The level-3 logind drop-in (`tamper::apply_level3_tty_lockdown`).
const LOGIND_DROPIN: &str = "/etc/systemd/logind.conf.d/50-openscreentime.conf";

fn marker_path() -> String {
    crate::paths::state("retired")
        .to_string_lossy()
        .into_owned()
}

/// This computer was retired: an agent that starts here must not enforce.
pub fn marked() -> bool {
    std::path::Path::new(&marker_path()).exists()
}

/// A new enrollment un-retires the computer.
pub fn clear_marker() {
    let _ = std::fs::remove_file(marker_path());
}

/// Record the retirement (see [`marked`]).
pub fn mark(exec: &Exec) {
    let when = chrono::Utc::now().to_rfc3339();
    if let Err(e) = exec.write_file(&marker_path(), &format!("{when}\n")) {
        tracing::warn!("could not record the retirement: {e}");
    }
}

/// Everything the rules put on this computer, off — except the freeze and
/// the lock, which whoever calls this thaws and takes down first.
pub fn teardown_enforcement(exec: &Exec) {
    firewall::teardown(exec);
    // The computer's own DNS back before the resolver's rules go.
    dns::unpin_resolv_conf(exec);
    // A resolver only we brought is switched off first, not left serving
    // port 53 — nor restarted on its stock config, which binds every
    // address and fails next to systemd-resolved. (The package stays; it's
    // inert disabled.) One that was here before goes back to its own config.
    if crate::service::installed_by_us(exec)
        .iter()
        .any(|p| p == "dnsmasq")
    {
        let _ = exec.run("systemctl", &["disable", "--now", "dnsmasq"]);
    }
    dns::remove_config(exec);
    for path in [crate::tamper::POLKIT_RULE_PATH, LOGIND_DROPIN] {
        if let Err(e) = exec.remove_file(path) {
            tracing::warn!("could not remove {path}: {e}");
        }
    }
    crate::service::remove_parent_sudo(exec);
    // The unlock-code secret and the recovery codes ride in these.
    for path in [
        crate::policy::POLICY_CACHE_PATH,
        crate::policy::BUNDLE_CACHE_PATH,
    ] {
        let _ = exec.remove_file(path);
    }
}

/// Start `ost __retire` outside the agent's sandbox.
pub fn spawn_helper(exec: &Exec) {
    match exec.run(
        "systemd-run",
        &[
            "--quiet",
            "--collect",
            "--unit=openscreentime-retire",
            crate::service::BIN_TARGET,
            "__retire",
        ],
    ) {
        Ok(_) => tracing::warn!("retired: removing OpenScreenTime from this computer"),
        Err(e) => tracing::error!("could not start the retirement helper: {e}"),
    }
}

/// `ost __retire` (hidden): the rest of the retirement, as root outside the
/// agent's sandbox. Safe to run twice.
pub fn run_helper() -> Result<()> {
    run_helper_with(&Exec::new(AgentCtx::new(false, false, 1)))
}

fn run_helper_with(exec: &Exec) -> Result<()> {
    if !exec.dry_run() {
        mark(exec);
    }
    // The watchdog first, or it starts the agent right back up.
    let _ = exec.run(
        "systemctl",
        &["disable", "--now", crate::service::WATCHDOG_TIMER_UNIT],
    );
    let _ = exec.run(
        "systemctl",
        &["disable", "--now", crate::service::AGENT_UNIT],
    );
    // Nobody stays frozen, nobody behind a lock — even if the agent died
    // before it got there.
    if !exec.dry_run() {
        for user in crate::sysusers::login_users() {
            let _ = crate::enforce::screentime::freeze_user(exec, &user.username, false, false);
        }
        crate::lock::teardown_recorded(exec, crate::runner::recorded_lock());
    }
    teardown_enforcement(exec);
    crate::service::remove_installed(exec);
    remove_the_rest(exec);
    let _ = exec.run("systemctl", &["daemon-reload"]);
    println!("This computer was removed from its household; OpenScreenTime took itself off it.");
    Ok(())
}

/// Everything else of ours, last — after the units, which could start the
/// agent again, are gone: the enrollment (its token is dead anyway) and the
/// rest of the config, the state (the ledger, the unlock code's counters,
/// the retirement marker, whose job is done once nothing can start), the
/// runtime files, each person's own runtime files (seen-code markers, the
/// warning ring), the binary with its aliases and its update leftovers. This
/// process runs from that binary; unlinking it under itself is fine.
fn remove_the_rest(exec: &Exec) {
    let _ = exec.remove_file(&crate::config::config_path().to_string_lossy());
    let _ = exec.remove_file(crate::config::LEGACY_CONFIG_PATH);
    let config_dir = std::path::Path::new(crate::config::CONFIG_PATH)
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    for dir in [
        config_dir.as_str(),
        crate::paths::STATE_DIR,
        crate::paths::RUN_DIR,
    ] {
        if let Err(e) = exec.remove_dir_all(dir) {
            tracing::warn!("could not remove {dir}: {e:#}");
        }
    }
    let uids: Vec<u32> = if exec.dry_run() {
        Vec::new()
    } else {
        crate::sysusers::login_users()
            .into_iter()
            .filter_map(|u| u.uid)
            .collect()
    };
    for uid in uids {
        let _ = exec.remove_dir_all(&format!("/run/user/{uid}/openscreentime"));
    }
    for path in [
        crate::service::BIN_ALIAS,
        crate::service::LEGACY_BIN,
        crate::update::STAGING_PATH,
        crate::update::BACKUP_PATH,
        crate::service::BIN_TARGET,
    ] {
        let _ = exec.remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(log: &[String], entry: &str) -> usize {
        log.iter()
            .position(|l| l == entry)
            .unwrap_or_else(|| panic!("{entry:?} not in {log:#?}"))
    }

    /// What the retirement takes off the computer, and in which order.
    #[test]
    fn the_helper_takes_everything_off_and_stops_the_units() {
        let pinned = crate::enforce::dns::render_resolv_conf();
        let exec = Exec::simulated(
            &[],
            &[
                ("read /etc/resolv.conf", pinned.as_str()),
                (
                    "read /var/lib/openscreentime/resolv.conf.pre-pin",
                    r#"{"symlink":null,"content":"nameserver 192.168.1.1\n"}"#,
                ),
                // install-service brought dnsmasq: it's switched off again.
                (
                    "read /var/lib/openscreentime/installed-packages",
                    "dnsmasq\nnftables\n",
                ),
                // …and on this Arch box it named our rules in dnsmasq.conf.
                (
                    "read /etc/dnsmasq.conf",
                    "#conf-dir=/etc/dnsmasq.d/,*.conf\n# Added by OpenScreenTime: its \
                     website rules. Removed when it leaves this computer.\n\
                     conf-dir=/etc/openscreentime/dnsmasq.d\n",
                ),
                // mia at her desktop, philip over ssh, the greeter.
                (
                    "loginctl list-sessions --no-legend",
                    "c1 120 Debian-gdm seat0 tty1\n2 1000 mia seat0 tty2\n3 1001 philip - pts/0\n",
                ),
            ],
        );
        run_helper_with(&exec).unwrap();
        let log = exec.log();
        // Off before its config goes, never restarted on the stock one.
        assert!(
            pos(&log, "run systemctl disable --now dnsmasq")
                < pos(&log, "remove /etc/dnsmasq.d/00-openscreentime.conf")
        );
        let timer = pos(
            &log,
            "run systemctl disable --now openscreentime-watchdog.timer",
        );
        let agent = pos(
            &log,
            "run systemctl disable --now openscreentime-agent.service",
        );
        assert!(timer < agent, "the watchdog would restart the agent");
        pos(&log, "run nft delete table inet openscreentime");
        pos(&log, "run chattr -i /etc/resolv.conf");
        // The computer's own DNS is back, not a pin to a stopped resolver.
        pos(&log, "write /etc/resolv.conf");
        pos(&log, "remove /etc/dnsmasq.d/00-openscreentime.conf");
        pos(&log, "remove /etc/polkit-1/rules.d/49-openscreentime.rules");
        pos(&log, "remove /etc/openscreentime/policy_bundle.json");
        pos(
            &log,
            "remove /etc/systemd/system/openscreentime-agent.service",
        );
        pos(&log, "remove /etc/openscreentime/agent.toml");
        assert!(!log.iter().any(|l| l.contains("chattr +i")));
        // dnsmasq stays installed, with its own config back.
        pos(&log, "write /etc/dnsmasq.conf");
        assert!(!log
            .iter()
            .any(|l| l.contains("remove") && l.contains("dnsmasq.conf")));

        // Acceptance round 2: after the removal the binary, the ledger and
        // the rest of the state, the rules' directory and both companions
        // were still there — and a used sign-in code popped up again. The
        // companion stops for everyone signed in, desktop or not, whoever
        // started it; then everything of ours goes, the binary last.
        let units = pos(
            &log,
            "remove /etc/systemd/system/openscreentime-agent.service",
        );
        for who in ["mia", "philip"] {
            let stop = pos(
                &log,
                &format!("run systemctl --user -M {who}@ stop openscreentime-tray.service"),
            );
            assert!(stop < units);
        }
        assert!(!log.iter().any(|l| l.contains("Debian-gdm@")));
        let killed = pos(
            &log,
            "run pkill -TERM -f ^(/usr/local/bin/)?(openscreentime|ost) (tray|app)( |$)",
        );
        let gone = [
            "remove -r /etc/openscreentime",
            "remove -r /var/lib/openscreentime",
            "remove -r /run/openscreentime",
            "remove /usr/local/bin/ost",
            "remove /usr/local/bin/openscreentime.bak",
        ]
        .map(|step| pos(&log, step));
        let binary = pos(&log, "remove /usr/local/bin/openscreentime");
        assert!(gone
            .iter()
            .all(|g| killed < *g && units < *g && *g < binary));
        assert_eq!(
            log.last().map(String::as_str),
            Some("run systemctl daemon-reload")
        );
    }

    /// What pkill is given (a POSIX extended regex over the whole command
    /// line — checked here with grep -E, the same syntax) stops the
    /// companion and the app window however they were started, and nothing
    /// else of ours.
    #[test]
    fn the_companion_pattern_is_the_companion() {
        let log = {
            let exec = Exec::simulated(&[], &[]);
            crate::service::remove_installed(&exec);
            exec.log()
        };
        let pattern = log
            .iter()
            .find_map(|l| l.strip_prefix("run pkill -TERM -f "))
            .expect("pkill is asked")
            .to_string();
        let matches = |line: &str| {
            std::process::Command::new("grep")
                .args(["-qE", &pattern])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut c| {
                    use std::io::Write;
                    c.stdin.take().unwrap().write_all(line.as_bytes())?;
                    c.wait()
                })
                .map(|s| s.success())
                .unwrap_or_else(|e| panic!("grep: {e}"))
        };
        for yes in [
            "/usr/local/bin/openscreentime tray",
            "/usr/local/bin/openscreentime app",
            "ost app",
            "openscreentime tray --verbose",
        ] {
            assert!(matches(yes), "{yes}");
        }
        for no in [
            "/usr/local/bin/openscreentime run",
            "/usr/local/bin/openscreentime __retire",
            "/usr/local/bin/openscreentime __lockscreen",
            "/usr/local/bin/openscreentime trayx",
            "bash -c ost app",
        ] {
            assert!(!matches(no), "{no}");
        }
    }
}
