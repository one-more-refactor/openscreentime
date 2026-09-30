//! Tamper resistance (TAMPER.md). The honest posture: raise the cost, detect &
//! report every attempt, recover automatically. We never claim unbypassable
//! enforcement, and we ALWAYS preserve an `ost-admin` root recovery path.
//!
//! Level 1 (default): hardened unit (see `systemd/`), watchdog heartbeat file,
//! NetworkManager disconnect guard, resolv.conf/nft re-assertion, `tamper`
//! events. Power-off, reboot and suspend are never blocked.
//! Level 3 (opt-in): + TTY switch lockdown, a polkit rule against
//! `systemctl stop` of the units, bootloader/firmware guidance as an event.

use crate::config::HEARTBEAT_FILE;
use crate::enforce::{dns, firewall};
use crate::protocol::{Event, EV_ENFORCEMENT_DEGRADED, EV_TAMPER, SEV_CRITICAL, SEV_WARN};
use crate::util::Exec;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub const POLKIT_RULE_PATH: &str = "/etc/polkit-1/rules.d/49-openscreentime.rules";
/// The recovery account that can always stop the openscreentime units.
pub const ADMIN_USER: &str = "ost-admin";

/// A tamper level the server asked for, against the one this computer runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TamperLevel {
    /// What the server asked for (1..=3).
    pub requested: u8,
    /// This computer's own ceiling (`AgentCtx::tamper_max`: 3 with
    /// `--tamper-max`, 1 without).
    pub ceiling: u8,
    /// What this computer actually runs at.
    pub applied: u8,
}

impl TamperLevel {
    /// The server asked for more than this computer allows.
    pub fn capped(&self) -> bool {
        self.requested > self.applied
    }
}

/// The tamper level to run at for a `requested` one. Level 3 can lock a
/// household out of its own machine (VT lockdown, unstoppable units), so it
/// needs `--tamper-max` on the computer itself: without it a server — or a
/// stolen console session — can ask for 3 and gets the ceiling, reported as
/// capped rather than quietly applied or quietly dropped.
pub fn clamp_tamper_level(requested: u8, ceiling: u8) -> TamperLevel {
    let requested = requested.clamp(1, 3);
    let ceiling = ceiling.clamp(1, 3);
    TamperLevel {
        requested,
        ceiling,
        applied: requested.min(ceiling),
    }
}

/// The honest answer to a capped request, as a tamper event for the console.
pub fn tamper_level_capped_event(t: &TamperLevel) -> Event {
    Event::new(
        EV_TAMPER,
        SEV_WARN,
        json!({
            "kind": "tamper_level_capped",
            "requested": t.requested,
            "applied": t.applied,
            "ceiling": t.ceiling,
            "message": format!(
                "Asked for tamper level {}, running at level {}: level 3 needs \
                 `--tamper-max` on this computer.",
                t.requested, t.applied
            ),
        }),
    )
}

/// Write/update the watchdog heartbeat file (mtime = liveness). The watchdog unit
/// restarts the agent if this goes stale (TAMPER.md L1).
pub fn touch_heartbeat(exec: &Exec) {
    let ts = chrono::Utc::now().to_rfc3339();
    if let Err(e) = exec.write_file(HEARTBEAT_FILE, &format!("{ts}\n")) {
        tracing::debug!("heartbeat write failed: {e}");
    }
}

