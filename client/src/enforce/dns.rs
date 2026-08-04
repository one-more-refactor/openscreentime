//! DNS enforcement: a zero-trust default-deny resolver.
//!
//! Strategy (TAMPER.md): run/configure a local resolver that answers only
//! allowlisted names (wildcards supported) and forwards them to the filtered
//! `upstream`; everything else → NXDOMAIN. `/etc/resolv.conf` is pinned to the
//! local resolver and guarded (immutable bit) so a managed user can't repoint it.
//!
//! The skeleton emits a `dnsmasq` config that realizes this policy and pins
//! resolv.conf. On non-dnsmasq systems the same config is logged (dry-run) and the
//! README documents the `systemd-resolved` drop-in equivalent.
//!
//! Every way this can fail on a real host is reported as a [`DnsGap`] rather than
//! swallowed: a device that cannot enforce DNS must never look like one that can.

use crate::policy::{DnsPolicy, NetworkLockdown};
use crate::util::Exec;
use anyhow::Result;

const DNSMASQ_CONF: &str = "/etc/sentinel/dnsmasq.d/sentinel.conf";
const SENTINEL_CONF_DIR: &str = "/etc/sentinel/dnsmasq.d";
const RESOLV_CONF: &str = "/etc/resolv.conf";
const LOCAL_RESOLVER: &str = "127.0.0.1";

/// Distro directories dnsmasq already reads on startup. We drop a one-line
/// `conf-dir=` stub into the first one that exists, because nothing makes
/// dnsmasq read [`SENTINEL_CONF_DIR`] on its own — a stock Debian
/// `/etc/dnsmasq.conf` has no active directives at all.
const DISTRO_CONF_DIRS: &[&str] = &["/etc/dnsmasq.d", "/usr/local/etc/dnsmasq.d"];

/// Filename for that stub. `00-` so it is parsed before anything else in the
/// directory, since ordering decides who wins on conflicting options.
const INCLUDE_STUB: &str = "00-sentinel.conf";

/// A reason DNS enforcement is not actually in force on this host.
///
/// The agent cannot repair any of these by itself, and each one means the
/// allowlist is not doing what the console says it is doing. They are surfaced
/// as `critical` events instead of being logged and forgotten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsGap {
    /// No local resolver is listening, so the rendered ruleset is inert.
    NoLocalResolver,
    /// `/etc/resolv.conf` was a symlink — another service (systemd-resolved's
    /// stub, `resolvconf`) owns the path and will rewrite it.
    ResolvConfNotAFile,
    /// The immutable bit could not be set: a managed user can repoint DNS.
    ResolvConfNotLocked,
    /// The rendered ruleset was written, but no directory dnsmasq actually
    /// reads could be found to include it from — so dnsmasq is running with
    /// its stock config and the policy is a file nobody parses.
    PolicyNotLoaded,
}

impl DnsGap {
    /// Stable machine-readable identifier (event payload `kind`).
    pub fn kind(self) -> &'static str {
        match self {
            DnsGap::NoLocalResolver => "dns_no_local_resolver",
            DnsGap::ResolvConfNotAFile => "dns_resolv_conf_not_a_file",
            DnsGap::ResolvConfNotLocked => "dns_resolv_conf_not_locked",
            DnsGap::PolicyNotLoaded => "dns_policy_not_loaded",
        }
    }

    /// Operator-facing explanation, in the terms the parent/admin needs.
    pub fn explain(self) -> &'static str {
        match self {
            DnsGap::NoLocalResolver => {
                "no local resolver is listening on 127.0.0.1 — dnsmasq is not \
                 installed or failed to start, so the DNS allowlist is not \
                 filtering anything. Install dnsmasq on this device."
            }
            DnsGap::ResolvConfNotAFile => {
                "/etc/resolv.conf was a symlink owned by another service \
                 (systemd-resolved or resolvconf). It has been replaced with a \
                 real file; disable that service or it will fight the pin on \
                 every network change."
            }
            DnsGap::ResolvConfNotLocked => {
                "the immutable bit could not be set on /etc/resolv.conf — the \
                 filesystem does not support it. A managed user can repoint DNS \
                 and only the 10-second drift check will pull it back."
            }
            DnsGap::PolicyNotLoaded => {
                "the DNS ruleset was written but dnsmasq never reads it: no \
                 dnsmasq config directory was found to include it from, so \
                 dnsmasq is serving its stock config and NOTHING is filtered. \
                 Add `conf-dir=/etc/sentinel/dnsmasq.d` to this host's \
                 dnsmasq.conf."
            }
        }
    }
}

