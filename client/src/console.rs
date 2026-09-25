//! Kernel messages off the screen while the text lock has it.
//!
//! The kernel prints its log to whichever VT is on screen, up to the
//! console log level — on a VM or a box booted without `quiet`, everything
//! up to info. Over the text lock that is noise at best (a "gnome-shell
//! segfault" line across "Time's up for today") and at worst looks broken.
//! While the text lock holds the screen, only emergencies reach the console;
//! the level before is kept and put back when it lets go. The messages still
//! go to the kernel log and the journal — nothing is lost, only not drawn.
//!
//! The level is kept in the state directory, so an agent that dies with the
//! lock up puts it back the next time the lock lets go (the runtime directory
//! would be gone with the unit by then).

use crate::util::Exec;

const PRINTK: &str = "/proc/sys/kernel/printk";
/// Only `KERN_EMERG` (0) is below it.
const QUIET_LEVEL: &str = "1";

fn saved_path() -> String {
    crate::paths::state("printk-console-level")
        .to_string_lossy()
        .into_owned()
}

/// The console log level: the first of printk's four numbers.
fn level(printk: &str) -> Option<u8> {
    printk.split_whitespace().next()?.parse().ok()
}

/// `on`: quiet the console (keeping the level it had); off: put that level
/// back. Idempotent both ways; best-effort — a console that can't be quieted
/// only means kernel lines may show on the lock.
pub fn quiet(exec: &Exec, on: bool) {
    let saved = exec.read_file(&saved_path()).and_then(|s| level(&s));
    if on {
        if saved.is_some() {
            return; // already quiet, the level from before kept
        }
        let Some(now) = exec.read_file(PRINTK).and_then(|s| level(&s)) else {
            return;
        };
        if now <= 1 {
            return; // quiet already, by someone else
        }
        if exec.write_file(&saved_path(), &format!("{now}\n")).is_err() {
            return; // never quiet it without a way back
        }
        if let Err(e) = exec.write_file(PRINTK, QUIET_LEVEL) {
            tracing::debug!("could not quiet the console: {e:#}");
            let _ = exec.remove_file(&saved_path());
        }
    } else if let Some(before) = saved {
        if let Err(e) = exec.write_file(PRINTK, &before.to_string()) {
            tracing::warn!("could not put the console log level back: {e:#}");
            return;
        }
        let _ = exec.remove_file(&saved_path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_console_is_quiet_while_locked_and_as_before_after() {
        const VM: &str = "7\t4\t1\t7\n";
        let exec = Exec::simulated(&[], &[("read /proc/sys/kernel/printk", VM)]);
        quiet(&exec, true);
        assert_eq!(
            exec.log(),
            [
                "write /var/lib/openscreentime/printk-console-level",
                "write /proc/sys/kernel/printk",
            ]
        );
        // Up again while quiet (a restart adopting the lock): nothing new —
        // the level from before stays the one kept.
        let quiet_now = Exec::simulated(
            &[],
            &[
                ("read /proc/sys/kernel/printk", "1\t4\t1\t7\n"),
                ("read /var/lib/openscreentime/printk-console-level", "7\n"),
            ],
        );
        quiet(&quiet_now, true);
        assert!(quiet_now.log().is_empty());
        // Down: the level comes back, and the note of it goes.
        quiet(&quiet_now, false);
        assert_eq!(
            quiet_now.log(),
            [
                "write /proc/sys/kernel/printk",
                "remove /var/lib/openscreentime/printk-console-level",
            ]
        );
        // Nothing kept, nothing to put back; a console quiet already
        // (booted `quiet`) is left alone.
        let booted_quiet = Exec::simulated(&[], &[("read /proc/sys/kernel/printk", "1 4 1 7")]);
        quiet(&booted_quiet, false);
        quiet(&booted_quiet, true);
        assert!(booted_quiet.log().is_empty());
        // Never quieted without a way back.
        let no_state = Exec::simulated(&[], &[("read /proc/sys/kernel/printk", VM)])
            .failing(&["write:/var/lib/openscreentime/printk-console-level"]);
        quiet(&no_state, true);
        assert!(no_state.log().is_empty());
    }
}