/// The polkit rule for a tamper level, or `None` when the level needs none.
///
/// Only level 3 has one: nobody but root and `ost-admin` may stop, disable or
/// mask the openscreentime units (the agent and its watchdog). Power-off,
/// reboot and suspend are left alone at every level. They used to be denied
/// to every non-root user — parents and adults on their own computers
/// included — which also kept laptops from sleeping and counted a closed lid
/// as screen time. The day's time and who is stopped are kept on disk (the
/// ledger, `freeze_state`), so a power-cycle or a suspend is no way around a
/// stop, and the denial bought nothing.
///
/// `ost-admin`'s exemption covers exactly the guarded units; the rule grants
/// nothing else to anyone.
pub fn render_polkit_rule(level: u8) -> Option<String> {
    if level < 3 {
        return None;
    }
    let mut js = String::new();
    js.push_str("// Managed by openscreentime (tamper level 3) — do not edit.\n");
    js.push_str("// Only root and the ost-admin recovery account may stop, disable or mask\n");
    js.push_str("// the openscreentime units. The watchdog is the recovery net for a stopped\n");
    js.push_str("// agent, so it is guarded too — masking it alone would disarm recovery.\n");
    js.push_str("polkit.addRule(function(action, subject) {\n");
    js.push_str("  if (action.id != \"org.freedesktop.systemd1.manage-units\") { return; }\n");
    js.push_str("  var guarded = [\n");
    js.push_str(&format!("    \"{}\",\n", crate::service::AGENT_UNIT));
    js.push_str(&format!("    \"{}\",\n", crate::service::WATCHDOG_UNIT));
    js.push_str("    \"openscreentime-watchdog.timer\"\n");
    js.push_str("  ];\n");
    js.push_str("  if (guarded.indexOf(action.lookup(\"unit\")) < 0) { return; }\n");
    js.push_str(&format!(
        "  if (subject.user == \"{ADMIN_USER}\" || subject.user == \"root\") \
         {{ return polkit.Result.YES; }}\n"
    ));
    js.push_str("  var verb = action.lookup(\"verb\");\n");
    js.push_str(
        "  if (verb == \"stop\" || verb == \"disable\" || verb == \"mask\") \
         { return polkit.Result.NO; }\n",
    );
    js.push_str("});\n");
    Some(js)
}

/// Bring the polkit rule in line with the effective level: write it when the
/// level has one (and it differs), remove it when the level has none. Runs at
/// every start — so an existing install, including one updating from a build
/// that denied power-off to everyone, gets the current rule — and on every
/// level change.
pub fn install_polkit(exec: &Exec, level: u8) -> anyhow::Result<()> {
    match render_polkit_rule(level) {
        Some(rule) => {
            let current = std::fs::read_to_string(POLKIT_RULE_PATH).ok();
            if current.as_deref() == Some(rule.as_str()) {
                return Ok(());
            }
            exec.write_file(POLKIT_RULE_PATH, &rule)?;
            tracing::info!(
                "polkit rule installed (level {level}): only root and {ADMIN_USER} can stop \
                 the openscreentime units"
            );
        }
        None => {
            if exec.remove_file(POLKIT_RULE_PATH)? {
                tracing::info!("polkit rule removed (level {level} needs none)");
            }
        }
    }
    Ok(())
}

/// Level 3 extras: disable VT switching for managed sessions. We set the kernel
/// knob that blocks `Ctrl+Alt+F*` (reversible; ost-admin can restore).
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
        "/etc/systemd/logind.conf.d/50-openscreentime.conf",
        "# Managed by openscreentime (tamper level 3)\n[Login]\nReserveVT=0\n",
    )?;
    tracing::info!("level 3: TTY/VT lockdown drop-in written (ost-admin can revert)");
    Ok(())
}

