//! `openscreentime pam-auth` — RETIRED. Refuses every token.
//!
//! Versions up to 0.6 wired this into `/etc/pam.d/openscreentime-parent` and a
//! sudoers drop-in so that a managed (child) OS user could `sudo` to full root
//! by typing the unlock code, a recovery code or the backup code. That turned
//! a code meant for "30 more minutes" into a root password on the child's own
//! account, so the grant was removed (see `service::remove_parent_sudo`).
//!
//! The subcommand itself is kept, and made to always refuse, because the
//! binary is swapped by the self-updater *before* the agent has had a chance
//! to delete the old PAM service and sudoers drop-in. During that window sudo
//! would still call us: refusing here closes the hole the moment the new
//! binary is in place, independent of whether the cleanup has run yet.
//!
//! Exit 1 = refuse. Nothing is printed (pam_exec's `quiet` hides our stdout;
//! the user sees sudo's own "Sorry, try again.").

use anyhow::Result;
use std::io::Read;

pub fn run() -> Result<()> {
    // Drain stdin so pam_exec never blocks writing the token to a closed pipe.
    // The token itself is never looked at, and never verified.
    let mut raw = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut raw);
    drop(raw);
    std::process::exit(1);
}