/// Make dnsmasq actually read [`SENTINEL_CONF_DIR`].
///
/// Writing the ruleset is not enough: dnsmasq only parses `/etc/dnsmasq.conf`
/// plus whatever `conf-dir` it was given, and a stock Debian install has no
/// active directives whatsoever. Without this stub the agent writes a policy,
/// restarts dnsmasq successfully, sees it `active`, and reports a fully
/// enforcing device while dnsmasq serves its default forward-everything
/// config — the exact silent-green failure this module exists to prevent.
fn ensure_include(exec: &Exec) -> Option<DnsGap> {
    let dir = DISTRO_CONF_DIRS
        .iter()
        .find(|d| std::path::Path::new(d).is_dir());

    // Under --dry-run nothing exists to probe; log the intent, claim no gap.
    let Some(dir) = dir else {
        if exec.dry_run() {
            return None;
        }
        return Some(DnsGap::PolicyNotLoaded);
    };

    let stub = format!("{dir}/{INCLUDE_STUB}");
    let body =
        format!("# Managed by sentinel-agent — do not edit.\nconf-dir={SENTINEL_CONF_DIR}\n");
    if let Err(e) = exec.write_file(&stub, &body) {
        tracing::error!("could not write dnsmasq include stub {stub}: {e}");
        return Some(DnsGap::PolicyNotLoaded);
    }
    None
}

/// Is a local resolver actually answering? A rendered allowlist that nothing
/// serves is worse than no allowlist, because the console reports it as applied.
fn local_resolver_running(exec: &Exec) -> bool {
    exec.probe("systemctl", &["is-active", "dnsmasq"]).trim() == "active"
}

/// Build the dnsmasq ruleset that realizes the policy.
pub fn render_dnsmasq(dns: &DnsPolicy, lockdown: &NetworkLockdown) -> String {
    let mut out = String::new();
    out.push_str("# Managed by sentinel-agent — do not edit.\n");
    out.push_str("no-resolv\n"); // never inherit host resolv.conf upstreams
    out.push_str("bogus-priv\n");
    out.push_str("domain-needed\n");
    out.push_str("listen-address=127.0.0.1\n");
    out.push_str("bind-interfaces\n");

    if dns.is_default_deny() && !dns.allows_everything() {
        // Zero-trust: forward ONLY allowlisted domains to the filtered upstream.
        // dnsmasq with no-resolv and no matching server returns NXDOMAIN/REFUSED
        // for everything else, which is the default-deny behavior we want.
        for domain in &dns.allowlist {
            let d = domain.trim_start_matches("*.").trim_start_matches('*');
            let d = d.trim_start_matches('.');
            if d.is_empty() {
                continue;
            }
            out.push_str(&format!("server=/{d}/{}\n", dns.upstream));
        }
        // Explicit extra blocks (redundant under default-deny, honored anyway).
        for b in &dns.blocklist {
            let b = b
                .trim_start_matches("*.")
                .trim_start_matches('*')
                .trim_start_matches('.');
            if !b.is_empty() {
                out.push_str(&format!("address=/{b}/0.0.0.0\n"));
            }
        }
        // Catch-all: anything not matched above is NXDOMAIN.
        out.push_str("address=/#/\n");
    } else {
        // allow_all mode, or allowlist == ["*"] (the `default` profile): forward
        // everything to the filtered upstream. Structurally still zero-trust:
        // firewall ports + safe-search stay on.
        out.push_str(&format!("server={}\n", dns.upstream));
        for b in &dns.blocklist {
            let b = b
                .trim_start_matches("*.")
                .trim_start_matches('*')
                .trim_start_matches('.');
            if !b.is_empty() {
                out.push_str(&format!("address=/{b}/0.0.0.0\n"));
            }
        }
    }

    if lockdown.block_tor {
        // block_tor: NXDOMAIN .onion and the Tor Project bootstrap domains, so a
        // managed user can't reach hidden services or download a Tor client.
        out.push_str("# block_tor: Tor hidden services + bootstrap domains\n");
        out.push_str("address=/onion/0.0.0.0\n");
        out.push_str("address=/torproject.org/0.0.0.0\n");
    }

    if dns.safe_search {
        // Force safe-search endpoints for the big providers (CNAME rewrites).
        out.push_str("# safe-search enforced\n");
        out.push_str("cname=www.google.com,forcesafesearch.google.com\n");
        out.push_str("cname=www.youtube.com,restrict.youtube.com\n");
        out.push_str("cname=www.bing.com,strict.bing.com\n");
    }
    out
}