/// Re-assert network enforcement if it drifted. Returns what it saw this
/// tick — every tick, undeduplicated: the caller reports each incident once
/// ([`Incidents`]) and feeds the raw kinds to the [`TamperMonitor`].
///
/// Only what someone did is `tamper`. What this computer can't do — a tool
/// that isn't installed, a resolver that isn't running, a check that could
/// not run — is `enforcement_degraded`: a setup to fix, never an accusation.
///
/// `firewall_loaded`: the last network apply loaded our nft table. When it
/// could not (nftables missing, ruleset refused), a missing table is that
/// known gap, not a flush — and never a reason to lock the computer down.
pub fn reassert_all(exec: &Exec, firewall_loaded: bool) -> Vec<Event> {
    let mut events = Vec::new();
    match dns::reassert(exec) {
        Ok(dns::Reassert::InForce) => {}
        Ok(dns::Reassert::Repinned(gaps)) => {
            events.push(tamper_event(
                "resolv_conf_drift",
                SEV_WARN,
                "resolv.conf was changed; re-pinned to local resolver",
            ));
            // A re-pin that could not be locked down is not a recovery — the
            // next edit sticks just as easily.
            for gap in gaps {
                events.push(degraded_event(gap.kind(), SEV_CRITICAL, gap.explain()));
            }
        }
        // Never pinned to a resolver that isn't running: the computer keeps
        // its own DNS. Taking an earlier pin off is worth saying once.
        Ok(dns::Reassert::NoResolver { unpinned: true }) => events.push(degraded_event(
            KIND_RESOLVER_STOPPED,
            SEV_CRITICAL,
            "the local resolver stopped, so resolv.conf was un-pinned and this \
             computer uses its own DNS: websites are not filtered until dnsmasq \
             runs again",
        )),
        Ok(dns::Reassert::NoResolver { unpinned: false }) => {}
        Err(e) => {
            tracing::error!("resolv reassert failed: {e}");
            events.push(degraded_event(
                "resolv_conf_reassert_failed",
                SEV_CRITICAL,
                "could not re-pin resolv.conf; DNS enforcement may be off",
            ));
        }
    }
    // No nft here is the `firewall_not_installed` gap the apply already
    // reported — a missing package, not a flushed table.
    if exec.observes() && exec.has("nft") && firewall_loaded {
        match firewall::table_missing(exec) {
            Some(true) => events.push(tamper_event(
                "nft_flush",
                SEV_CRITICAL,
                "openscreentime nftables table missing; ruleset must be re-applied",
            )),
            Some(false) => {}
            // Couldn't check ≠ missing. `nft_flush` is the one kind the tamper
            // monitor escalates to a device lockdown, so it must only ever be
            // fed a verified observation — a spawn failure gets its own,
            // never-escalating kind and is retried next tick.
            None => events.push(degraded_event(
                "nft_probe_failed",
                SEV_WARN,
                "could not run nft to verify the firewall table; will retry",
            )),
        }
    }
    events
}

/// The local resolver stopped under our pin, which was taken off.
pub const KIND_RESOLVER_STOPPED: &str = "dns_resolver_stopped";

fn degraded_event(kind: &str, severity: &str, message: &str) -> Event {
    Event::new(
        EV_ENFORCEMENT_DEGRADED,
        severity,
        json!({ "kind": kind, "message": message }),
    )
}

/// How long a signal that went away and came back stays quiet.
const REPEAT_AFTER: Duration = Duration::from_secs(3600);

/// Reports each incident once. A signal seen on consecutive ticks is one
/// incident, reported when it starts; it ends on the first tick without it.
/// One that comes back within [`REPEAT_AFTER`] of its last report stays quiet
/// too, so a flapping check is not a message every other tick. Keyed on
/// (event type, kind). What a tick saw still reaches the [`TamperMonitor`]
/// raw — this only decides what the console and the phone are told.
#[derive(Debug, Default)]
pub struct Incidents {
    open: HashSet<(String, String)>,
    reported: HashMap<(String, String), Instant>,
}

impl Incidents {
    /// Of one tick's observations, the ones worth reporting.
    pub fn report(&mut self, observed: Vec<Event>) -> Vec<Event> {
        self.report_at(observed, Instant::now())
    }

    fn report_at(&mut self, observed: Vec<Event>, now: Instant) -> Vec<Event> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for ev in observed {
            let kind = ev
                .payload
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or_default()
                .to_string();
            let key = (ev.ev_type.clone(), kind);
            if !seen.insert(key.clone()) {
                continue;
            }
            let quiet = self.open.contains(&key)
                || self
                    .reported
                    .get(&key)
                    .is_some_and(|t| now.duration_since(*t) < REPEAT_AFTER);
            if !quiet {
                self.reported.insert(key, now);
                out.push(ev);
            }
        }
        self.open = seen;
        out
    }
}

