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

use crate::policy::DnsPolicy;
use crate::util::Exec;
use anyhow::Result;

const DNSMASQ_CONF: &str = "/etc/sentinel/dnsmasq.d/sentinel.conf";
const RESOLV_CONF: &str = "/etc/resolv.conf";
const LOCAL_RESOLVER: &str = "127.0.0.1";

/// Build the dnsmasq ruleset that realizes the policy.
pub fn render_dnsmasq(dns: &DnsPolicy) -> String {
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
pub fn apply(exec: &Exec, dns: &DnsPolicy) -> Result<()> {
    let conf = render_dnsmasq(dns);
    exec.write_file(DNSMASQ_CONF, &conf)?;

    // Restart whichever local resolver is present. Best-effort: try dnsmasq, then
    // ask systemd-resolved to reload if that's what's running.
    if let Err(e) = exec.run("systemctl", &["restart", "dnsmasq"]) {
        tracing::debug!("dnsmasq restart failed ({e}); trying systemd-resolved reload");
        let _ = exec.run("resolvectl", &["flush-caches"]);
    }

    // Pin resolv.conf to the local resolver, then set the immutable bit so a
    // managed user can't repoint it. (chattr +i; re-asserted by the tamper loop.)
    pin_resolv_conf(exec)?;
    tracing::info!(
        "DNS applied: {} allowlist entries, upstream {}",
        dns.allowlist.len(),
        dns.upstream
    );
    Ok(())
}

/// Pin & guard /etc/resolv.conf (removing any prior immutable bit first).
pub fn pin_resolv_conf(exec: &Exec) -> Result<()> {
    let _ = exec.run("chattr", &["-i", RESOLV_CONF]); // ignore if not set / unsupported fs
    exec.write_file(RESOLV_CONF, &render_resolv_conf())?;
    if let Err(e) = exec.run("chattr", &["+i", RESOLV_CONF]) {
        tracing::debug!("could not set immutable bit on resolv.conf: {e}");
    }
    Ok(())
}

/// Re-assert resolv.conf if it drifted (called by the tamper loop).
pub fn reassert(exec: &Exec) -> Result<bool> {
    let current = std::fs::read_to_string(RESOLV_CONF).unwrap_or_default();
    if !current.contains(LOCAL_RESOLVER) {
        tracing::warn!("resolv.conf drifted off local resolver — re-pinning");
        pin_resolv_conf(exec)?;
        return Ok(true);
    }
    Ok(false)
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
        let conf = render_dnsmasq(&dns);
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
        let conf = render_dnsmasq(&dns);
        assert!(conf.contains("server=1.1.1.2"));
        assert!(!conf.contains("address=/#/"));
    }
}
