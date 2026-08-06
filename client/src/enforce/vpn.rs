//! Device VPN profile enforcement: reconcile the admin-uploaded WireGuard /
//! OpenVPN client config from the policy bundle onto this host.
//!
//! WireGuard configs land in `/etc/wireguard/sentinel.conf` and run as
//! `wg-quick@sentinel`; OpenVPN configs land in
//! `/etc/openvpn/client/sentinel.conf` and run as `openvpn-client@sentinel`.
//! Config bodies carry private keys, so files are written `0600` root-only and
//! their contents are withheld from dry-run logs.
//!
//! The firewall must cooperate: under `default_deny` (and `block_vpn`
//! lockdown!) the tunnel's own handshake would be dropped, so
//! [`plan`] extracts the endpoint + tunnel interface for `firewall.rs` to
//! whitelist ahead of the drop rules. Failures follow the DNS-gap doctrine: a
//! tunnel that is not actually up is reported as a [`VpnGap`], never a silent
//! success.

use crate::policy::VpnProfile;
use crate::util::Exec;
use anyhow::Result;

const WG_CONF: &str = "/etc/wireguard/sentinel.conf";
const WG_UNIT: &str = "wg-quick@sentinel";
/// wg-quick@sentinel names the tunnel interface after the unit instance.
const WG_IFACE: &str = "sentinel";

const OVPN_CONF: &str = "/etc/openvpn/client/sentinel.conf";
const OVPN_UNIT: &str = "openvpn-client@sentinel";
/// OpenVPN's default `dev tun` allocates tun0/tun1/… — match them all.
const OVPN_IFACE: &str = "tun*";

/// A reason the VPN profile is not actually in force on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnGap {
    /// The config was written but the tunnel service is not active.
    NotRunning,
    /// The server sent a profile kind this agent doesn't know how to apply.
    UnsupportedKind,
}

impl VpnGap {
    /// Stable machine-readable identifier (event payload `kind`).
    pub fn kind(self) -> &'static str {
        match self {
            VpnGap::NotRunning => "vpn_not_running",
            VpnGap::UnsupportedKind => "vpn_unsupported_kind",
        }
    }

    /// Operator-facing explanation, in the terms the parent/admin needs.
    pub fn explain(self) -> &'static str {
        match self {
            VpnGap::NotRunning => {
                "the VPN profile was written but its service is not running — \
                 wireguard-tools (wg-quick) or openvpn is probably not installed \
                 on this device, or the config is rejected. Traffic is NOT going \
                 through the VPN."
            }
            VpnGap::UnsupportedKind => {
                "the server sent a VPN profile kind this agent version does not \
                 support — update the agent or re-upload as WireGuard/OpenVPN."
            }
        }
    }
}

/// What the VPN needs from the firewall to function: an accept for the tunnel
/// interface pattern plus accepts for the tunnel endpoint(s), all of which must
/// precede the lockdown drop rules (`block_vpn` drops udp 51820/1194!).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VpnPlan {
    /// nft `oifname` pattern of the tunnel interface (`"sentinel"` / `"tun*"`).
    pub iface: Option<&'static str>,
    /// Tunnel endpoints to whitelist in the output chain.
    pub endpoints: Vec<Endpoint>,
}

/// One tunnel endpoint from the client config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Hostname or IP literal, as written in the config.
    pub host: String,
    pub port: u16,
    /// `"udp"` or `"tcp"` (nft keyword).
    pub proto: &'static str,
}

/// Directive for [`reconcile`]: CLI paths that hold no server state (e.g.
/// `openscreentime unlock` re-applying the cached policy) must not tear down a
/// tunnel they know nothing about.
#[derive(Debug, Clone, Copy)]
pub enum VpnState<'a> {
    /// Leave whatever profile is on disk untouched (but still whitelist it).
    Keep,
    /// Reconcile to the server's device-level profile; `None` = remove.
    Sync(Option<&'a VpnProfile>),
}