/// Guard against NetworkManager disconnect of a managed connection. Skeleton:
/// probe current connectivity; a real build subscribes to NM D-Bus
/// `StateChanged` / `DeviceRemoved` signals and re-activates the connection.
/// Is the device actually on a network right now? A default route means "the
/// local network is up, we just can't reach OUR server" — the only case in
/// which counting toward an offline hard-lockdown is legitimate. No route (or
/// no `ip`) means the box is simply offline (holiday, dead router, no wifi) or
/// we can't tell — and we must NOT freeze the family for that, so this returns
/// false, erring toward never punishing an innocent outage.
pub fn local_network_up(exec: &Exec) -> bool {
    match exec.try_probe("ip", &["route", "show", "default"]) {
        Some(out) => out.lines().any(|l| l.contains("default")),
        None => false,
    }
}

pub fn nm_guard_probe(exec: &Exec) -> Option<Event> {
    if !exec.has("nmcli") {
        return None;
    }
    let state = exec.probe("nmcli", &["-t", "-f", "STATE", "general"]);
    // NetworkManager says "disconnected" whenever it has no connected device
    // of its own — also on a computer whose network it doesn't run at all
    // (systemd-networkd, ifupdown, a cloud image). A default route means this
    // computer is online anyway: nothing was disconnected.
    if state.trim() == "disconnected" && !local_network_up(exec) {
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
/// instead defused by the trusted clock, see `crate::clock` — `clock_skew` is
/// now wall vs. trusted time, which a suspend never trips),
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
    fn level3_needs_the_local_flag() {
        // No --tamper-max (ceiling 1): a request for 3 is capped, and says so.
        let t = clamp_tamper_level(3, 1);
        assert_eq!(t.applied, 1);
        assert!(t.capped());
        let ev = tamper_level_capped_event(&t);
        assert_eq!(ev.payload["kind"], "tamper_level_capped");
        assert_eq!(ev.payload["requested"], 3);
        assert_eq!(ev.payload["applied"], 1);
        // Anything the server can send is capped the same way.
        assert_eq!(clamp_tamper_level(u8::MAX, 1).applied, 1);
        assert_eq!(clamp_tamper_level(2, 1).applied, 1);
        // Within the ceiling nothing is capped.
        let t = clamp_tamper_level(1, 1);
        assert_eq!((t.applied, t.capped()), (1, false));
        // With --tamper-max (ceiling 3), 3 is 3 — and never more.
        let t = clamp_tamper_level(3, 3);
        assert_eq!((t.applied, t.capped()), (3, false));
        assert_eq!(clamp_tamper_level(9, 3).applied, 3);
        // The flag raises the ceiling; it doesn't stop the server lowering it.
        assert_eq!(clamp_tamper_level(1, 3).applied, 1);
        // 0 isn't a level: it's the default, 1.
        assert_eq!(clamp_tamper_level(0, 3).applied, 1);
    }

    #[test]
    fn polkit_preserves_admin_recovery() {
        let r = render_polkit_rule(3).unwrap();
        assert!(r.contains("subject.user == \"ost-admin\""));
        assert!(r.contains(crate::service::AGENT_UNIT));
        // The exemption is scoped to the guarded units: the rule's one YES
        // comes after the unit check, never as a blanket grant.
        assert_eq!(r.matches("polkit.Result.YES").count(), 1);
        assert!(r.find("guarded.indexOf").unwrap() < r.find("polkit.Result.YES").unwrap());
    }

    #[test]
    fn polkit_never_denies_power_or_sleep() {
        // Parents and adults shut down their own computers, and laptops sleep.
        // With the ledger and freeze_state on disk, neither gets round a stop.
        for level in 1..=3 {
            let r = render_polkit_rule(level).unwrap_or_default();
            for action in [
                "org.freedesktop.login1",
                "power-off",
                "reboot",
                "halt",
                "suspend",
                "hibernate",
            ] {
                assert!(!r.contains(action), "level {level} still denies {action}");
            }
        }
    }

    #[test]
    fn polkit_level3_guards_the_watchdog_too() {
        // Masking the watchdog alone would silently disarm the recovery net.
        let r = render_polkit_rule(3).unwrap();
        assert!(r.contains(crate::service::WATCHDOG_UNIT));
        assert!(r.contains("openscreentime-watchdog.timer"));
        assert!(r.contains("verb == \"stop\""));
    }

    #[test]
    fn below_level3_there_is_no_rule_file() {
        // No rule means install_polkit removes the file, so an install from a
        // build that denied power-off loses that rule on its next start.
        assert!(render_polkit_rule(1).is_none());
        assert!(render_polkit_rule(2).is_none());
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

    fn kinds(evs: &[Event]) -> Vec<(String, String)> {
        evs.iter()
            .map(|e| {
                (
                    e.ev_type.clone(),
                    e.payload["kind"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    /// A stock Debian desktop: no dnsmasq, no nftables. That is a setup to
    /// report once (the network apply does, as `enforcement_degraded`) —
    /// never a tamper event, and never one every ten seconds.
    #[test]
    fn missing_tools_are_never_tamper() {
        let exec = crate::util::Exec::simulated(
            &["nft", "dnsmasq"],
            &[("systemctl is-active dnsmasq", "inactive\n")],
        );
        for _ in 0..60 {
            let evs = reassert_all(&exec, false);
            assert!(evs.is_empty(), "{:?}", kinds(&evs));
        }
        // …and resolv.conf was never pinned to a resolver that isn't there.
        assert!(
            !exec
                .log()
                .iter()
                .any(|l| l.starts_with("write /etc/resolv.conf")
                    || l == "run chattr +i /etc/resolv.conf"),
            "{:?}",
            exec.log()
        );
    }

    /// nftables installed but the ruleset never loaded (refused): the missing
    /// table is the known gap, not a flush the monitor would lock down on.
    #[test]
    fn a_table_that_never_loaded_is_not_a_flush() {
        let exec = crate::util::Exec::simulated(
            &[],
            &[
                ("systemctl is-active dnsmasq", "active\n"),
                (
                    "read /etc/resolv.conf",
                    &crate::enforce::dns::render_resolv_conf(),
                ),
            ],
        );
        assert!(reassert_all(&exec, false).is_empty());
        // Loaded before and gone now: that *is* a flush.
        let evs = reassert_all(&exec, true);
        assert_eq!(kinds(&evs), vec![("tamper".into(), "nft_flush".into())]);
    }

    /// NetworkManager "disconnected" on a computer that is online through
    /// something else (systemd-networkd) is not a disconnect.
    #[test]
    fn nm_disconnected_with_a_default_route_is_not_tamper() {
        let online = crate::util::Exec::simulated(
            &[],
            &[
                ("nmcli -t -f STATE general", "disconnected\n"),
                ("ip route show default", "default via 10.0.2.2 dev ens3\n"),
            ],
        );
        assert!(nm_guard_probe(&online).is_none());
        let offline =
            crate::util::Exec::simulated(&[], &[("nmcli -t -f STATE general", "disconnected\n")]);
        assert_eq!(
            nm_guard_probe(&offline).unwrap().payload["kind"],
            "nm_disconnect"
        );
        assert!(nm_guard_probe(&crate::util::Exec::simulated(&["nmcli"], &[])).is_none());
    }

    #[test]
    fn an_incident_is_reported_once() {
        let mut inc = Incidents::default();
        let t0 = Instant::now();
        let drift = || tamper_event("resolv_conf_drift", SEV_WARN, "x");
        // Starts: reported. Persists for an hour of ticks: quiet.
        assert_eq!(inc.report_at(vec![drift()], t0).len(), 1);
        for i in 1..360 {
            let now = t0 + Duration::from_secs(10 * i);
            assert!(inc.report_at(vec![drift(), drift()], now).is_empty());
        }
        // Ends, comes back soon after (flapping): still quiet.
        let later = t0 + Duration::from_secs(3610);
        assert!(inc.report_at(vec![], later).is_empty());
        assert!(inc
            .report_at(vec![drift()], later - Duration::from_secs(3000))
            .is_empty());
        // Ends, and comes back after the quiet hour: a new incident.
        assert!(inc.report_at(vec![], later).is_empty());
        let much_later = t0 + Duration::from_secs(7300);
        assert_eq!(inc.report_at(vec![drift()], much_later).len(), 1);
        // Different kinds are different incidents.
        let flush = tamper_event("nft_flush", SEV_CRITICAL, "y");
        assert_eq!(inc.report_at(vec![drift(), flush], much_later).len(), 1);
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
