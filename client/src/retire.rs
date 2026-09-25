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
//!    files `install-service` put down are removed, and the enrollment is
//!    forgotten. Re-running the install one-liner enrolls it afresh.
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
    // Forget the enrollment: the token is dead anyway.
    let _ = exec.remove_file(&crate::config::config_path().to_string_lossy());
    let _ = exec.remove_file(crate::config::LEGACY_CONFIG_PATH);
    println!("This computer was removed from its household; OpenScreenTime took itself off it.");
    Ok(())
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
            ],
        );
        run_helper_with(&exec).unwrap();
        let log = exec.log();
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
    }
}
