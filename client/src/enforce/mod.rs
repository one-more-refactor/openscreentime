//! Zero-trust enforcement primitives (TAMPER.md → "Zero-trust enforcement
//! primitives (Linux)"). Each submodule shells out through `util::Exec`, so every
//! action honors `--dry-run` and refuses to run as non-root outside dry-run.

pub mod activity;
pub mod apps;
pub mod dns;
pub mod firewall;
pub mod safesearch;
pub mod screentime;
pub mod vpn;

use crate::config::AgentCtx;
use crate::policy::{NetworkLockdown, Policy};
use crate::util::Exec;
use std::sync::Arc;

/// One reason any part of network enforcement is not actually in force on this
/// host. Unifies [`dns::DnsGap`], [`firewall::FirewallGap`] and
/// [`vpn::VpnGap`] so callers surface every gap the same way (standing in the
/// `state` frame, and one `enforcement_degraded` event per incident).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    Dns(dns::DnsGap),
    Firewall(firewall::FirewallGap),
    Vpn(vpn::VpnGap),
}

impl Gap {
    /// Stable machine-readable identifier (event payload `kind`).
    pub fn kind(self) -> &'static str {
        match self {
            Gap::Dns(g) => g.kind(),
            Gap::Firewall(g) => g.kind(),
            Gap::Vpn(g) => g.kind(),
        }
    }

    /// Operator-facing explanation, in the terms the parent/admin needs.
    pub fn explain(self) -> &'static str {
        match self {
            Gap::Dns(g) => g.explain(),
            Gap::Firewall(g) => g.explain(),
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
/// Four stages — DNS, the firewall, its anti-bypass lockdown rules, the VPN —
/// each applied on its own: a stage that fails is a [`Gap`] of its own and
/// never skips the ones after it. (A resolv.conf that couldn't be written
/// once took the whole apply down with it, firewall included: a computer
/// with nothing in force and one line in the journal.)
///
/// `safe`: the safe-search front ends' addresses as last looked up
/// ([`safesearch`]); an engine without them is passed through, not redirected.
///
/// Returns the [`Gap`]s that prevent this host from actually enforcing the
/// policy — an empty vec means enforcement is genuinely in force. Callers
/// must surface a non-empty result rather than treating it as "applied".
pub fn apply_network_policy(
    ctx: Arc<AgentCtx>,
    exec: &Exec,
    server_host: Option<&str>,
    policy: &Policy,
    vpn_state: &vpn::VpnState,
    safe: &safesearch::SafeSearch,
) -> (Vec<Gap>, Option<vpn::VpnReport>) {
    // 1. DNS. App/category blocks → DNS sinkholes (the catalog is the single
    // source; `policy.blocks` on the effective policy is the union over every
    // user).
    let sinkhole = openscreentime_policy::catalog::expand(&policy.blocks).domains;
    let dns_gaps = dns::apply(
        exec,
        &policy.dns,
        &policy.lockdown,
        server_host,
        &sinkhole,
        safe,
    );
    // Nothing answering on 127.0.0.1, or resolv.conf not pointing there: the
    // firewall's `force_dns` drops would then sever ALL name resolution — for
    // the child AND for the agent's own control channel, so no relaxing
    // policy could ever arrive. Suppress force_dns exactly in that case;
    // every other lockdown flag still applies. (The gap is reported.)
    let mut fw_lockdown = policy.lockdown.clone();
    if dns_gaps.iter().any(|g| g.breaks_forced_dns()) {
        fw_lockdown.force_dns = false;
    }
    let mut gaps: Vec<Gap> = dns_gaps.into_iter().map(Gap::Dns).collect();

    // 2 + 3. The firewall, then its lockdown rules. Firewall before the
    // tunnel (with the tunnel's accepts in place) — bringing a wg/ovpn unit
    // up before its endpoint accept exists would fail its handshake against
    // our own default-deny.
    let plan = vpn::plan(vpn_state);
    gaps.extend(
        apply_firewall(exec, policy, &fw_lockdown, server_host, &plan)
            .into_iter()
            .map(Gap::Firewall),
    );

    // 4. The VPN.
    let (vpn_gaps, vpn_report) = match vpn::reconcile(exec, vpn_state) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("VPN profile not applied: {e:#}");
            // A profile waiting for its test verdict hears it failed.
            let report = match vpn_state {
                vpn::VpnState::Sync(Some(p)) => p.id.clone().map(|profile_id| vpn::VpnReport {
                    profile_id,
                    ok: false,
                    error: Some(format!("{e:#}")),
                }),
                _ => None,
            };
            (vec![vpn::VpnGap::NotApplied], report)
        }
    };
    gaps.extend(vpn_gaps.into_iter().map(Gap::Vpn));
    tracing::info!(
        dry_run = ctx.dry_run,
        "network policy applied (dns.mode={}, fw.mode={}, vpn={}, gaps={})",
        policy.dns.mode,
        policy.firewall.mode,
        plan.iface.unwrap_or("none"),
        gaps.len()
    );
    (gaps, vpn_report)
}