/// Extract the firewall requirements for the given state — from the incoming
/// profile when syncing, or from the on-disk config when keeping — so the
/// ruleset can be rendered correctly even in dry-run (where nothing is written).
pub fn plan(state: &VpnState) -> VpnPlan {
    let from_disk = || {
        for (path, kind) in [(WG_CONF, "wireguard"), (OVPN_CONF, "openvpn")] {
            if let Ok(config) = std::fs::read_to_string(path) {
                return plan_for(kind, &config);
            }
        }
        VpnPlan::default()
    };
    match state {
        VpnState::Keep | VpnState::Sync(None) => from_disk(),
        VpnState::Sync(Some(p)) => plan_for(&p.kind, &p.config),
    }
}

fn plan_for(kind: &str, config: &str) -> VpnPlan {
    match kind {
        "wireguard" => VpnPlan {
            iface: Some(WG_IFACE),
            endpoints: parse_wg_endpoints(config),
        },
        "openvpn" => VpnPlan {
            iface: Some(OVPN_IFACE),
            endpoints: parse_ovpn_endpoints(config),
        },
        _ => VpnPlan::default(),
    }
}

/// `Endpoint = host:port` lines from a wg config (always UDP).
fn parse_wg_endpoints(config: &str) -> Vec<Endpoint> {
    config
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            if !key.trim().eq_ignore_ascii_case("endpoint") {
                return None;
            }
            // Split host:port from the right so a bracketed IPv6 host survives.
            let value = value.trim();
            let (host, port) = value.rsplit_once(':')?;
            Some(Endpoint {
                host: host.trim_matches(['[', ']']).to_string(),
                port: port.trim().parse().ok()?,
                proto: "udp",
            })
        })
        .collect()
}

/// `remote <host> [port] [proto]` lines from an ovpn config, with the global
/// `port` / `proto` directives as fallbacks (OpenVPN defaults: 1194/udp).
fn parse_ovpn_endpoints(config: &str) -> Vec<Endpoint> {
    let global = |name: &str| {
        config.lines().find_map(|l| {
            let mut it = l.split_whitespace();
            (it.next() == Some(name))
                .then(|| it.next())?
                .map(String::from)
        })
    };
    let global_port: u16 = global("port").and_then(|p| p.parse().ok()).unwrap_or(1194);
    let global_proto = if global("proto").is_some_and(|p| p.starts_with("tcp")) {
        "tcp"
    } else {
        "udp"
    };

    config
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            if it.next() != Some("remote") {
                return None;
            }
            let host = it.next()?.to_string();
            let port = it
                .next()
                .and_then(|p| p.parse().ok())
                .unwrap_or(global_port);
            let proto = match it.next() {
                Some(p) if p.starts_with("tcp") => "tcp",
                Some(_) => "udp",
                None => global_proto,
            };
            Some(Endpoint { host, port, proto })
        })
        .collect()
}

