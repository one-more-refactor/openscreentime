//! Firewall enforcement: an `nftables` ruleset, default-deny in both directions,
//! allowing only policy ports + established/related + loopback + the server + the
//! DNS upstream (TAMPER.md). Re-applied on any drift by the tamper loop.

use crate::policy::FirewallPolicy;
use crate::util::Exec;
use anyhow::Result;

const NFT_TABLE: &str = "sentinel";

/// Render the full `nft -f` ruleset for the policy.
pub fn render_ruleset(fw: &FirewallPolicy, dns_upstream: &str, server: Option<&str>) -> String {
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
    dns_upstream: &str,
    server: Option<&str>,
) -> Result<()> {
    let ruleset = render_ruleset(fw, dns_upstream, server);
    // Delete our table if present (ignore error), then load. This keeps other
    // tables (e.g. docker) intact while making our ruleset authoritative.
    let _ = exec.run("nft", &["delete", "table", "inet", NFT_TABLE]);
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
        let r = render_ruleset(&fw, "1.1.1.2", Some("203.0.113.10"));
        assert!(r.contains("hook input priority 0; policy drop"));
        assert!(r.contains("hook output priority 0; policy drop"));
        assert!(r.contains("ip daddr 1.1.1.2 accept"));
        assert!(r.contains("ip daddr 203.0.113.10 accept"));
        assert!(r.contains("tcp dport 443 accept"));
        assert!(r.contains("iif lo accept"));
    }
}
