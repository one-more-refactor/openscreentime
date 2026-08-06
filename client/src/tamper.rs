//! Tamper resistance (TAMPER.md). The honest posture: raise the cost, detect &
//! report every attempt, recover automatically. We never claim unbypassable
//! enforcement, and we ALWAYS preserve a `sentinel-admin` root recovery path.
//!
//! Level 1 (default): hardened unit (see `systemd/`), watchdog heartbeat file,
//! polkit masking of user power controls, NetworkManager disconnect guard,
//! resolv.conf/nft re-assertion, `tamper` events.
//! Level 3 (opt-in): + TTY switch lockdown, mask `systemctl stop` of the unit,
//! bootloader/firmware guidance surfaced as an event.

use crate::config::HEARTBEAT_FILE;
use crate::enforce::{dns, firewall};
use crate::protocol::{Event, EV_TAMPER, SEV_CRITICAL, SEV_WARN};
use crate::util::Exec;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub const POLKIT_RULE_PATH: &str = "/etc/polkit-1/rules.d/49-sentinel.rules";
/// The recovery account that must always retain power/stop rights at every level.
pub const ADMIN_USER: &str = "sentinel-admin";

/// Write/update the watchdog heartbeat file (mtime = liveness). The watchdog unit
/// restarts the agent if this goes stale (TAMPER.md L1).
pub fn touch_heartbeat(exec: &Exec) {
    let ts = chrono::Utc::now().to_rfc3339();
    if let Err(e) = exec.write_file(HEARTBEAT_FILE, &format!("{ts}\n")) {
        tracing::debug!("heartbeat write failed: {e}");
    }
}

/// The polkit rule content. Denies power-off/reboot/suspend to non-root managed
/// users; at level 3 also denies stopping the sentinel unit — but `sentinel-admin`
/// and root are always allowed (recovery path).
pub fn render_polkit_rule(level: u8) -> String {
    let mut js = String::new();
    js.push_str("// Managed by openscreentime — do not edit.\n");
    js.push_str("polkit.addRule(function(action, subject) {\n");
    js.push_str(&format!("  if (subject.user == \"{ADMIN_USER}\" || subject.user == \"root\") {{ return polkit.Result.YES; }}\n"));
    js.push_str("  var power = [\n");
    js.push_str("    \"org.freedesktop.login1.power-off\",\n");
    js.push_str("    \"org.freedesktop.login1.power-off-multiple-sessions\",\n");
    js.push_str("    \"org.freedesktop.login1.reboot\",\n");
    js.push_str("    \"org.freedesktop.login1.reboot-multiple-sessions\",\n");
    js.push_str("    \"org.freedesktop.login1.halt\",\n");
    js.push_str("    \"org.freedesktop.login1.halt-multiple-sessions\",\n");
    js.push_str("    \"org.freedesktop.login1.suspend\",\n");
    js.push_str("    \"org.freedesktop.login1.suspend-multiple-sessions\",\n");
    js.push_str("    \"org.freedesktop.login1.hibernate\",\n");
    js.push_str("    \"org.freedesktop.login1.hibernate-multiple-sessions\",\n");
    js.push_str("    \"org.freedesktop.login1.suspend-then-hibernate\",\n");
    js.push_str("    \"org.freedesktop.login1.suspend-then-hibernate-multiple-sessions\"\n");
    js.push_str("  ];\n");
    js.push_str("  if (power.indexOf(action.id) >= 0) { return polkit.Result.NO; }\n");
    if level >= 3 {
        js.push_str("  // Level 3: block user-initiated stop/disable of the sentinel units.\n");
        js.push_str("  // The watchdog is the recovery net for a killed/stopped agent, so it\n");
        js.push_str(
            "  // must be protected too — masking it alone would silently disarm recovery.\n",
        );
        js.push_str("  var guarded = [\n");
        js.push_str(&format!("    \"{}\",\n", crate::service::AGENT_UNIT));
        js.push_str(&format!("    \"{}\",\n", crate::service::WATCHDOG_UNIT));
        js.push_str("    \"sentinel-watchdog.timer\"\n");
        js.push_str("  ];\n");
        js.push_str("  if (action.id == \"org.freedesktop.systemd1.manage-units\" &&\n");
        js.push_str("      guarded.indexOf(action.lookup(\"unit\")) >= 0) {\n");
        js.push_str("    var verb = action.lookup(\"verb\");\n");
        js.push_str("    if (verb == \"stop\" || verb == \"disable\" || verb == \"mask\") { return polkit.Result.NO; }\n");
        js.push_str("  }\n");
    }
    js.push_str("});\n");
    js
}

