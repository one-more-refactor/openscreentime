//! Firewall enforcement: an `nftables` ruleset, default-deny in both directions,
//! allowing only policy ports + established/related + loopback + the server + the
//! DNS upstream (TAMPER.md). Re-applied on any drift by the tamper loop.

use super::vpn::VpnPlan;
use crate::policy::{FirewallPolicy, NetworkLockdown};
use crate::util::Exec;
use anyhow::Result;

const NFT_TABLE: &str = "sentinel";

/// Well-known public DoH resolver addresses (Cloudflare/Google/Quad9/OpenDNS/
/// AdGuard/NextDNS). `block_doh` drops tcp/udp 443 to all of them — including
/// the configured `dns_upstream`, see below.
///
/// Entries may be CIDRs; they are interpolated straight into `ip daddr`.
const DOH_RESOLVERS: &[&str] = &[
    "1.1.1.1",
    "1.0.0.1",
    "1.1.1.2",
    "1.0.0.2", // Cloudflare family — the secondary was missing entirely
    "1.1.1.3",
    "1.0.0.3", // ditto
    "8.8.8.8",
    "8.8.4.4",
    "9.9.9.9",
    "149.112.112.112",
    "208.67.222.222",
    "208.67.220.220",
    "94.140.14.14",
    "94.140.15.15",
    // NextDNS hands each profile its own address inside these ranges, so the
    // bare network addresses that used to be listed here (45.90.28.0 /
    // 45.90.30.0) matched no real resolver at all.
    "45.90.28.0/24",
    "45.90.30.0/24",
];

/// The IPv6 twins of [`DOH_RESOLVERS`]. Every rule in this file used to match
/// on `ip daddr` only, which an IPv6 packet never matches — so on any network
/// with v6 connectivity, pointing a browser at a resolver's v6 address walked
/// straight past `block_doh` (and `force_dns`, fixed below the same way).
const DOH_RESOLVERS_V6: &[&str] = &[
    "2606:4700:4700::1111",
    "2606:4700:4700::1001", // Cloudflare
    "2606:4700:4700::1112",
    "2606:4700:4700::1002", // Cloudflare family (malware)
    "2606:4700:4700::1113",
    "2606:4700:4700::1003", // Cloudflare family (malware + adult)
    "2001:4860:4860::8888",
    "2001:4860:4860::8844", // Google
    "2620:fe::fe",
    "2620:fe::9", // Quad9
    "2620:119:35::35",
    "2620:119:53::53", // OpenDNS
    "2a10:50c0::ad1:ff",
    "2a10:50c0::ad2:ff", // AdGuard
    // NextDNS per-profile addresses live inside these allocations.
    "2a07:a8c0::/32",
    "2a07:a8c1::/32",
];

/// Tor OR/directory/SOCKS ports blocked by `block_tor`.
const TOR_PORTS: &str = "9001, 9030, 9050, 9051, 9150";

