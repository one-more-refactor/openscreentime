//! `openscreentime unlock` — the full agent-unlock recovery path ("admin is
//! physically here"). Verifies the parent PIN against the cached policy's
//! `parent_pin_hash` (argon2, fully offline) and, on success, suspends network
//! enforcement for a configurable window: tears down our nft table (and the legacy one),
//! un-pins `/etc/resolv.conf`, and un-freezes every login user. Requires root
//! (same check every other enforcing subcommand uses).
//!
//! Minimal-but-real auto-resume: spawns a detached copy of this binary running
//! the hidden `__resume-enforcement` helper (see `main.rs`), which sleeps for
//! the suspend window and then re-applies the cached policy.

use crate::config::AgentCtx;
use crate::enforce::{self, screentime};
use crate::policy::{self, Policy};
use crate::sysusers;
use crate::util::Exec;
use anyhow::{Context, Result};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

/// `openscreentime unlock --pin <PIN> [--minutes <N>]`.
pub fn run(ctx: &Arc<AgentCtx>, pin: &str, minutes: u64) -> Result<()> {
    ctx.require_root_for_enforcement()?;

    let policy = policy::load_cache().context(
        "cannot verify parent PIN: no cached policy on this device (has the agent ever run?)",
    )?;

    let Some(hash) = policy.parent_pin_hash.as_deref() else {
        anyhow::bail!(
            "no parent PIN is configured on this device's policy — set one in the profile \
             editor first (or use the server's admin lock/unlock instead)"
        );
    };

    if !crate::pin::verify_pin(pin, hash) {
        anyhow::bail!("incorrect parent PIN");
    }

    tracing::warn!(
        "ADMIN RECOVERY: parent PIN verified — suspending enforcement for {minutes} minute(s)"
    );

    let exec = Exec::new(ctx.clone());
    suspend_enforcement(&exec, &policy)?;

    if minutes > 0 {
        spawn_resume(minutes * 60).unwrap_or_else(|e| {
            tracing::warn!(
                "could not schedule auto-resume in {minutes}m ({e}); enforcement stays \
                 suspended until the agent next applies a policy"
            );
        });
    }

    tracing::warn!(
        "ADMIN RECOVERY: enforcement suspended. It will auto-resume in {minutes} minute(s), \
         or immediately once the agent next reaches the server."
    );
    Ok(())
}

/// Tear down the enforcement surface: nft table, resolv.conf pin, frozen users.
fn suspend_enforcement(exec: &Exec, policy: &Policy) -> Result<()> {
    let _ = policy; // reserved: nothing else to key the teardown on today.

    // 1) Remove our nft table (default-deny gone → normal connectivity). The
    // legacy table goes too: an agent upgraded from the Sentinel name can have
    // left one loaded, and half a teardown is worse than none — the user would
    // still be firewalled by rules nothing on the box admits to owning.
    for table in [enforce::firewall::NFT_TABLE, enforce::firewall::LEGACY_NFT_TABLE] {
        if let Err(e) = exec.run("nft", &["delete", "table", "inet", table]) {
            tracing::debug!("nft table {table} delete (probably already absent): {e}");
        }
    }

    // 2) Un-pin resolv.conf so the host can use whatever resolver it likes.
    let _ = exec.run("chattr", &["-i", "/etc/resolv.conf"]);
    let _ = exec.run("systemctl", &["stop", "dnsmasq"]);

    // 3) Un-freeze every login user (cgroup freezer), regardless of which users
    // the agent currently holds policy for in memory (this is a separate
    // process — there is no in-memory state to consult).
    for user in sysusers::login_users() {
        if let Err(e) = screentime::freeze_user(exec, &user.username, false, false) {
            tracing::debug!(
                "unfreeze {} failed (maybe wasn't frozen): {e}",
                user.username
            );
        }
    }

    tracing::info!(
        "enforcement teardown complete: nft table removed, resolv.conf un-pinned, users un-frozen"
    );
    Ok(())
}

/// Spawn a detached child (`openscreentime __resume-enforcement <secs>`) that
/// sleeps out the suspend window, then re-applies the cached policy.
fn spawn_resume(secs: u64) -> Result<()> {
    let exe = std::env::current_exe().context("resolving current executable")?;
    Command::new(exe)
        .args(["__resume-enforcement", &secs.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning resume helper")?;
    Ok(())
}

/// The hidden helper's body (invoked via `main.rs` before clap parsing).
/// Blocking sleep is intentional — this process's only job is to wait, then
/// re-apply enforcement once, then exit.
pub fn resume_after(secs: u64) -> Result<()> {
    std::thread::sleep(Duration::from_secs(secs));
    let policy = policy::load_cache().context("re-applying policy after suspend window")?;
    let ctx = AgentCtx::new(false, false, 1);
    let exec = Exec::new(ctx.clone());
    // This CLI path holds no server state — never tear down (or start) a VPN
    // profile from here; the running agent reconciles it on its next apply.
    enforce::apply_network_policy(ctx, &exec, None, &policy, &enforce::vpn::VpnState::Keep)?;
    tracing::warn!("ADMIN RECOVERY: suspend window elapsed — enforcement re-applied");
    Ok(())
}
