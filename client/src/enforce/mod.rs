//! Zero-trust enforcement primitives (TAMPER.md → "Zero-trust enforcement
//! primitives (Linux)"). Each submodule shells out through `util::Exec`, so every
//! action honors `--dry-run` and refuses to run as non-root outside dry-run.

pub mod dns;
pub mod firewall;
pub mod screentime;

use crate::config::AgentCtx;
use crate::policy::Policy;
use crate::util::Exec;
use anyhow::Result;
use std::sync::Arc;

/// Apply the network-level parts of a policy (DNS + firewall). Screen-time is a
/// continuous accounting loop and lives in `screentime` / the runner.
///
/// The docs model policy as per-user, but DNS/nftables are host-global on Linux.
/// The skeleton applies the *most restrictive* effective network policy across the
/// currently active users; per-user network isolation (nftables cgroup/uid match,
/// split-DNS per session) is noted as future work in the README.
///
/// `server_host` is passed in by the caller (rather than derived here from a
/// server URL) so this module doesn't need to reach into the transport layer
/// (`client::server_host`) to do its job — the caller already knows it.
pub fn apply_network_policy(
    ctx: Arc<AgentCtx>,
    exec: &Exec,
    server_host: Option<&str>,
    policy: &Policy,
) -> Result<()> {
    dns::apply(exec, &policy.dns, &policy.lockdown)?;
    firewall::apply(
        exec,
        &policy.firewall,
        &policy.lockdown,
        &policy.dns.upstream,
        server_host,
    )?;
    tracing::info!(
        dry_run = ctx.dry_run,
        "network policy applied (dns.mode={}, fw.mode={})",
        policy.dns.mode,
        policy.firewall.mode
    );
    Ok(())
}