/// The agent's verdict on a profile it was asked to test — reported back to
/// the server as a `vpn_profile` event so the admin sees ACTIVE or FAILED
/// (with the reason) instead of guessing.
#[derive(Debug, Clone)]
pub struct VpnReport {
    pub profile_id: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Reconcile the on-host tunnel to `state`. Called from
/// `apply_network_policy` AFTER the firewall (so the endpoint accepts are in
/// place before the handshake). Returns the gaps preventing the tunnel from
/// actually being in force, plus — when the server asked for a test cycle or
/// the config changed — the verdict to report back.
///
/// Test-before-enforce: a new/changed config is brought up and VERIFIED
/// (service active + tunnel interface carrying traffic). If verification
/// fails, the previous on-disk config is restored and restarted, so a broken
/// upload can never silently strand the device behind a dead tunnel.
pub fn reconcile(exec: &Exec, state: &VpnState) -> Result<(Vec<VpnGap>, Option<VpnReport>)> {
    let profile = match state {
        VpnState::Keep => return Ok((Vec::new(), None)),
        VpnState::Sync(p) => *p,
    };

    match profile {
        None => {
            teardown(exec, WG_CONF, WG_UNIT);
            teardown(exec, OVPN_CONF, OVPN_UNIT);
            Ok((Vec::new(), None))
        }
        Some(p) => {
            let (conf, unit, other) = match p.kind.as_str() {
                "wireguard" => (WG_CONF, WG_UNIT, (OVPN_CONF, OVPN_UNIT)),
                "openvpn" => (OVPN_CONF, OVPN_UNIT, (WG_CONF, WG_UNIT)),
                _ => {
                    tracing::error!(
                        "VPN gap [{}]: {}",
                        VpnGap::UnsupportedKind.kind(),
                        VpnGap::UnsupportedKind.explain()
                    );
                    let report = p.id.clone().map(|profile_id| VpnReport {
                        profile_id,
                        ok: false,
                        error: Some("unsupported profile kind".into()),
                    });
                    return Ok((vec![VpnGap::UnsupportedKind], report));
                }
            };

            // Does anything need doing? Unchanged config + running unit + no
            // pending test request = leave it alone.
            let on_disk = std::fs::read_to_string(conf).ok();
            let unchanged = on_disk.as_deref() == Some(p.config.as_str());
            let running =
                exec.dry_run() || exec.probe("systemctl", &["is-active", unit]).trim() == "active";
            let testing = p.status.as_deref() == Some("testing");
            if unchanged && running && !testing {
                return Ok((Vec::new(), None));
            }

            // Snapshot the previous world for rollback: whichever managed conf
            // exists right now (same kind or the other one).
            let backup: Option<(&str, &str, String)> = [(conf, unit), (other.0, other.1)]
                .into_iter()
                .find_map(|(c, u)| std::fs::read_to_string(c).ok().map(|body| (c, u, body)));

            // Only one managed tunnel at a time: a kind switch removes the other.
            teardown(exec, other.0, other.1);

            write_secret(exec, conf, &p.config)?;
            // enable = survive reboot; restart (not `start`) = pick up a changed
            // config on an already-running tunnel.
            let _ = exec.run("systemctl", &["enable", unit]);
            if let Err(e) = exec.run("systemctl", &["restart", unit]) {
                tracing::error!("{unit} restart failed: {e}");
            }

            if exec.dry_run() {
                return Ok((Vec::new(), None));
            }

            match verify_tunnel(exec, p.kind.as_str(), unit) {
                Ok(()) => {
                    tracing::info!("VPN profile verified ({} via {unit})", p.kind);
                    let report = p.id.clone().map(|profile_id| VpnReport {
                        profile_id,
                        ok: true,
                        error: None,
                    });
                    Ok((Vec::new(), report))
                }
                Err(why) => {
                    tracing::error!("VPN profile FAILED verification: {why} — rolling back");
                    // Roll back to the previous world so the device isn't
                    // stranded behind a dead tunnel.
                    teardown(exec, conf, unit);
                    if let Some((prev_conf, prev_unit, body)) = backup {
                        if write_secret(exec, prev_conf, &body).is_ok() {
                            let _ = exec.run("systemctl", &["enable", prev_unit]);
                            let _ = exec.run("systemctl", &["restart", prev_unit]);
                            tracing::info!("previous VPN profile restored ({prev_unit})");
                        }
                    }
                    let report = p.id.clone().map(|profile_id| VpnReport {
                        profile_id,
                        ok: false,
                        error: Some(why.clone()),
                    });
                    Ok((vec![VpnGap::NotRunning], report))
                }
            }
        }
    }
}

/// Budget for a tunnel to come up and prove itself.
const VERIFY_TIMEOUT_SECS: u64 = 12;

/// Prove the tunnel is genuinely in force: the unit is active AND the tunnel
/// interface exists AND (for WireGuard) a peer handshake completed — induced
/// by pinging through the interface, since wg handshakes only on traffic.
fn verify_tunnel(exec: &Exec, kind: &str, unit: &str) -> std::result::Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(VERIFY_TIMEOUT_SECS);
    let mut last = String::from("verification never ran");
    while std::time::Instant::now() < deadline {
        if exec.probe("systemctl", &["is-active", unit]).trim() != "active" {
            last = format!("{unit} is not active");
            std::thread::sleep(std::time::Duration::from_secs(1));
            continue;
        }
        match kind {
            "wireguard" => {
                // Induce traffic, then check for any nonzero peer handshake.
                let _ = exec.probe("ping", &["-c", "1", "-W", "2", "-I", WG_IFACE, "1.1.1.1"]);
                let hs = exec.probe("wg", &["show", WG_IFACE, "latest-handshakes"]);
                let ok = hs
                    .split_whitespace()
                    .filter_map(|t| t.parse::<u64>().ok())
                    .any(|ts| ts > 0);
                if ok {
                    return Ok(());
                }
                last = "wireguard peer never completed a handshake".into();
            }
            "openvpn" => {
                // OpenVPN allocates its tun device only once connected.
                let links = exec.probe("ip", &["-o", "link"]);
                if links.lines().any(|l| l.contains(": tun")) {
                    return Ok(());
                }
                last = "openvpn never brought a tun interface up".into();
            }
            _ => return Err("unsupported kind".into()),
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    Err(last)
}

/// Stop/disable a managed tunnel unit and remove its config — but only if our
/// config file actually exists, so an agent with no VPN history never churns
/// systemctl on every policy apply.
fn teardown(exec: &Exec, conf: &str, unit: &str) {
    if !std::path::Path::new(conf).exists() {
        return;
    }
    let _ = exec.run("systemctl", &["disable", "--now", unit]);
    if exec.dry_run() {
        tracing::info!(target: "dry_run", "WOULD REMOVE {conf}");
    } else if let Err(e) = std::fs::remove_file(conf) {
        tracing::warn!("could not remove {conf}: {e}");
    } else {
        tracing::info!("VPN profile removed ({unit} stopped, {conf} deleted)");
    }
}

/// Write a key-bearing config: `0600` root-only from the first byte (fresh
/// file via `create_new`), and never echo the contents into dry-run logs the
/// way `Exec::write_file` does.
fn write_secret(exec: &Exec, path: &str, contents: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if exec.dry_run() {
        tracing::info!(
            target: "dry_run",
            "WOULD WRITE {path} ({} bytes) [contents withheld — contains private keys]",
            contents.len()
        );
        return Ok(());
    }
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(contents.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wg_endpoint_parsed() {
        let conf = "[Interface]\nPrivateKey = abc\nAddress = 10.0.0.2/32\n\n\
                    [Peer]\nPublicKey = xyz\nEndpoint = vpn.example.org:51820\n\
                    AllowedIPs = 0.0.0.0/0\n";
        assert_eq!(
            parse_wg_endpoints(conf),
            vec![Endpoint {
                host: "vpn.example.org".into(),
                port: 51820,
                proto: "udp"
            }]
        );
    }

    #[test]
    fn wg_ipv6_bracketed_endpoint_parsed() {
        let conf = "Endpoint = [2001:db8::1]:51820\n";
        let eps = parse_wg_endpoints(conf);
        assert_eq!(eps[0].host, "2001:db8::1");
        assert_eq!(eps[0].port, 51820);
    }

    #[test]
    fn ovpn_remote_lines_parsed_with_defaults_and_overrides() {
        let conf = "client\nproto tcp\nremote a.example.org\nremote b.example.org 443\n\
                    remote c.example.org 1194 udp\n";
        let eps = parse_ovpn_endpoints(conf);
        assert_eq!(eps.len(), 3);
        // No port/proto on the line → global directives (proto tcp, default 1194).
        assert_eq!((eps[0].port, eps[0].proto), (1194, "tcp"));
        assert_eq!((eps[1].port, eps[1].proto), (443, "tcp"));
        // Per-line proto wins over the global directive.
        assert_eq!((eps[2].port, eps[2].proto), (1194, "udp"));
    }

    #[test]
    fn plan_matches_kind() {
        let wg = plan_for("wireguard", "Endpoint = 1.2.3.4:51820\n");
        assert_eq!(wg.iface, Some("sentinel"));
        assert_eq!(wg.endpoints.len(), 1);
        let ovpn = plan_for("openvpn", "remote 1.2.3.4 1194 udp\n");
        assert_eq!(ovpn.iface, Some("tun*"));
        let unknown = plan_for("ipsec", "whatever");
        assert_eq!(unknown, VpnPlan::default());
    }

    /// The `kind` strings land in stored event payloads — they are API. Pin them.
    #[test]
    fn gap_kinds_are_stable_identifiers() {
        assert_eq!(VpnGap::NotRunning.kind(), "vpn_not_running");
        assert_eq!(VpnGap::UnsupportedKind.kind(), "vpn_unsupported_kind");
    }

    /// Every gap must tell the operator what to do about it.
    #[test]
    fn every_gap_explains_itself() {
        for gap in [VpnGap::NotRunning, VpnGap::UnsupportedKind] {
            assert!(gap.explain().len() > 40, "{} has no guidance", gap.kind());
        }
    }
}
