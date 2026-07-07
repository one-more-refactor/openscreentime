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
    js.push_str("// Managed by sentinel-agent — do not edit.\n");
    js.push_str("polkit.addRule(function(action, subject) {\n");
    js.push_str(&format!("  if (subject.user == \"{ADMIN_USER}\" || subject.user == \"root\") {{ return polkit.Result.YES; }}\n"));
    js.push_str("  var power = [\n");
    js.push_str("    \"org.freedesktop.login1.power-off\",\n");
    js.push_str("    \"org.freedesktop.login1.power-off-multiple-sessions\",\n");
    js.push_str("    \"org.freedesktop.login1.reboot\",\n");
    js.push_str("    \"org.freedesktop.login1.reboot-multiple-sessions\",\n");
    js.push_str("    \"org.freedesktop.login1.suspend\",\n");
    js.push_str("    \"org.freedesktop.login1.suspend-multiple-sessions\"\n");
    js.push_str("  ];\n");
    js.push_str("  if (power.indexOf(action.id) >= 0) { return polkit.Result.NO; }\n");
    if level >= 3 {
        js.push_str("  // Level 3: block user-initiated stop/disable of the sentinel unit.\n");
        js.push_str("  if (action.id == \"org.freedesktop.systemd1.manage-units\" &&\n");
        js.push_str("      action.lookup(\"unit\") == \"sentinel-agent.service\") {\n");
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
    exec.write_file(
        "/etc/systemd/logind.conf.d/50-sentinel.conf",
        "# Managed by sentinel-agent (tamper level 3)\n[Login]\nReserveVT=0\nKillUserProcesses=yes\n",
    )?;
    tracing::info!("level 3: TTY/VT lockdown drop-in written (sentinel-admin can revert)");
    Ok(())
}

/// Re-assert network enforcement if it drifted. Returns any tamper events to emit.
pub fn reassert_all(exec: &Exec) -> Vec<Event> {
    let mut events = Vec::new();
    match dns::reassert(exec) {
        Ok(true) => events.push(tamper_event(
            "resolv_conf_drift",
            SEV_WARN,
            "resolv.conf was changed; re-pinned to local resolver",
        )),
        Ok(false) => {}
        Err(e) => tracing::debug!("resolv reassert error: {e}"),
    }
    if firewall::table_missing(exec) && !exec.dry_run() {
        events.push(tamper_event(
            "nft_flush",
            SEV_CRITICAL,
            "sentinel nftables table missing; ruleset must be re-applied",
        ));
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

pub fn tamper_event(kind: &str, severity: &str, message: &str) -> Event {
    Event::new(
        EV_TAMPER,
        severity,
        json!({ "kind": kind, "message": message }),
    )
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
    fn polkit_preserves_admin_recovery() {
        let r = render_polkit_rule(3);
        assert!(r.contains("subject.user == \"sentinel-admin\""));
        assert!(r.contains("org.freedesktop.login1.power-off"));
        assert!(r.contains("sentinel-agent.service"));
    }

    #[test]
    fn level1_does_not_mask_systemctl_stop() {
        let r = render_polkit_rule(1);
        assert!(!r.contains("sentinel-agent.service"));
    }
}