/// Render the full `nft -f` ruleset for the policy.
pub fn render_ruleset(
    fw: &FirewallPolicy,
    lockdown: &NetworkLockdown,
    dns_upstream: &str,
    server: Option<&str>,
    vpn: &VpnPlan,
) -> String {
    let mut s = String::new();
    s.push_str("# Managed by sentinel-agent — do not edit.\n");
    s.push_str(&format!("table inet {NFT_TABLE} {{\n"));

    // ---- input chain ----
    let in_policy = if fw.is_default_deny() {
        "drop"
    } else {
        "accept"
    };
    s.push_str(&format!(
        "  chain input {{\n    type filter hook input priority 0; policy {in_policy};\n"
    ));
    s.push_str("    iif lo accept\n");
    s.push_str("    ct state established,related accept\n");
    s.push_str("    ct state invalid drop\n");
    s.push_str("    ip protocol icmp accept\n");
    // ICMPv6 is not optional the way ICMPv4 ping is: neighbor discovery and
    // router advertisements ride on it, so dropping it under default-deny
    // doesn't restrict IPv6 — it silently breaks even the allowed ports on v6.
    s.push_str("    meta l4proto ipv6-icmp accept\n");
    for p in &fw.allow_inbound_ports {
        s.push_str(&format!("    tcp dport {p} accept\n"));
        s.push_str(&format!("    udp dport {p} accept\n"));
    }
    s.push_str("  }\n");

    // ---- output chain ----
    let out_policy = if fw.is_default_deny() {
        "drop"
    } else {
        "accept"
    };
    s.push_str(&format!(
        "  chain output {{\n    type filter hook output priority 0; policy {out_policy};\n"
    ));
    s.push_str("    oif lo accept\n");
    s.push_str("    ct state established,related accept\n");
    // Outbound neighbor solicitation is ICMPv6; without this, default-deny
    // output kills v6 address resolution before any allowed port can be used.
    s.push_str("    meta l4proto ipv6-icmp accept\n");

    // ---- the device's OWN managed VPN (admin-uploaded profile) — these accepts
    // must come BEFORE the lockdown drops: `block_vpn` drops udp 51820/1194,
    // which would kill the parent's own tunnel handshake. ----
    if let Some(iface) = vpn.iface {
        s.push_str("    # sentinel VPN profile: tunnel traffic + endpoint handshake\n");
        s.push_str(&format!("    oifname \"{iface}\" accept\n"));
    }
    for ep in &vpn.endpoints {
        if ep.host.parse::<std::net::IpAddr>().is_ok() {
            // Literal IP: pin the accept to the endpoint address (by family —
            // an `ip daddr` with a v6 literal would abort the ruleset load).
            let m = if ep.host.parse::<std::net::Ipv6Addr>().is_ok() {
                "ip6"
            } else {
                "ip"
            };
            s.push_str(&format!(
                "    {m} daddr {} {} dport {} accept\n",
                ep.host, ep.proto, ep.port
            ));
        } else {
            // Hostname endpoints can't be matched in nft; accept the port. The
            // widened hole is parent-configured and bounded to one port/proto.
            s.push_str(&format!("    {} dport {} accept\n", ep.proto, ep.port));
        }
    }

    // ---- network anti-bypass (NetworkLockdown) — DROP rules FIRST, before the
    // generic accepts below, since nft chains are first-match-wins/terminal. ----
    if lockdown.block_dot {
        s.push_str("    # block_dot: DNS-over-TLS\n");
        s.push_str("    tcp dport 853 drop\n");
        s.push_str("    udp dport 853 drop\n");
    }
    if lockdown.force_dns {
        s.push_str("    # force_dns: plaintext DNS only to our own upstream\n");
        // The `ip daddr` match only sees IPv4 packets, so the other family
        // must be dropped wholesale on port 53 — otherwise any v6 resolver is
        // a complete bypass. Loopback (::1 / 127.0.0.1 → dnsmasq) is already
        // accepted above.
        if dns_upstream.parse::<std::net::Ipv6Addr>().is_ok() {
            s.push_str(&format!(
                "    ip6 daddr != {dns_upstream} udp dport 53 drop\n"
            ));
            s.push_str(&format!(
                "    ip6 daddr != {dns_upstream} tcp dport 53 drop\n"
            ));
            s.push_str("    meta nfproto ipv4 udp dport 53 drop\n");
            s.push_str("    meta nfproto ipv4 tcp dport 53 drop\n");
        } else {
            s.push_str(&format!(
                "    ip daddr != {dns_upstream} udp dport 53 drop\n"
            ));
            s.push_str(&format!(
                "    ip daddr != {dns_upstream} tcp dport 53 drop\n"
            ));
            s.push_str("    meta nfproto ipv6 udp dport 53 drop\n");
            s.push_str("    meta nfproto ipv6 tcp dport 53 drop\n");
        }
    }
    if lockdown.block_doh {
        // The upstream is NOT exempt. It used to be — "never block our own
        // upstream" — but the agent only ever needs port 53 there (dnsmasq
        // forwards plaintext DNS), and force_dns already opens 53 to it alone.
        // Exempting 443 instead handed out a working DoH endpoint: the kids
        // preset ships upstream 1.1.1.2, which IS Cloudflare's public
        // family-filter DoH resolver. A browser pointed at
        // https://1.1.1.2/dns-query then resolved everything, so the
        // default-deny allowlist became decorative while the console showed
        // both "default_deny" and "block_doh: true" in green.
        //
        // These drops precede the generic accepts (including the upstream
        // accept), and nft chains are first-match-wins, so they take effect.
        s.push_str("    # block_doh: known public DNS-over-HTTPS resolvers (upstream included)\n");
        for ip in DOH_RESOLVERS {
            s.push_str(&format!("    ip daddr {ip} tcp dport 443 drop\n"));
            s.push_str(&format!("    ip daddr {ip} udp dport 443 drop\n"));
        }
        for ip in DOH_RESOLVERS_V6 {
            s.push_str(&format!("    ip6 daddr {ip} tcp dport 443 drop\n"));
            s.push_str(&format!("    ip6 daddr {ip} udp dport 443 drop\n"));
        }
    }
    if lockdown.block_vpn {
        s.push_str("    # block_vpn: common commercial-VPN ports\n");
        s.push_str("    udp dport 51820 drop\n"); // WireGuard
        s.push_str("    udp dport 1194 drop\n"); // OpenVPN
        s.push_str("    tcp dport 1194 drop\n"); // OpenVPN
        s.push_str("    udp dport 500 drop\n"); // IPsec/IKE
        s.push_str("    udp dport 4500 drop\n"); // IPsec/IKE NAT-T
    }
    if lockdown.block_tor {
        s.push_str("    # block_tor: Tor OR/directory/SOCKS ports\n");
        s.push_str(&format!("    tcp dport {{ {TOR_PORTS} }} drop\n"));
    }

    // Always let the agent reach the DNS upstream and the control server.
    // `ip daddr` with a v6 literal is a syntax error that would abort the
    // whole (all-or-nothing) ruleset load, so pick the match by family.
    let daddr_accept = |s: &mut String, addr: &str| {
        let m = if addr.parse::<std::net::Ipv6Addr>().is_ok() {
            "ip6"
        } else {
            "ip"
        };
        s.push_str(&format!("    {m} daddr {addr} accept\n"));
    };
    daddr_accept(&mut s, dns_upstream);
    if let Some(srv) = server {
        // If the server is a literal IP, pin it; hostnames are covered by the
        // allowed 80/443 outbound ports below (and DNS resolves via upstream).
        if srv.parse::<std::net::IpAddr>().is_ok() {
            daddr_accept(&mut s, srv);
        }
    }
    for p in &fw.allow_outbound_ports {
        s.push_str(&format!("    tcp dport {p} accept\n"));
        s.push_str(&format!("    udp dport {p} accept\n"));
    }
    s.push_str("  }\n");

    // ---- forward chain (default-deny; device isn't a router) ----
    let fwd_policy = if fw.is_default_deny() {
        "drop"
    } else {
        "accept"
    };
    s.push_str(&format!(
        "  chain forward {{\n    type filter hook forward priority 0; policy {fwd_policy};\n  }}\n"
    ));

    s.push_str("}\n");
    s
}

