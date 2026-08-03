//! Firewall enforcement: an `nftables` ruleset, default-deny in both directions,
//! allowing only policy ports + established/related + loopback + the server + the
//! DNS upstream (TAMPER.md). Re-applied on any drift by the tamper loop.

use super::vpn::VpnPlan;
use crate::policy::{FirewallPolicy, NetworkLockdown};
use crate::util::Exec;
use anyhow::Result;

const NFT_TABLE: &str = "sentinel";

/// Well-known public DoH resolver IPs (Cloudflare/Google/Quad9/OpenDNS/AdGuard/
/// NextDNS). `block_doh` drops tcp/udp 443 to these, except the configured
/// `dns_upstream` (never block our own upstream).
const DOH_RESOLVERS: &[&str] = &[
    "1.1.1.1",
    "1.0.0.1",
    "1.1.1.2",
    "1.1.1.3",
    "8.8.8.8",
    "8.8.4.4",
    "9.9.9.9",
    "149.112.112.112",
    "208.67.222.222",
    "208.67.220.220",
    "94.140.14.14",
    "94.140.15.15",
    "45.90.28.0",
    "45.90.30.0",
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

    // ---- the device's OWN managed VPN (admin-uploaded profile) — these accepts
    // must come BEFORE the lockdown drops: `block_vpn` drops udp 51820/1194,
    // which would kill the parent's own tunnel handshake. ----
    if let Some(iface) = vpn.iface {
        s.push_str("    # sentinel VPN profile: tunnel traffic + endpoint handshake\n");
        s.push_str(&format!("    oifname \"{iface}\" accept\n"));
    }
    for ep in &vpn.endpoints {
        if ep.host.parse::<std::net::IpAddr>().is_ok() {
            // Literal IP: pin the accept to the endpoint address.
            s.push_str(&format!(
                "    ip daddr {} {} dport {} accept\n",
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
        s.push_str(&format!(
            "    ip daddr != {dns_upstream} udp dport 53 drop\n"
        ));
        s.push_str(&format!(
            "    ip daddr != {dns_upstream} tcp dport 53 drop\n"
        ));
    }
    if lockdown.block_doh {
        s.push_str(
            "    # block_doh: known public DNS-over-HTTPS resolvers (except our upstream)\n",
        );
        for ip in DOH_RESOLVERS {
            if *ip == dns_upstream {
                continue;
            }
            s.push_str(&format!("    ip daddr {ip} tcp dport 443 drop\n"));
            s.push_str(&format!("    ip daddr {ip} udp dport 443 drop\n"));
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
    s.push_str(&format!("    ip daddr {dns_upstream} accept\n"));
    if let Some(srv) = server {
        // If the server is a literal IP, pin it; hostnames are covered by the
        // allowed 80/443 outbound ports below (and DNS resolves via upstream).
        if srv.parse::<std::net::IpAddr>().is_ok() {
            s.push_str(&format!("    ip daddr {srv} accept\n"));
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

/// True if our table is missing (drifted / was flushed) → re-apply needed.
pub fn table_missing(exec: &Exec) -> bool {
    let out = exec.probe("nft", &["list", "table", "inet", NFT_TABLE]);
    out.trim().is_empty()
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
    fn block_doh_drops_known_resolvers_but_not_upstream() {
        let lockdown = NetworkLockdown {
            block_doh: true,
            ..Default::default()
        };
        let r = render_ruleset(&fw_basic(), &lockdown, "1.1.1.2", None, &VpnPlan::default());
        assert!(r.contains("ip daddr 8.8.8.8 tcp dport 443 drop"));
        assert!(r.contains("ip daddr 9.9.9.9 udp dport 443 drop"));
        // Never block our own configured upstream, even though it's in the list.
        assert!(!r.contains("ip daddr 1.1.1.2 tcp dport 443 drop"));
        assert!(!r.contains("ip daddr 1.1.1.2 udp dport 443 drop"));
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