/// Install/refresh the polkit rule for the effective level.
pub fn install_polkit(exec: &Exec, level: u8) -> anyhow::Result<()> {
    exec.write_file(POLKIT_RULE_PATH, &render_polkit_rule(level))?;
    tracing::info!(
        "polkit power/stop masking installed (level {level}); {ADMIN_USER} retains recovery"
    );
    Ok(())
}

/// Level 3 extras: disable VT switching for managed sessions. We set the kernel
/// knob that blocks `Ctrl+Alt+F*` (reversible; sentinel-admin can restore).
pub fn apply_level3_tty_lockdown(exec: &Exec) -> anyhow::Result<()> {
    // Disable VT switching via the AllowVTSwitch/`kbd` sysctl-ish knob.
    // (kernel.sysrq + logind ReserveVT are the practical levers; documented in README.)
    let _ = exec.run("loginctl", &["--help"]); // presence check, harmless

    // ReserveVT only. KillUserProcesses=yes used to ride along here, but it
    // has nothing to do with VT switching — it kills every process the user
    // owns at logout (tmux, editors mid-save, unattended homework), turning a
    // screen-time control into unrelated data loss. The freeze/lockout path
    // already handles sessions; logout behavior stays stock.
    exec.write_file(
        "/etc/systemd/logind.conf.d/50-sentinel.conf",
        "# Managed by openscreentime (tamper level 3)\n[Login]\nReserveVT=0\n",
    )?;
    tracing::info!("level 3: TTY/VT lockdown drop-in written (sentinel-admin can revert)");
    Ok(())
}

/// Re-assert network enforcement if it drifted. Returns any tamper events to emit.
pub fn reassert_all(exec: &Exec) -> Vec<Event> {
    let mut events = Vec::new();
    match dns::reassert(exec) {
        Ok((drifted, gaps)) => {
            if drifted {
                events.push(tamper_event(
                    "resolv_conf_drift",
                    SEV_WARN,
                    "resolv.conf was changed; re-pinned to local resolver",
                ));
            }
            // A re-pin that could not be locked down is not a recovery — the
            // next edit sticks just as easily. Say so, every time.
            for gap in gaps {
                events.push(tamper_event(gap.kind(), SEV_CRITICAL, gap.explain()));
            }
        }
        Err(e) => {
            tracing::error!("resolv reassert failed: {e}");
            events.push(tamper_event(
                "resolv_conf_reassert_failed",
                SEV_CRITICAL,
                "could not re-pin resolv.conf; DNS enforcement may be off",
            ));
        }
    }
    if !exec.dry_run() {
        match firewall::table_missing(exec) {
            Some(true) => events.push(tamper_event(
                "nft_flush",
                SEV_CRITICAL,
                "sentinel nftables table missing; ruleset must be re-applied",
            )),
            Some(false) => {}
            // Couldn't check ≠ missing. `nft_flush` is the one kind the tamper
            // monitor escalates to a device lockdown, so it must only ever be
            // fed a verified observation — a spawn failure gets its own,
            // never-escalating kind and is retried next tick.
            None => events.push(tamper_event(
                "nft_probe_failed",
                SEV_WARN,
                "could not run nft to verify the firewall table; will retry",
            )),
        }
    }
    events
}

/// Guard against NetworkManager disconnect of a managed connection. Skeleton:
/// probe current connectivity; a real build subscribes to NM D-Bus
/// `StateChanged` / `DeviceRemoved` signals and re-activates the connection.
pub fn nm_guard_probe(exec: &Exec) -> Option<Event> {
    let state = exec.probe("nmcli", &["-t", "-f", "STATE", "general"]);
    if state.trim() == "disconnected" {
        // Re-assert connectivity best-effort.
        let _ = exec.run("nmcli", &["networking", "on"]);
        return Some(tamper_event(
            "nm_disconnect",
            SEV_WARN,
            "NetworkManager reported disconnected; re-asserted networking",
        ));
    }
    None
}