/// The firewall stage and its lockdown stage. `nft` loads a ruleset
/// all-or-nothing, so lockdown rules this kernel refuses would take the whole
/// table with them: the base table is then loaded without them, and only
/// the lockdown is the gap. Never an abort — DNS, the tunnel and screen time
/// don't depend on `nft` (a stock Debian desktop has none).
fn apply_firewall(
    exec: &Exec,
    policy: &Policy,
    lockdown: &NetworkLockdown,
    server_host: Option<&str>,
    plan: &vpn::VpnPlan,
) -> Vec<firewall::FirewallGap> {
    use firewall::FirewallGap;
    if !exec.has("nft") {
        return vec![FirewallGap::NotInstalled];
    }
    let load = |lockdown: &NetworkLockdown| {
        firewall::apply(
            exec,
            &policy.firewall,
            lockdown,
            &policy.dns.upstream,
            server_host,
            plan,
        )
    };
    let Err(e) = load(lockdown) else {
        return Vec::new();
    };
    tracing::error!("firewall not applied: {e:#}");
    if !lockdown.any() {
        return vec![FirewallGap::NotApplied];
    }
    match load(&NetworkLockdown::default()) {
        Ok(()) => {
            tracing::error!("firewall applied without its lockdown rules (those were refused)");
            vec![FirewallGap::LockdownNotApplied]
        }
        Err(e) => {
            tracing::error!("firewall not applied without its lockdown rules either: {e:#}");
            vec![FirewallGap::NotApplied]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `kind` lands in stored event payloads and whatever the console filters
    /// on, so it is API. And every gap must tell an operator what to do — an
    /// alert nobody can act on is the failure this reporting exists to remove.
    ///
    /// Checked across every variant rather than a sample, so a gap added later
    /// cannot ship with an empty or non-actionable explanation.
    #[test]
    fn every_gap_is_identified_and_actionable() {
        use dns::DnsGap as D;
        use firewall::FirewallGap as F;
        use vpn::VpnGap as V;

        let all = [
            Gap::Dns(D::ResolverMissing),
            Gap::Dns(D::NoLocalResolver),
            Gap::Dns(D::ResolvConfNotAFile),
            Gap::Dns(D::ResolvConfNotLocked),
            Gap::Dns(D::PolicyNotLoaded),
            Gap::Dns(D::RulesNotWritten),
            Gap::Dns(D::ResolvConfNotPinned),
            Gap::Dns(D::SafeSearchUnavailable),
            Gap::Firewall(F::NotInstalled),
            Gap::Firewall(F::NotApplied),
            Gap::Firewall(F::LockdownNotApplied),
            Gap::Vpn(V::NotRunning),
            Gap::Vpn(V::UnsupportedKind),
            Gap::Vpn(V::NotApplied),
        ];

        let mut kinds = std::collections::HashSet::new();
        for g in all {
            let kind = g.kind();
            assert!(
                kind.starts_with("dns_")
                    || kind.starts_with("firewall_")
                    || kind.starts_with("vpn_"),
                "{kind}: the prefix is what the console filters on"
            );
            assert!(kinds.insert(kind), "{kind}: duplicate kind, these are ids");
            assert!(
                g.explain().len() > 60,
                "{kind}: an operator cannot act on a one-liner"
            );
        }
    }

    fn blocking() -> Policy {
        let mut p = Policy::default();
        p.blocks.custom_domains = vec!["example.org".into()];
        p.lockdown.force_dns = true;
        p.lockdown.block_doh = true;
        p
    }

    fn apply(exec: &Exec, p: &Policy) -> Vec<&'static str> {
        let (gaps, _) = apply_network_policy(
            AgentCtx::new(true, false, 1),
            exec,
            None,
            p,
            &vpn::VpnState::Keep,
            &safesearch::SafeSearch::default(),
        );
        gaps.into_iter().map(Gap::kind).collect()
    }

    const RUNNING: (&str, &str) = ("systemctl is-active dnsmasq", "active\n");

    /// Acceptance round 2: resolv.conf couldn't be written, the DNS step
    /// returned an error, and the firewall was never loaded. Every stage
    /// applies or reports its own gap; one failing never skips the others.
    #[test]
    fn a_failing_dns_step_never_skips_the_firewall() {
        // resolv.conf can't be written: a DNS gap; the firewall still loads,
        // without forced DNS (it would cut every lookup off).
        let exec = Exec::simulated(&[], &[RUNNING]).failing(&["write:/etc/resolv.conf"]);
        assert_eq!(apply(&exec, &blocking()), ["dns_resolv_conf_not_pinned"]);
        assert!(
            exec.log().iter().any(|l| l == "run nft -f -"),
            "{:?}",
            exec.log()
        );

        // The ruleset itself can't be written: its own gap, the rest applies.
        let exec = Exec::simulated(&[], &[RUNNING])
            .failing(&["write:/etc/openscreentime/dnsmasq.d/openscreentime.conf"]);
        assert_eq!(apply(&exec, &blocking()), ["dns_rules_not_written"]);
        let log = exec.log();
        assert!(log.iter().any(|l| l == "run nft -f -"), "{log:?}");
        assert!(log.iter().any(|l| l == "write /etc/resolv.conf"), "{log:?}");
    }

    /// nft refusing the ruleset is the firewall's gap; DNS was applied before
    /// it and stays applied. Lockdown rules the kernel refuses don't take the
    /// base firewall with them.
    #[test]
    fn a_failing_firewall_never_undoes_dns_and_lockdown_fails_alone() {
        let exec = Exec::simulated(&[], &[RUNNING]).failing(&["nft"]);
        assert_eq!(apply(&exec, &blocking()), ["firewall_not_applied"]);
        let log = exec.log();
        assert!(log.iter().any(|l| l == "write /etc/resolv.conf"), "{log:?}");
        assert!(log.iter().any(|l| l == "run systemctl restart dnsmasq"));

        // No nft at all: its own gap, nothing tried.
        let exec = Exec::simulated(&["nft"], &[RUNNING]);
        assert_eq!(apply(&exec, &blocking()), ["firewall_not_installed"]);
    }

    /// Lockdown rules this kernel's nft refuses (the whole table is one
    /// transaction) don't take the base firewall down with them: it loads
    /// without them, and only the lockdown is the gap — DNS and the rest
    /// applied all the same.
    #[test]
    fn a_refused_lockdown_falls_back_to_the_base_firewall() {
        let exec = Exec::simulated(&[], &[RUNNING]).failing(&["stdin:block_doh"]);
        assert_eq!(apply(&exec, &blocking()), ["firewall_lockdown_not_applied"]);
        let log = exec.log();
        assert_eq!(log.iter().filter(|l| *l == "run nft -f -").count(), 1);
        assert!(log.iter().any(|l| l == "write /etc/resolv.conf"), "{log:?}");

        // A ruleset with no lockdown in it isn't retried: nothing to drop.
        let exec = Exec::simulated(&[], &[]).failing(&["nft"]);
        let open = Policy::default();
        assert!(!open.lockdown.any());
        assert_eq!(
            apply_firewall(&exec, &open, &open.lockdown, None, &vpn::VpnPlan::default()),
            [firewall::FirewallGap::NotApplied]
        );
    }
}
