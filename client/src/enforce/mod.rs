//! Zero-trust enforcement primitives (TAMPER.md → "Zero-trust enforcement
//! primitives (Linux)"). Each submodule shells out through `util::Exec`, so every
//! action honors `--dry-run` and refuses to run as non-root outside dry-run.

pub mod dns;
pub mod firewall;
pub mod screentime;

use crate::client::server_host;
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
pub fn apply_network_policy(
    ctx: Arc<AgentCtx>,
    exec: &Exec,
    server_url: &str,
    policy: &Policy,
) -> Result<()> {
    let server = server_host(server_url);
    dns::apply(exec, &policy.dns)?;
    firewall::apply(
        exec,
        &policy.firewall,
        &policy.dns.upstream,
        server.as_deref(),
    )?;
    tracing::info!(
        dry_run = ctx.dry_run,
        "network policy applied (dns.mode={}, fw.mode={})",
        policy.dns.mode,
        policy.firewall.mode
    );
    Ok(())
}
