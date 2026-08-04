//! Zero-trust enforcement primitives (TAMPER.md → "Zero-trust enforcement
//! primitives (Linux)"). Each submodule shells out through `util::Exec`, so every
//! action honors `--dry-run` and refuses to run as non-root outside dry-run.

pub mod dns;
pub mod firewall;
pub mod screentime;
pub mod vpn;

use crate::config::AgentCtx;
use crate::policy::Policy;
use crate::util::Exec;
use anyhow::Result;
use std::sync::Arc;

/// One reason any part of network enforcement is not actually in force on this
/// host. Unifies [`dns::DnsGap`] and [`vpn::VpnGap`] so callers surface every
/// gap the same way (as `enforcement_degraded` critical events).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    Dns(dns::DnsGap),
    Vpn(vpn::VpnGap),
}

impl Gap {
    /// Stable machine-readable identifier (event payload `kind`).
    pub fn kind(self) -> &'static str {
        match self {
            Gap::Dns(g) => g.kind(),
            Gap::Vpn(g) => g.kind(),
        }
    }

    /// Operator-facing explanation, in the terms the parent/admin needs.
    pub fn explain(self) -> &'static str {
        match self {
            Gap::Dns(g) => g.explain(),
            Gap::Vpn(g) => g.explain(),
        }
    }
}

/// Apply the network-level parts of a policy (DNS + firewall + the device VPN
/// profile). Screen-time is a continuous accounting loop and lives in
/// `screentime` / the runner.
///
/// The docs model policy as per-user, but DNS/nftables are host-global on Linux.
/// The skeleton applies the *most restrictive* effective network policy across the
/// currently active users; per-user network isolation (nftables cgroup/uid match,
/// split-DNS per session) is noted as future work in the README.
///
/// `server_host` is passed in by the caller (rather than derived here from a
/// server URL) so this module doesn't need to reach into the transport layer
/// (`client::server_host`) to do its job — the caller already knows it.
///
/// `vpn_state` is declarative when the caller holds server state
/// ([`vpn::VpnState::Sync`]) and inert for CLI paths that don't
/// ([`vpn::VpnState::Keep`]) — either way the firewall whitelists whatever
/// tunnel is in force ahead of the lockdown drops.
///
/// Returns the [`Gap`]s that prevent this host from actually enforcing the
/// policy — an empty vec means enforcement is genuinely in force. Callers
/// must surface a non-empty result rather than treating `Ok` as "applied".
pub fn apply_network_policy(
    ctx: Arc<AgentCtx>,
    exec: &Exec,
    server_host: Option<&str>,
    policy: &Policy,
    vpn_state: &vpn::VpnState,
) -> Result<(Vec<Gap>, Option<vpn::VpnReport>)> {
    let mut gaps: Vec<Gap> = dns::apply(exec, &policy.dns, &policy.lockdown)?
        .into_iter()
        .map(Gap::Dns)
        .collect();
    // Firewall first (with the tunnel's accepts in place), THEN the tunnel —
    // bringing a wg/ovpn unit up before its endpoint accept exists would fail
    // its handshake against our own default-deny.
    let plan = vpn::plan(vpn_state);
    firewall::apply(
        exec,
        &policy.firewall,
        &policy.lockdown,
        &policy.dns.upstream,
        server_host,
        &plan,
    )?;
    let (vpn_gaps, vpn_report) = vpn::reconcile(exec, vpn_state)?;
    gaps.extend(vpn_gaps.into_iter().map(Gap::Vpn));
    tracing::info!(
        dry_run = ctx.dry_run,
        "network policy applied (dns.mode={}, fw.mode={}, vpn={}, gaps={})",
        policy.dns.mode,
        policy.firewall.mode,
        plan.iface.unwrap_or("none"),
        gaps.len()
    );
    Ok((gaps, vpn_report))
}