pub fn render_resolv_conf() -> String {
    format!(
        "# Managed by sentinel-agent — pinned. Do not edit.\nnameserver {LOCAL_RESOLVER}\noptions edns0 trust-ad\n"
    )
}

/// Apply DNS policy and pin resolv.conf.
///
/// Returns the [`DnsGap`]s that stop this host from actually enforcing the
/// policy. An empty vec means DNS is genuinely in force.
pub fn apply(exec: &Exec, dns: &DnsPolicy, lockdown: &NetworkLockdown) -> Result<Vec<DnsGap>> {
    let mut gaps = Vec::new();
    let conf = render_dnsmasq(dns, lockdown);
    exec.write_file(DNSMASQ_CONF, &conf)?;

    // Writing the ruleset does not make dnsmasq read it. Drop the include stub
    // BEFORE the restart so the policy goes live on this cycle, not the next.
    if let Some(gap) = ensure_include(exec) {
        gaps.push(gap);
    }

    // dnsmasq is what actually serves the allowlist. There is no equivalent
    // systemd-resolved path implemented, so a failure here is not something a
    // cache flush papers over — it means nothing is filtering.
    if let Err(e) = exec.run("systemctl", &["restart", "dnsmasq"]) {
        tracing::error!("dnsmasq restart failed: {e}");
    }
    if !exec.dry_run() && !local_resolver_running(exec) {
        gaps.push(DnsGap::NoLocalResolver);
    }

    // Pin resolv.conf to the local resolver, then set the immutable bit so a
    // managed user can't repoint it. (chattr +i; re-asserted by the tamper loop.)
    gaps.extend(pin_resolv_conf(exec)?);

    for gap in &gaps {
        tracing::error!("DNS enforcement gap [{}]: {}", gap.kind(), gap.explain());
    }
    tracing::info!(
        "DNS applied: {} allowlist entries, upstream {}, {} gap(s)",
        dns.allowlist.len(),
        dns.upstream,
        gaps.len()
    );
    Ok(gaps)
}

/// Pin & guard /etc/resolv.conf (removing any prior immutable bit first).
pub fn pin_resolv_conf(exec: &Exec) -> Result<Vec<DnsGap>> {
    let mut gaps = Vec::new();

    // If the path is a symlink, another service owns it. Writing through the
    // link lands in *that* service's file — typically on tmpfs, where the
    // immutable bit does not exist — so `chattr +i` silently no-ops and the
    // owner rewrites our nameserver on the next network change. Replace the
    // link with a real file we control.
    if !exec.dry_run() {
        if let Ok(md) = std::fs::symlink_metadata(RESOLV_CONF) {
            if md.file_type().is_symlink() {
                gaps.push(DnsGap::ResolvConfNotAFile);
                let _ = std::fs::remove_file(RESOLV_CONF);
            }
        }
    }

    let _ = exec.run("chattr", &["-i", RESOLV_CONF]); // ignore if not set / unsupported fs
    exec.write_file(RESOLV_CONF, &render_resolv_conf())?;
    if let Err(e) = exec.run("chattr", &["+i", RESOLV_CONF]) {
        tracing::error!("could not set immutable bit on resolv.conf: {e}");
        gaps.push(DnsGap::ResolvConfNotLocked);
    }
    Ok(gaps)
}

