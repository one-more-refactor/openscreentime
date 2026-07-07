//! LAN discovery (TAMPER.md → "Device discovery"). Only runs when a `discover`
//! command arrives — never unsolicited. Reads the ARP cache and does a light TCP
//! connect sweep on common ports, returning hosts as a `discovery_result` event.

use crate::protocol::{Event, EV_DISCOVERY_RESULT, SEV_INFO};
use serde::Serialize;
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::TcpStream;

/// Common ports probed to fingerprint a host's role.
const COMMON_PORTS: &[u16] = &[22, 80, 443, 445, 3389, 5900, 8080, 62078];
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Serialize)]
pub struct Host {
    pub ip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    pub open_ports: Vec<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
}

/// Parse `/proc/net/arp` into (ip, mac) pairs, skipping incomplete entries.
pub fn parse_arp(contents: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for line in contents.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // IP HWtype Flags HWaddress Mask Device
        if cols.len() < 4 {
            continue;
        }
        let ip = cols[0].to_string();
        let mac = cols[3];
        let mac = if mac == "00:00:00:00:00:00" {
            None
        } else {
            Some(mac.to_string())
        };
        out.push((ip, mac));
    }
    out
}

/// TCP connect sweep of one host's common ports.
async fn scan_ports(ip: Ipv4Addr) -> Vec<u16> {
    let mut open = Vec::new();
    for &port in COMMON_PORTS {
        let addr = SocketAddr::new(IpAddr::V4(ip), port);
        if let Ok(Ok(_)) = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            open.push(port);
        }
    }
    open
}

/// Derive the local /24 to sweep from the ARP cache (skeleton heuristic). A real
/// build enumerates interface addresses/netmasks; here we sweep the /24 of the
/// first IPv4 seen in ARP, which covers the common home-LAN case.
fn subnet_base(arp: &[(String, Option<String>)]) -> Option<[u8; 3]> {
    for (ip, _) in arp {
        if let Ok(v4) = ip.parse::<Ipv4Addr>() {
            let o = v4.octets();
            return Some([o[0], o[1], o[2]]);
        }
    }
    None
}

/// Run the sweep and return a `discovery_result` event.
pub async fn run() -> Event {
    let arp_raw = std::fs::read_to_string("/proc/net/arp").unwrap_or_default();
    let arp = parse_arp(&arp_raw);
    tracing::info!(
        "discovery: {} ARP neighbor(s), starting TCP sweep",
        arp.len()
    );

    let mut hosts: Vec<Host> = Vec::new();

    // 1) Known neighbors from ARP (have a MAC).
    for (ip, mac) in &arp {
        if let Ok(v4) = ip.parse::<Ipv4Addr>() {
            let open = scan_ports(v4).await;
            hosts.push(Host {
                ip: ip.clone(),
                mac: mac.clone(),
                hostname: None,
                open_ports: open,
                vendor: mac.as_deref().map(oui_vendor).unwrap_or(None),
            });
        }
    }

    // 2) Light sweep across the rest of the /24 to find hosts not yet in ARP.
    if let Some(base) = subnet_base(&arp) {
        let known: std::collections::HashSet<String> = hosts.iter().map(|h| h.ip.clone()).collect();
        let mut tasks = Vec::new();
        for host_octet in 1u16..=254 {
            let ip = Ipv4Addr::new(base[0], base[1], base[2], host_octet as u8);
            if known.contains(&ip.to_string()) {
                continue;
            }
            tasks.push(tokio::spawn(async move {
                let open = scan_ports(ip).await;
                (ip, open)
            }));
        }
        for t in tasks {
            if let Ok((ip, open)) = t.await {
                if !open.is_empty() {
                    hosts.push(Host {
                        ip: ip.to_string(),
                        mac: None,
                        hostname: None,
                        open_ports: open,
                        vendor: None,
                    });
                }
            }
        }
    }

    tracing::info!("discovery: {} host(s) found", hosts.len());
    Event::new(EV_DISCOVERY_RESULT, SEV_INFO, json!({ "hosts": hosts }))
}

/// Trivial OUI → vendor stub (first three MAC octets). Real build ships an OUI DB.
fn oui_vendor(mac: &str) -> Option<String> {
    let prefix = mac.get(0..8)?.to_uppercase();
    let vendor = match prefix.as_str() {
        "B8:27:EB" | "DC:A6:32" | "E4:5F:01" => "Raspberry Pi",
        "F0:18:98" | "A4:83:E7" | "AC:BC:32" => "Apple",
        _ => return None,
    };
    Some(vendor.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_arp_table() {
        let raw =
            "IP address       HW type     Flags       HW address            Mask     Device\n\
                   192.168.1.1      0x1         0x2         aa:bb:cc:dd:ee:ff     *        eth0\n\
                   192.168.1.5      0x1         0x0         00:00:00:00:00:00     *        eth0\n";
        let arp = parse_arp(raw);
        assert_eq!(arp.len(), 2);
        assert_eq!(arp[0].0, "192.168.1.1");
        assert_eq!(arp[0].1.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
        assert_eq!(arp[1].1, None); // incomplete entry
    }
}
