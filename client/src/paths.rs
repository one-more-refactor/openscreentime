//! Every on-disk location the agent owns, in one place, with the rename
//! migration handled once instead of at twenty call sites.
//!
//! The product was called Sentinel; its state lived under `/var/lib/sentinel`,
//! its runtime files under `/run/sentinel`, its config in `/etc/sentinel`.
//!
//! The two directories are not equally important:
//!
//! * `/run/…` is a tmpfs, rebuilt every boot. Nothing there needs migrating —
//!   it just needs one consistent name.
//! * `/var/lib/…` holds the **usage ledger**. Losing it silently resets how
//!   much screen time a child has already spent today, which this project has
//!   been bitten by before (a reboot used to do exactly that). So the state
//!   directory is migrated, once, before anything reads it.

use std::path::{Path, PathBuf};

pub const STATE_DIR: &str = "/var/lib/openscreentime";
pub const LEGACY_STATE_DIR: &str = "/var/lib/sentinel";
pub const RUN_DIR: &str = "/run/openscreentime";

/// A file under the state directory (persists across reboots).
pub fn state(name: &str) -> PathBuf {
    Path::new(STATE_DIR).join(name)
}

/// A file under the runtime directory (tmpfs, gone on reboot).
pub fn run(name: &str) -> PathBuf {
    Path::new(RUN_DIR).join(name)
}

/// String form, for the many call sites that log or format a path.
pub fn run_str(name: &str) -> String {
    run(name).to_string_lossy().into_owned()
}

/// Move state left behind by the old name into the new directory, once.
///
/// Called before any state is read. Best-effort and never fatal: an agent that
/// refuses to start because a rename failed is worse than one running with a
/// fresh ledger, and both are worse than the rename simply working.
///
/// Uses rename() where possible (atomic, same filesystem) and falls back to a
/// file-by-file copy. Existing files in the destination are never overwritten:
/// if both exist, the new location is authoritative.
pub fn migrate_state_dir() {
    let new = Path::new(STATE_DIR);
    let old = Path::new(LEGACY_STATE_DIR);

    if !old.exists() || old == new {
        return;
    }

    // Nothing at the new path yet: a plain rename is atomic and cheapest.
    if !new.exists() {
        match std::fs::rename(old, new) {
            Ok(()) => {
                tracing::info!("migrated agent state {LEGACY_STATE_DIR} → {STATE_DIR}");
                return;
            }
            Err(e) => {
                tracing::warn!("could not rename {LEGACY_STATE_DIR} → {STATE_DIR}: {e}");
            }
        }
    }

    // Both exist (or rename failed): copy anything the new dir is missing.
    if let Err(e) = std::fs::create_dir_all(new) {
        tracing::warn!("could not create {STATE_DIR}: {e}");
        return;
    }
    let entries = match std::fs::read_dir(old) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("could not read {LEGACY_STATE_DIR}: {e}");
            return;
        }
    };
    let mut moved = 0usize;
    for entry in entries.flatten() {
        let dest = new.join(entry.file_name());
        if dest.exists() {
            continue; // the new location wins
        }
        if entry.path().is_file() {
            match std::fs::copy(entry.path(), &dest) {
                Ok(_) => moved += 1,
                Err(e) => tracing::warn!("could not copy {}: {e}", entry.path().display()),
            }
        }
    }
    if moved > 0 {
        tracing::info!("migrated {moved} state file(s) from {LEGACY_STATE_DIR}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_live_under_the_current_name() {
        assert_eq!(
            state("usage_ledger.json").to_string_lossy(),
            format!("{STATE_DIR}/usage_ledger.json")
        );
        assert_eq!(
            run("heartbeat").to_string_lossy(),
            format!("{RUN_DIR}/heartbeat")
        );
        // The old name is still known — that is what makes migration possible.
        assert!(LEGACY_STATE_DIR.contains("sentinel"));
        assert!(!STATE_DIR.contains("sentinel"));
    }
}