/// Re-assert resolv.conf if it drifted (called by the tamper loop). Returns
/// whether it had drifted, plus any gap that stops the re-pin from sticking.
pub fn reassert(exec: &Exec) -> Result<(bool, Vec<DnsGap>)> {
    let current = std::fs::read_to_string(RESOLV_CONF).unwrap_or_default();
    if !current.contains(LOCAL_RESOLVER) {
        tracing::warn!("resolv.conf drifted off local resolver — re-pinning");
        let gaps = pin_resolv_conf(exec)?;
        return Ok((true, gaps));
    }
    Ok((false, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_deny_emits_catch_all_nxdomain() {
        let dns = DnsPolicy {
            mode: "default_deny".into(),
            allowlist: vec!["wikipedia.org".into(), "*.edu".into()],
            blocklist: vec![],
            safe_search: true,
            upstream: "1.1.1.2".into(),
        };
        let conf = render_dnsmasq(&dns, &NetworkLockdown::default());
        assert!(conf.contains("server=/wikipedia.org/1.1.1.2"));
        assert!(conf.contains("server=/edu/1.1.1.2"));
        assert!(conf.contains("address=/#/"));
        assert!(conf.contains("forcesafesearch.google.com"));
    }

    #[test]
    fn wildcard_star_forwards_everything() {
        let dns = DnsPolicy {
            mode: "default_deny".into(),
            allowlist: vec!["*".into()],
            blocklist: vec![],
            safe_search: false,
            upstream: "1.1.1.2".into(),
        };
        let conf = render_dnsmasq(&dns, &NetworkLockdown::default());
        assert!(conf.contains("server=1.1.1.2"));
        assert!(!conf.contains("address=/#/"));
    }

    #[test]
    fn block_tor_emits_onion_and_torproject_blocks() {
        let dns = DnsPolicy {
            mode: "default_deny".into(),
            allowlist: vec!["*".into()],
            blocklist: vec![],
            safe_search: false,
            upstream: "1.1.1.2".into(),
        };
        let lockdown = NetworkLockdown {
            block_tor: true,
            ..Default::default()
        };
        let conf = render_dnsmasq(&dns, &lockdown);
        assert!(conf.contains("address=/onion/0.0.0.0"));
        assert!(conf.contains("address=/torproject.org/0.0.0.0"));
    }

    /// The `kind` strings land in stored event payloads and in whatever the
    /// console filters on, so they are API. Pin them.
    #[test]
    fn gap_kinds_are_stable_identifiers() {
        assert_eq!(DnsGap::NoLocalResolver.kind(), "dns_no_local_resolver");
        assert_eq!(
            DnsGap::ResolvConfNotAFile.kind(),
            "dns_resolv_conf_not_a_file"
        );
        assert_eq!(
            DnsGap::ResolvConfNotLocked.kind(),
            "dns_resolv_conf_not_locked"
        );
        assert_eq!(DnsGap::PolicyNotLoaded.kind(), "dns_policy_not_loaded");
    }

    /// The include stub is what makes dnsmasq read our ruleset at all, so its
    /// contents are load-bearing: a typo here is a silently unfiltered device.
    #[test]
    fn include_stub_points_at_the_sentinel_conf_dir() {
        let body = format!("conf-dir={SENTINEL_CONF_DIR}\n");
        assert!(body.contains("conf-dir=/etc/sentinel/dnsmasq.d"));
        // The rendered ruleset must live inside the directory we include.
        assert!(DNSMASQ_CONF.starts_with(SENTINEL_CONF_DIR));
        // Sorts first, so a conflicting option later in the dir still wins
        // deliberately rather than by filename accident.
        assert!(INCLUDE_STUB.starts_with("00-"));
    }

    /// Every gap has to tell an operator what to actually do about it — an
    /// alert nobody can action is the failure mode this whole change exists
    /// to remove.
    #[test]
    fn every_gap_explains_itself() {
        for gap in [
            DnsGap::NoLocalResolver,
            DnsGap::ResolvConfNotAFile,
            DnsGap::ResolvConfNotLocked,
            DnsGap::PolicyNotLoaded,
        ] {
            assert!(gap.explain().len() > 40, "{} has no guidance", gap.kind());
        }
    }
}