/// Apply the ruleset: flush our table then load fresh (idempotent).
pub fn apply(
    exec: &Exec,
    fw: &FirewallPolicy,
    lockdown: &NetworkLockdown,
    dns_upstream: &str,
    server: Option<&str>,
    vpn: &VpnPlan,
) -> Result<()> {
    let body = render_ruleset(fw, lockdown, dns_upstream, server, vpn);
    // Atomic replace: `add table` (idempotent — creates if absent) then
    // `delete table` then the fresh definition, all in ONE `nft -f` transaction.
    // nft applies the file all-or-nothing, so a malformed rule (e.g. a bad
    // upstream) aborts the whole load and leaves the last-known-good table in
    // place — never a window with no table (which would fail OPEN). Other tables
    // (docker, etc.) are untouched.
    let ruleset = format!("add table inet {NFT_TABLE}\ndelete table inet {NFT_TABLE}\n{body}");
    exec.run_with_stdin("nft", &["-f", "-"], &ruleset)?;
    tracing::info!(
        "firewall applied: default-deny, {} outbound port(s) allowed",
        fw.allow_outbound_ports.len()
    );
    Ok(())
}

/// Whether our table is missing (drifted / was flushed) → re-apply needed.
///
/// Three-valued on purpose: `Some(true)` means nft ran and the table is
/// verifiably gone, `Some(false)` means it's there, `None` means the check
/// itself could not run (nft failed to spawn — transient fork/memory failure).
/// `None` must never be treated as "missing": this answer feeds the tamper
/// monitor, and two ticks of a conflated spawn failure used to confirm as
/// "sustained evasion" and lock the whole device down over a fork() hiccup.
pub fn table_missing(exec: &Exec) -> Option<bool> {
    exec.try_probe("nft", &["list", "table", "inet", NFT_TABLE])
        .map(|out| out.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_deny_drops_both_directions() {
        let fw = FirewallPolicy {
            mode: "default_deny".into(),
            allow_outbound_ports: vec![53, 80, 443],
            allow_inbound_ports: vec![],
        };
        let r = render_ruleset(
            &fw,
            &NetworkLockdown::default(),
            "1.1.1.2",
            Some("203.0.113.10"),
            &VpnPlan::default(),
        );
        assert!(r.contains("hook input priority 0; policy drop"));
        assert!(r.contains("hook output priority 0; policy drop"));
        assert!(r.contains("ip daddr 1.1.1.2 accept"));
        assert!(r.contains("ip daddr 203.0.113.10 accept"));
        assert!(r.contains("tcp dport 443 accept"));
        assert!(r.contains("iif lo accept"));
    }

    fn fw_basic() -> FirewallPolicy {
        FirewallPolicy {
            mode: "default_deny".into(),
            allow_outbound_ports: vec![80, 443],
            allow_inbound_ports: vec![],
        }
    }

    #[test]
    fn block_dot_drops_853() {
        let lockdown = NetworkLockdown {
            block_dot: true,
            ..Default::default()
        };
        let r = render_ruleset(&fw_basic(), &lockdown, "1.1.1.2", None, &VpnPlan::default());
        assert!(r.contains("tcp dport 853 drop"));
        assert!(r.contains("udp dport 853 drop"));
    }

    #[test]
    fn force_dns_drops_non_upstream_53() {
        let lockdown = NetworkLockdown {
            force_dns: true,
            ..Default::default()
        };
        let r = render_ruleset(&fw_basic(), &lockdown, "1.1.1.2", None, &VpnPlan::default());
        assert!(r.contains("ip daddr != 1.1.1.2 udp dport 53 drop"));
        assert!(r.contains("ip daddr != 1.1.1.2 tcp dport 53 drop"));
    }

    #[test]
    fn block_doh_drops_known_resolvers_including_the_upstream() {
        let lockdown = NetworkLockdown {
            block_doh: true,
            ..Default::default()
        };
        let r = render_ruleset(&fw_basic(), &lockdown, "1.1.1.2", None, &VpnPlan::default());
        assert!(r.contains("ip daddr 8.8.8.8 tcp dport 443 drop"));
        assert!(r.contains("ip daddr 9.9.9.9 udp dport 443 drop"));

        // The upstream gets blocked on 443 like everything else. This assertion
        // used to be inverted: 1.1.1.2 is Cloudflare's family DoH resolver, so
        // exempting it from the 443 drop left a fully working DoH endpoint open
        // and the DNS allowlist bypassable. The agent needs 53 there, not 443.
        assert!(r.contains("ip daddr 1.1.1.2 tcp dport 443 drop"));
        assert!(r.contains("ip daddr 1.1.1.2 udp dport 443 drop"));

        // …and 53 to the upstream must still be the one plaintext path allowed.
        let fd = NetworkLockdown {
            block_doh: true,
            force_dns: true,
            ..Default::default()
        };
        let r2 = render_ruleset(&fw_basic(), &fd, "1.1.1.2", None, &VpnPlan::default());
        assert!(r2.contains("ip daddr != 1.1.1.2 udp dport 53 drop"));
        assert!(!r2.contains("ip daddr 1.1.1.2 udp dport 53 drop\n"));
    }

    /// Both halves of each provider pair must be listed — a kid who finds the
    /// secondary has the same bypass as one who finds the primary.
    #[test]
    fn doh_list_covers_secondaries_and_nextdns_ranges() {
        let lockdown = NetworkLockdown {
            block_doh: true,
            ..Default::default()
        };
        let r = render_ruleset(&fw_basic(), &lockdown, "9.9.9.9", None, &VpnPlan::default());
        for ip in [
            "1.1.1.1", "1.0.0.1", "1.1.1.2", "1.0.0.2", "1.1.1.3", "1.0.0.3",
        ] {
            assert!(
                r.contains(&format!("ip daddr {ip} tcp dport 443 drop")),
                "{ip} missing from the DoH block list"
            );
        }
        // NextDNS assigns a per-profile address in these ranges; the bare
        // network addresses previously listed matched no real resolver.
        assert!(r.contains("ip daddr 45.90.28.0/24 tcp dport 443 drop"));
        assert!(r.contains("ip daddr 45.90.30.0/24 udp dport 443 drop"));
    }

    /// `ip daddr` never matches an IPv6 packet, so every lockdown that matters
    /// must exist in an `ip6`/`nfproto ipv6` form too — otherwise any network
    /// with v6 connectivity is a wholesale bypass of DNS enforcement.
    #[test]
    fn lockdowns_cover_ipv6() {
        let lockdown = NetworkLockdown {
            force_dns: true,
            block_doh: true,
            ..Default::default()
        };
        let r = render_ruleset(&fw_basic(), &lockdown, "1.1.1.2", None, &VpnPlan::default());
        // force_dns: a v4 upstream means NO v6 destination is ever legitimate
        // on port 53 (loopback is accepted earlier in the chain).
        assert!(r.contains("meta nfproto ipv6 udp dport 53 drop"));
        assert!(r.contains("meta nfproto ipv6 tcp dport 53 drop"));
        // block_doh: the well-known resolvers' v6 twins are dropped too.
        assert!(r.contains("ip6 daddr 2606:4700:4700::1112 tcp dport 443 drop"));
        assert!(r.contains("ip6 daddr 2001:4860:4860::8888 udp dport 443 drop"));
        assert!(r.contains("ip6 daddr 2620:fe::fe tcp dport 443 drop"));
        assert!(r.contains("ip6 daddr 2a07:a8c0::/32 tcp dport 443 drop"));
    }

    /// Under default-deny, ICMPv6 must stay open in both directions: neighbor
    /// discovery rides on it, so dropping it doesn't restrict IPv6 — it breaks
    /// even the explicitly allowed ports on v6, silently.
    #[test]
    fn default_deny_keeps_icmpv6_alive() {
        let fw = FirewallPolicy {
            mode: "default_deny".into(),
            allow_outbound_ports: vec![443],
            allow_inbound_ports: vec![],
        };
        let r = render_ruleset(
            &fw,
            &NetworkLockdown::default(),
            "1.1.1.2",
            None,
            &VpnPlan::default(),
        );
        assert_eq!(r.matches("meta l4proto ipv6-icmp accept").count(), 2);
    }

    /// A v6 upstream or server literal must render as `ip6 daddr` — `ip daddr`
    /// with a v6 address is a syntax error, and nft loads the file
    /// all-or-nothing, so one bad line would abort the entire ruleset.
    #[test]
    fn v6_literals_use_ip6_daddr() {
        let lockdown = NetworkLockdown {
            force_dns: true,
            ..Default::default()
        };
        let r = render_ruleset(
            &fw_basic(),
            &lockdown,
            "2606:4700:4700::1113",
            Some("2001:db8::7"),
            &VpnPlan::default(),
        );
        assert!(r.contains("ip6 daddr 2606:4700:4700::1113 accept"));
        assert!(r.contains("ip6 daddr 2001:db8::7 accept"));
        assert!(r.contains("ip6 daddr != 2606:4700:4700::1113 udp dport 53 drop"));
        // …and the OTHER family gets dropped wholesale on 53.
        assert!(r.contains("meta nfproto ipv4 udp dport 53 drop"));
        assert!(!r.contains("ip daddr 2606"));
    }

    #[test]
    fn block_vpn_drops_common_ports() {
        let lockdown = NetworkLockdown {
            block_vpn: true,
            ..Default::default()
        };
        let r = render_ruleset(&fw_basic(), &lockdown, "1.1.1.2", None, &VpnPlan::default());
        assert!(r.contains("udp dport 51820 drop"));
        assert!(r.contains("udp dport 1194 drop"));
        assert!(r.contains("tcp dport 1194 drop"));
        assert!(r.contains("udp dport 500 drop"));
        assert!(r.contains("udp dport 4500 drop"));
    }

    #[test]
    fn block_tor_drops_tor_ports() {
        let lockdown = NetworkLockdown {
            block_tor: true,
            ..Default::default()
        };
        let r = render_ruleset(&fw_basic(), &lockdown, "1.1.1.2", None, &VpnPlan::default());
        assert!(r.contains("tcp dport { 9001, 9030, 9050, 9051, 9150 } drop"));
    }

    #[test]
    fn vpn_exemptions_precede_block_vpn_drops() {
        use super::super::vpn::Endpoint;
        let lockdown = NetworkLockdown {
            block_vpn: true,
            ..Default::default()
        };
        let vpn = VpnPlan {
            iface: Some("sentinel"),
            endpoints: vec![Endpoint {
                host: "203.0.113.7".into(),
                port: 51820,
                proto: "udp",
            }],
        };
        let r = render_ruleset(&fw_basic(), &lockdown, "1.1.1.2", None, &vpn);
        let iface_pos = r.find("oifname \"sentinel\" accept").unwrap();
        let ep_pos = r
            .find("ip daddr 203.0.113.7 udp dport 51820 accept")
            .unwrap();
        let drop_pos = r.find("udp dport 51820 drop").unwrap();
        assert!(
            iface_pos < drop_pos && ep_pos < drop_pos,
            "the managed VPN's accepts must precede block_vpn's drops"
        );
    }

    #[test]
    fn vpn_hostname_endpoint_gets_port_accept() {
        use super::super::vpn::Endpoint;
        let vpn = VpnPlan {
            iface: Some("tun*"),
            endpoints: vec![Endpoint {
                host: "vpn.example.org".into(),
                port: 1194,
                proto: "udp",
            }],
        };
        let r = render_ruleset(
            &fw_basic(),
            &NetworkLockdown::default(),
            "1.1.1.2",
            None,
            &vpn,
        );
        // Hostnames can't be nft-matched — the port itself is accepted.
        assert!(r.contains("udp dport 1194 accept"));
        assert!(!r.contains("vpn.example.org"));
        assert!(r.contains("oifname \"tun*\" accept"));
    }

    #[test]
    fn drop_rules_precede_generic_accepts() {
        let lockdown = NetworkLockdown {
            block_dot: true,
            ..Default::default()
        };
        let r = render_ruleset(&fw_basic(), &lockdown, "1.1.1.2", None, &VpnPlan::default());
        let drop_pos = r.find("tcp dport 853 drop").unwrap();
        let accept_pos = r.find("ip daddr 1.1.1.2 accept").unwrap();
        assert!(
            drop_pos < accept_pos,
            "drop rules must precede the generic accepts"
        );
    }
}