/// Clock-skew detector: a large jump vs. a monotonic reference is a tamper signal
/// (used to evade screen-time). Skeleton returns the event; the runner tracks the
/// reference timestamp.
pub fn clock_skew_event(
    expected: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<Event> {
    let drift = (now - expected).num_seconds().abs();
    if drift > 3600 {
        return Some(tamper_event(
            "clock_skew",
            SEV_WARN,
            &format!("system clock jumped {drift}s vs expected"),
        ));
    }
    None
}

/// Boot-time clock-rollback detector: `saved` is the wall-clock persisted by
/// the previous run's last tick, `now` is this run's startup. `now` earlier
/// than `saved` means the clock was set back while the agent was off — the one
/// direction the per-tick skew detector cannot see (its reference starts every
/// run as `None`), and the direction that actually pays: rolling back before
/// bedtime, or onto a date whose ledger counters are empty.
///
/// Forward gaps are NOT flagged here — a machine that was simply powered off
/// looks identical to a forward clock-set from where we sit. WARN, not
/// CRITICAL: an RTC-less machine (or a dead CMOS battery) legitimately boots
/// in the past until NTP catches up, so this is a loud signal for the console,
/// not grounds for an automatic lockdown.
pub fn clock_rollback_event(
    saved: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<Event> {
    let rollback = (saved - now).num_seconds();
    if rollback > 300 {
        return Some(tamper_event(
            "clock_rollback",
            SEV_WARN,
            &format!("system clock is {rollback}s behind where it was before the last shutdown"),
        ));
    }
    None
}

pub fn tamper_event(kind: &str, severity: &str, message: &str) -> Event {
    Event::new(
        EV_TAMPER,
        severity,
        json!({ "kind": kind, "message": message }),
    )
}

/// How many consecutive ticks a monitored signal must persist before it counts
/// as a *confirmed* evasion attempt rather than a transient blip. `None` for a
/// signal means "detect, repair, and report, but never auto-lock" — reserved
/// for kinds with common benign causes.
///
/// Only `nft_flush` escalates today: our nftables table is root-owned, we own
/// it exclusively, and we atomically rebuild it every tick, so if it's *still*
/// gone a tick later something with root is actively deleting it faster than we
/// can heal it — a real, sustained attack, not a one-off collateral flush from
/// firewalld/NetworkManager (which our repair absorbs, resetting the counter).
///
/// Deliberately NOT escalated: `clock_skew` (a laptop resuming from a long
/// suspend jumps the wall clock exactly like a clock-set would, and an RTC-less
/// machine's first NTP sync is a legitimate large jump — the clock cheat is
/// instead defused in the usage ledger, see `UsageTracker::roll_day`),
/// `nm_disconnect` (roaming / a dropped packet), and `resolv_conf_drift`
/// (systemd-resolved / DHCP legitimately rewrite it; we just re-pin).
fn confirm_threshold(kind: &str) -> Option<u32> {
    match kind {
        "nft_flush" => Some(2),
        _ => None,
    }
}

/// The set of signal kinds the confirmation monitor tracks. Extend alongside
/// [`confirm_threshold`].
const MONITORED: &[&str] = &["nft_flush"];

/// Separates a real, sustained evasion attempt from a transient technical blip
/// (a dropped packet, a one-off collateral firewall flush). A monitored signal
/// must repeat across `confirm_threshold` consecutive enforcement ticks before
/// it's reported as confirmed; any tick the signal is absent resets its streak.
#[derive(Debug)]
pub struct TamperMonitor {
    strikes: HashMap<String, u32>,
    started: Instant,
}

impl Default for TamperMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl TamperMonitor {
    pub fn new() -> Self {
        TamperMonitor {
            strikes: HashMap::new(),
            started: Instant::now(),
        }
    }

    #[cfg(test)]
    fn with_started(started: Instant) -> Self {
        TamperMonitor {
            strikes: HashMap::new(),
            started,
        }
    }

    /// Feed the tamper-signal kinds observed this tick. Returns the kinds that
    /// *just* crossed their confirmation threshold (report + lock down once).
    /// Boot grace: signals in the first two minutes of agent uptime are ignored
    /// so a device settling after a restart/resume can't self-trigger.
    pub fn observe(&mut self, kinds: &[&str]) -> Vec<String> {
        const BOOT_GRACE: Duration = Duration::from_secs(120);
        let mut confirmed = Vec::new();
        let booting = self.started.elapsed() < BOOT_GRACE;
        let seen: HashSet<&str> = kinds.iter().copied().collect();
        for &kind in MONITORED {
            let Some(threshold) = confirm_threshold(kind) else {
                continue;
            };
            if seen.contains(kind) && !booting {
                let n = self.strikes.entry(kind.to_string()).or_insert(0);
                if *n < threshold {
                    *n += 1;
                    if *n == threshold {
                        confirmed.push(kind.to_string());
                    }
                }
            } else {
                self.strikes.remove(kind);
            }
        }
        confirmed
    }
}

/// Level 3 bootloader/firmware guidance (advisory — we can only recommend).
pub fn level3_boot_guidance_event() -> Event {
    Event::new(
        EV_TAMPER,
        SEV_WARN,
        json!({
            "kind": "boot_guidance",
            "message": "Set a GRUB password, a BIOS/UEFI admin password, and disable USB boot. \
                        These physical mitigations cannot be enforced by software.",
            "advisory": true
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_rollback_fires_only_backwards() {
        let saved = chrono::Utc::now();
        // Booting an hour "before" the last shutdown: the clock was set back.
        let ev = clock_rollback_event(saved, saved - chrono::Duration::hours(1));
        assert_eq!(
            ev.unwrap().payload.get("kind").unwrap().as_str(),
            Some("clock_rollback")
        );
        // A forward gap is just a machine that was powered off — never flagged.
        assert!(clock_rollback_event(saved, saved + chrono::Duration::days(3)).is_none());
        // Small backward steps (NTP correcting a fast clock) stay quiet.
        assert!(clock_rollback_event(saved, saved - chrono::Duration::seconds(120)).is_none());
    }

    #[test]
    fn polkit_preserves_admin_recovery() {
        let r = render_polkit_rule(3);
        assert!(r.contains("subject.user == \"sentinel-admin\""));
        assert!(r.contains("org.freedesktop.login1.power-off"));
        assert!(r.contains(crate::service::AGENT_UNIT));
    }

    #[test]
    fn polkit_denies_every_power_path() {
        // A gap in this list is a GUI-menu bypass (halt/hibernate) with no event.
        let r = render_polkit_rule(1);
        for action in [
            "org.freedesktop.login1.power-off",
            "org.freedesktop.login1.reboot",
            "org.freedesktop.login1.halt",
            "org.freedesktop.login1.suspend",
            "org.freedesktop.login1.hibernate",
            "org.freedesktop.login1.suspend-then-hibernate",
        ] {
            assert!(r.contains(action), "power path not denied: {action}");
        }
    }

    #[test]
    fn polkit_level3_guards_the_watchdog_too() {
        // Masking the watchdog alone would silently disarm the recovery net.
        let r = render_polkit_rule(3);
        assert!(r.contains(crate::service::WATCHDOG_UNIT));
        assert!(r.contains("sentinel-watchdog.timer"));
    }

    #[test]
    fn level1_does_not_mask_systemctl_stop() {
        let r = render_polkit_rule(1);
        assert!(!r.contains(crate::service::AGENT_UNIT));
    }

    #[test]
    fn single_flush_is_not_confirmed() {
        // One missing-table tick could be a collateral flush we heal next tick;
        // it must not lock the device on its own.
        let mut m = TamperMonitor::with_started(Instant::now() - Duration::from_secs(600));
        assert!(m.observe(&["nft_flush"]).is_empty());
    }

    #[test]
    fn sustained_flush_confirms_once() {
        let mut m = TamperMonitor::with_started(Instant::now() - Duration::from_secs(600));
        assert!(m.observe(&["nft_flush"]).is_empty()); // strike 1
        let hit = m.observe(&["nft_flush"]); // strike 2 → confirmed
        assert_eq!(hit, vec!["nft_flush".to_string()]);
        // Already confirmed: doesn't re-fire while it persists.
        assert!(m.observe(&["nft_flush"]).is_empty());
    }

    #[test]
    fn a_clear_tick_resets_the_streak() {
        let mut m = TamperMonitor::with_started(Instant::now() - Duration::from_secs(600));
        assert!(m.observe(&["nft_flush"]).is_empty()); // strike 1
        assert!(m.observe(&[]).is_empty()); // healed → reset
        assert!(m.observe(&["nft_flush"]).is_empty()); // back to strike 1, not confirmed
    }

    #[test]
    fn boot_grace_suppresses_early_signals() {
        // Fresh start: a signal during the settle window is ignored.
        let mut m = TamperMonitor::new();
        assert!(m.observe(&["nft_flush"]).is_empty());
        assert!(m.observe(&["nft_flush"]).is_empty());
    }

    #[test]
    fn clock_skew_never_auto_locks() {
        // A wall-clock jump (suspend/resume, RTC-less NTP sync) must never trip
        // the lockdown path — it's handled in the ledger, not here.
        let mut m = TamperMonitor::with_started(Instant::now() - Duration::from_secs(600));
        for _ in 0..10 {
            assert!(m.observe(&["clock_skew"]).is_empty());
        }
    }
}
