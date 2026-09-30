//! Safe search, the way the search engines document it: a network makes
//! `www.google.com` (and the rest) answer with the address of the engine's
//! safe-search front end — `forcesafesearch.google.com`, `restrict.youtube.com`,
//! `strict.bing.com`, `safe.duckduckgo.com` — which serves the same site with
//! safe search locked on.
//!
//! dnsmasq can't do that with `cname=`: its target must be a name dnsmasq
//! itself knows (hosts, DHCP), never one it only learns from upstream. A bare
//! `cname=www.google.com,forcesafesearch.google.com` answers with the CNAME and
//! no address, so Google, YouTube and Bing stopped resolving on every computer
//! with safe search on (acceptance round 3) — they worked only while the target
//! happened to sit in the cache.
//!
//! So the agent looks the targets up itself, straight from the family upstream
//! ([`resolve`]), and dnsmasq answers every covered name with those addresses
//! (`host-record=`, see [`render`]). The lookups are refreshed hourly and when
//! the network changes (the runner's `safesearch_loop`), and kept on disk so a
//! reboot enforces at once. A target that can't be resolved is **not**
//! redirected: its names pass through to the upstream like any other — a
//! working site without safe search, never a broken one — and the gap
//! `dns_safesearch_unavailable` says so.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// Every country domain Google search runs on (<https://www.google.com/supported_domains>,
/// without the leading `.google.`). Google documents forcing SafeSearch by
/// pointing `www.google.<each>` at `forcesafesearch.google.com`; the bare
/// domains only redirect to `www.`.
pub const GOOGLE_DOMAINS: &[&str] = &[
    "com", "ad", "ae", "com.af", "com.ag", "al", "am", "co.ao", "com.ar", "as", "at", "com.au",
    "az", "ba", "com.bd", "be", "bf", "bg", "com.bh", "bi", "bj", "com.bn", "com.bo", "com.br",
    "bs", "bt", "co.bw", "by", "com.bz", "ca", "cd", "cf", "cg", "ch", "ci", "co.ck", "cl", "cm",
    "cn", "com.co", "co.cr", "com.cu", "cv", "com.cy", "cz", "de", "dj", "dk", "dm", "com.do",
    "dz", "com.ec", "ee", "com.eg", "es", "com.et", "fi", "com.fj", "fm", "fr", "ga", "ge", "gg",
    "com.gh", "com.gi", "gl", "gm", "gr", "com.gt", "gy", "com.hk", "hn", "hr", "ht", "hu",
    "co.id", "ie", "co.il", "im", "co.in", "iq", "is", "it", "je", "com.jm", "jo", "co.jp",
    "co.ke", "com.kh", "ki", "kg", "co.kr", "com.kw", "kz", "la", "com.lb", "li", "lk", "co.ls",
    "lt", "lu", "lv", "com.ly", "co.ma", "md", "me", "mg", "mk", "ml", "com.mm", "mn", "com.mt",
    "mu", "mv", "mw", "com.mx", "com.my", "co.mz", "com.na", "com.ng", "com.ni", "ne", "nl", "no",
    "com.np", "nr", "nu", "co.nz", "com.om", "com.pa", "com.pe", "com.pg", "com.ph", "com.pk",
    "pl", "pn", "com.pr", "ps", "pt", "com.py", "com.qa", "ro", "ru", "rw", "com.sa", "com.sb",
    "sc", "se", "com.sg", "sh", "si", "sk", "com.sl", "sn", "so", "sm", "sr", "st", "com.sv", "td",
    "tg", "co.th", "com.tj", "tl", "tm", "tn", "to", "com.tr", "tt", "com.tw", "co.tz", "com.ua",
    "co.ug", "co.uk", "com.uy", "co.uz", "com.vc", "co.ve", "co.vi", "com.vn", "vu", "ws", "rs",
    "co.za", "co.zm", "co.zw", "cat",
];

/// One search engine: the safe-search front end its own documentation names,
/// and the hostnames a network points at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Engine {
    pub name: &'static str,
    pub target: &'static str,
    pub hosts: Vec<String>,
}

/// The engines safe search covers, each as its vendor documents it.
pub fn engines() -> Vec<Engine> {
    vec![
        Engine {
            name: "Google",
            target: "forcesafesearch.google.com",
            hosts: GOOGLE_DOMAINS
                .iter()
                .map(|d| format!("www.google.{d}"))
                .collect(),
        },
        // Google Workspace's "Restrict YouTube content" (DNS option): these
        // five names → restrict.youtube.com (Strict Restricted Mode).
        Engine {
            name: "YouTube",
            target: "restrict.youtube.com",
            hosts: [
                "www.youtube.com",
                "m.youtube.com",
                "youtubei.googleapis.com",
                "youtube.googleapis.com",
                "www.youtube-nocookie.com",
            ]
            .map(String::from)
            .to_vec(),
        },
        Engine {
            name: "Bing",
            target: "strict.bing.com",
            hosts: vec!["www.bing.com".into()],
        },
        Engine {
            name: "DuckDuckGo",
            target: "safe.duckduckgo.com",
            hosts: vec!["duckduckgo.com".into()],
        },
    ]
}

/// A safe-search front end's addresses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Addrs {
    #[serde(default)]
    pub v4: Vec<Ipv4Addr>,
    #[serde(default)]
    pub v6: Vec<Ipv6Addr>,
}

impl Addrs {
    /// Usable as a redirect: at least one IPv4 address. (An IPv6-only answer
    /// would take the site away from every computer without IPv6.)
    pub fn usable(&self) -> bool {
        !self.v4.is_empty()
    }
}

/// What the lookups found, kept across restarts (`safesearch.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeSearch {
    /// Target → its addresses, from the latest lookup that answered.
    #[serde(default)]
    pub targets: BTreeMap<String, Addrs>,
    /// Targets the latest lookup couldn't get addresses for.
    #[serde(default)]
    pub failed: BTreeSet<String>,
    /// The upstream asked.
    #[serde(default)]
    pub upstream: String,
    /// When (unix seconds); 0: never looked up.
    #[serde(default)]
    pub at: i64,
}

impl SafeSearch {
    /// A lookup has been tried at least once.
    pub fn attempted(&self) -> bool {
        self.at > 0
    }

    /// The addresses to redirect `target`'s names to, if usable.
    pub fn usable(&self, target: &str) -> Option<&Addrs> {
        self.targets.get(target).filter(|a| a.usable())
    }

    /// Every target has addresses to redirect to.
    pub fn complete(&self) -> bool {
        engines().iter().all(|e| self.usable(e.target).is_some())
    }

    /// Fold a lookup round in: an answer replaces what was known (an answer
    /// with no addresses removes it — the name really has none); a target
    /// that got no answer at all keeps its last known addresses (a network
    /// blip must not take safe search away, nor bring a broken site).
    pub fn merge(&mut self, upstream: &str, round: &Round, now: i64) {
        self.failed.clear();
        for e in engines() {
            match round.get(e.target) {
                Some(Some(addrs)) if addrs.usable() => {
                    self.targets.insert(e.target.to_string(), addrs.clone());
                }
                Some(Some(_)) => {
                    self.targets.remove(e.target);
                    self.failed.insert(e.target.to_string());
                }
                Some(None) | None => {
                    self.failed.insert(e.target.to_string());
                }
            }
        }
        self.upstream = upstream.to_string();
        self.at = now;
    }

    pub fn load() -> SafeSearch {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Ok(json) = serde_json::to_string(self) else {
            return;
        };
        if let Err(e) = std::fs::write(path(), json) {
            tracing::warn!("could not keep the safe-search addresses: {e}");
        }
    }
}

fn path() -> std::path::PathBuf {
    crate::paths::state("safesearch.json")
}

/// One lookup round: target → `Some(addresses)` when the upstream answered
/// (possibly with none), `None` when it didn't answer.
pub type Round = BTreeMap<String, Option<Addrs>>;

/// The dnsmasq lines that redirect every covered name `allowed` lets through
/// (a name under a block, or outside a default-deny allowlist, is left to
/// that rule), and the engines that could not be redirected — passed through
/// instead.
pub fn render(safe: &SafeSearch, allowed: &dyn Fn(&str) -> bool) -> (String, Vec<&'static str>) {
    let mut out = String::new();
    let mut unavailable = Vec::new();
    for e in engines() {
        let Some(addrs) = safe.usable(e.target) else {
            out.push_str(&format!(
                "# safe search: {} unavailable ({} not resolved) — passed through\n",
                e.name, e.target
            ));
            unavailable.push(e.name);
            continue;
        };
        out.push_str(&format!("# safe search: {} → {}\n", e.name, e.target));
        let rows = addrs.v4.len().max(addrs.v6.len());
        for host in e.hosts.iter().filter(|h| allowed(h)) {
            for i in 0..rows {
                let mut line = format!("host-record={host}");
                if let Some(a) = addrs.v4.get(i) {
                    line.push_str(&format!(",{a}"));
                }
                if let Some(a) = addrs.v6.get(i) {
                    line.push_str(&format!(",{a}"));
                }
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    (out, unavailable)
}

/// `name` is `domain` or a name under it.
pub fn under(name: &str, domain: &str) -> bool {
    let (name, domain) = (name.to_ascii_lowercase(), domain.to_ascii_lowercase());
    name == domain || name.ends_with(&format!(".{domain}"))
}

// ── Looking the targets up ───────────────────────────────────────────────────

const QTYPE_A: u16 = 1;
const QTYPE_AAAA: u16 = 28;

/// A DNS query for `name`/`qtype`, recursion desired.
pub fn query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
    let mut q = Vec::with_capacity(32 + name.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&qtype.to_be_bytes());
    q.extend_from_slice(&1u16.to_be_bytes());
    q
}

/// What an upstream said to one query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The addresses in the answer (following its CNAME chain), maybe none
    /// (NXDOMAIN, or no record of that type).
    Addrs(Vec<IpAddr>),
    /// Not an answer to rely on (SERVFAIL, REFUSED, truncated, garbled).
    Failed,
}

/// Skip a (possibly compressed) name at `pos`; the offset after it.
fn skip_name(msg: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *msg.get(pos)?;
        match len {
            0 => return Some(pos + 1),
            l if l & 0xC0 == 0xC0 => return Some(pos + 2),
            l => pos += 1 + l as usize,
        }
    }
}

/// Read the name at `pos` (compression followed, bounded).
fn read_name(msg: &[u8], mut pos: usize) -> Option<String> {
    let mut labels = Vec::new();
    for _ in 0..64 {
        let len = *msg.get(pos)?;
        if len == 0 {
            return Some(labels.join("."));
        }
        if len & 0xC0 == 0xC0 {
            let lo = *msg.get(pos + 1)?;
            pos = (usize::from(len & 0x3F) << 8) | usize::from(lo);
            continue;
        }
        let l = usize::from(len);
        labels.push(String::from_utf8_lossy(msg.get(pos + 1..pos + 1 + l)?).to_ascii_lowercase());
        pos += 1 + l;
    }
    None
}

/// Parse the answer to [`query`]`(id, name, qtype)`. `None`: not that answer.
pub fn parse(msg: &[u8], id: u16, name: &str, qtype: u16) -> Option<Answer> {
    if msg.len() < 12 || u16::from_be_bytes([msg[0], msg[1]]) != id || msg[2] & 0x80 == 0 {
        return None;
    }
    let qd = u16::from_be_bytes([msg[4], msg[5]]);
    let an = u16::from_be_bytes([msg[6], msg[7]]);
    if qd != 1 {
        return None;
    }
    let q_end = skip_name(msg, 12)?;
    let asked = read_name(msg, 12)?;
    let q_type = u16::from_be_bytes([*msg.get(q_end)?, *msg.get(q_end + 1)?]);
    if asked != name.trim_end_matches('.').to_ascii_lowercase() || q_type != qtype {
        return None;
    }
    if msg[2] & 0x02 != 0 {
        return Some(Answer::Failed); // truncated
    }
    match msg[3] & 0x0F {
        0 => {}
        3 => return Some(Answer::Addrs(Vec::new())), // NXDOMAIN
        _ => return Some(Answer::Failed),
    }
    let mut pos = q_end + 4;
    let mut addrs = Vec::new();
    for _ in 0..an {
        pos = skip_name(msg, pos)?;
        let rtype = u16::from_be_bytes([*msg.get(pos)?, *msg.get(pos + 1)?]);
        let rdlen = usize::from(u16::from_be_bytes([*msg.get(pos + 8)?, *msg.get(pos + 9)?]));
        let rdata = msg.get(pos + 10..pos + 10 + rdlen)?;
        match (rtype, rdlen) {
            (QTYPE_A, 4) if qtype == QTYPE_A => {
                addrs.push(IpAddr::from([rdata[0], rdata[1], rdata[2], rdata[3]]))
            }
            (QTYPE_AAAA, 16) if qtype == QTYPE_AAAA => {
                let b: [u8; 16] = rdata.try_into().ok()?;
                addrs.push(IpAddr::from(b));
            }
            _ => {} // the CNAME chain, anything else
        }
        pos += 10 + rdlen;
    }
    Some(Answer::Addrs(addrs))
}

/// Look every engine's target up (A and AAAA) straight from `upstream`:
/// all queries at once, one resend, `deadline` in all. Root may talk :53 to
/// the upstream under `force_dns` (firewall.rs), and nothing local — the
/// resolver we are configuring — is in the way.
pub async fn resolve(upstream: IpAddr, deadline: Duration) -> Round {
    let targets: Vec<&'static str> = engines().iter().map(|e| e.target).collect();
    let mut round: Round = targets.iter().map(|t| (t.to_string(), None)).collect();
    let bind: SocketAddr = if upstream.is_ipv4() {
        (Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (Ipv6Addr::UNSPECIFIED, 0).into()
    };
    let sock = match tokio::net::UdpSocket::bind(bind).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("safe search: no socket for the lookups: {e}");
            return round;
        }
    };
    if let Err(e) = sock.connect(SocketAddr::new(upstream, 53)).await {
        tracing::debug!("safe search: upstream {upstream} unreachable: {e}");
        return round;
    }
    // (target, qtype, id) → its answer.
    let base: u16 = rand::random();
    let mut pending: Vec<(&str, u16, u16)> = Vec::new();
    for (i, t) in targets.iter().enumerate() {
        for (j, qt) in [QTYPE_A, QTYPE_AAAA].into_iter().enumerate() {
            pending.push((t, qt, base.wrapping_add((i * 2 + j) as u16)));
        }
    }
    let mut got: BTreeMap<(String, u16), Answer> = BTreeMap::new();
    let start = tokio::time::Instant::now();
    let mut buf = vec![0u8; 4096];
    for attempt in 0..2 {
        for (t, qt, id) in &pending {
            if !got.contains_key(&(t.to_string(), *qt)) {
                let _ = sock.send(&query(*id, t, *qt)).await;
            }
        }
        let until = start + deadline * (attempt + 1) / 2;
        while got.len() < pending.len() {
            let Ok(Ok(n)) = tokio::time::timeout_at(until, sock.recv(&mut buf)).await else {
                break;
            };
            for (t, qt, id) in &pending {
                if let Some(a) = parse(&buf[..n], *id, t, *qt) {
                    got.insert((t.to_string(), *qt), a);
                }
            }
        }
        if got.len() == pending.len() {
            break;
        }
    }
    for t in &targets {
        let a = got.get(&(t.to_string(), QTYPE_A));
        let aaaa = got.get(&(t.to_string(), QTYPE_AAAA));
        // Only an A answer decides: without IPv4 there is nothing to redirect to.
        let Some(Answer::Addrs(v4)) = a else {
            continue;
        };
        let mut addrs = Addrs::default();
        for ip in v4 {
            if let IpAddr::V4(ip) = ip {
                addrs.v4.push(*ip);
            }
        }
        if let Some(Answer::Addrs(v6)) = aaaa {
            for ip in v6 {
                if let IpAddr::V6(ip) = ip {
                    addrs.v6.push(*ip);
                }
            }
        }
        addrs.v4.sort();
        addrs.v4.dedup();
        addrs.v6.sort();
        addrs.v6.dedup();
        round.insert(t.to_string(), Some(addrs));
    }
    round
}

/// The routes out of this computer (default routes, IPv4 and IPv6) — a
/// change means a different network, whose safe-search front end may be a
/// different one.
pub fn network_fingerprint() -> String {
    let v4 = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    let v6 = std::fs::read_to_string("/proc/net/ipv6_route").unwrap_or_default();
    fingerprint(&v4, &v6)
}

/// [`network_fingerprint`] from the two proc files' contents.
pub fn fingerprint(route: &str, ipv6_route: &str) -> String {
    let mut out: Vec<String> = route
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            // Iface Destination Gateway … Mask at 7.
            (f.len() > 7 && f[1] == "00000000" && f[7] == "00000000")
                .then(|| format!("4 {} {}", f[0], f[2]))
        })
        .collect();
    out.extend(ipv6_route.lines().filter_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        // dest prefixlen src srcprefix nexthop metric ref use flags iface
        (f.len() >= 10 && f[0].chars().all(|c| c == '0') && f[1] == "00" && f[9] != "lo")
            .then(|| format!("6 {} {}", f[9], f[4]))
    }));
    out.sort();
    out.dedup();
    out.join(";")
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// The front ends as 1.1.1.3 gave them (2026-09).
    pub fn resolved() -> SafeSearch {
        let mut s = SafeSearch::default();
        let mut round = Round::new();
        round.insert(
            "forcesafesearch.google.com".into(),
            Some(Addrs {
                v4: vec!["216.239.38.120".parse().unwrap()],
                v6: vec!["2001:4860:4802:32::78".parse().unwrap()],
            }),
        );
        round.insert(
            "restrict.youtube.com".into(),
            Some(Addrs {
                v4: vec!["216.239.38.120".parse().unwrap()],
                v6: vec![],
            }),
        );
        round.insert(
            "strict.bing.com".into(),
            Some(Addrs {
                v4: vec![
                    "150.171.27.16".parse().unwrap(),
                    "150.171.28.16".parse().unwrap(),
                ],
                v6: vec!["2620:1ec:33::16".parse().unwrap()],
            }),
        );
        round.insert(
            "safe.duckduckgo.com".into(),
            Some(Addrs {
                v4: vec!["40.114.177.246".parse().unwrap()],
                v6: vec![],
            }),
        );
        s.merge("1.1.1.3", &round, 1);
        s
    }

    #[test]
    fn every_covered_name_answers_with_the_front_ends_addresses() {
        let (conf, unavailable) = render(&resolved(), &|_| true);
        assert!(unavailable.is_empty());
        assert!(conf.contains("host-record=www.google.com,216.239.38.120,2001:4860:4802:32::78\n"));
        assert!(
            conf.contains("host-record=www.google.co.uk,216.239.38.120,2001:4860:4802:32::78\n")
        );
        assert!(conf.contains("host-record=www.google.de,216.239.38.120,"));
        for yt in [
            "www.youtube.com",
            "m.youtube.com",
            "youtubei.googleapis.com",
            "youtube.googleapis.com",
            "www.youtube-nocookie.com",
        ] {
            assert!(
                conf.contains(&format!("host-record={yt},216.239.38.120\n")),
                "{yt}"
            );
        }
        // Two IPv4 addresses, one IPv6: two lines.
        assert!(conf.contains("host-record=www.bing.com,150.171.27.16,2620:1ec:33::16\n"));
        assert!(conf.contains("host-record=www.bing.com,150.171.28.16\n"));
        assert!(conf.contains("host-record=duckduckgo.com,40.114.177.246\n"));
        // Never the bare cname= that broke the sites.
        assert!(!conf.contains("cname="));
        // Every Google country domain, and no line past dnsmasq's line length.
        assert_eq!(GOOGLE_DOMAINS.len(), 187);
        assert!(conf.lines().all(|l| l.len() < 200));
        assert_eq!(
            conf.lines()
                .filter(|l| l.starts_with("host-record=www.google."))
                .count(),
            GOOGLE_DOMAINS.len()
        );
    }

    #[test]
    fn a_target_that_cannot_be_resolved_is_passed_through_not_broken() {
        let mut s = resolved();
        let mut round = Round::new();
        // Bing answered with no address at all; the rest didn't answer.
        round.insert("strict.bing.com".into(), Some(Addrs::default()));
        s.merge("1.1.1.3", &round, 2);
        let (conf, unavailable) = render(&s, &|_| true);
        assert_eq!(unavailable, vec!["Bing"]);
        assert!(!conf.contains("host-record=www.bing.com"));
        assert!(conf.contains("safe search: Bing unavailable"));
        // No answer is not "no address": the last known ones stay.
        assert!(conf.contains("host-record=www.google.com,216.239.38.120"));
        assert!(s.failed.contains("forcesafesearch.google.com"));
        assert!(!s.complete());

        // Never looked up: nothing redirected, and it's not a failure yet.
        let fresh = SafeSearch::default();
        let (conf, unavailable) = render(&fresh, &|_| true);
        assert_eq!(unavailable.len(), 4);
        assert!(!conf.contains("host-record="));
        assert!(!fresh.attempted());
    }

    #[test]
    fn an_ipv6_only_answer_is_not_a_redirect() {
        let mut s = SafeSearch::default();
        let mut round = Round::new();
        round.insert(
            "strict.bing.com".into(),
            Some(Addrs {
                v4: vec![],
                v6: vec!["2620:1ec:33::16".parse().unwrap()],
            }),
        );
        s.merge("1.1.1.3", &round, 1);
        let (conf, _) = render(&s, &|_| true);
        assert!(!conf.contains("www.bing.com"));
    }

    #[test]
    fn names_the_rules_block_or_deny_are_left_to_them() {
        let (conf, _) = render(&resolved(), &|h| !under(h, "youtube.com"));
        assert!(!conf.contains("host-record=www.youtube.com"));
        assert!(!conf.contains("host-record=m.youtube.com"));
        assert!(conf.contains("host-record=youtubei.googleapis.com"));
        assert!(under("www.youtube.com", "youtube.com"));
        assert!(under("YouTube.com", "youtube.com"));
        assert!(!under("notyoutube.com", "youtube.com"));
    }

    /// A real answer from 1.1.1.3 for strict.bing.com: a CNAME chain, then
    /// the addresses.
    #[test]
    fn parses_an_answer_through_its_cname_chain() {
        let q = query(0x1234, "strict.bing.com", QTYPE_A);
        let mut r = q.clone();
        r[2] = 0x81;
        r[3] = 0x80;
        r[7] = 3; // three answers
                  // CNAME strict.bing.com → x.a-msedge.net (pointer to the question name)
        r.extend_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 0, 60]);
        let target = [
            1, b'x', 8, b'a', b'-', b'm', b's', b'e', b'd', b'g', b'e', 3, b'n', b'e', b't', 0,
        ];
        r.extend_from_slice(&(target.len() as u16).to_be_bytes());
        let cname_at = r.len();
        r.extend_from_slice(&target);
        for ip in [[150, 171, 27, 16], [150, 171, 28, 16]] {
            r.extend_from_slice(&[0xC0, cname_at as u8, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
            r.extend_from_slice(&ip);
        }
        assert_eq!(
            parse(&r, 0x1234, "strict.bing.com", QTYPE_A),
            Some(Answer::Addrs(vec![
                "150.171.27.16".parse().unwrap(),
                "150.171.28.16".parse().unwrap()
            ]))
        );
        // Another id, another name: not this answer.
        assert_eq!(parse(&r, 0x1235, "strict.bing.com", QTYPE_A), None);
        assert_eq!(parse(&r, 0x1234, "www.bing.com", QTYPE_A), None);
        // SERVFAIL: no answer to rely on; NXDOMAIN: no address.
        let mut fail = q.clone();
        fail[2] = 0x81;
        fail[3] = 0x82;
        assert_eq!(
            parse(&fail, 0x1234, "strict.bing.com", QTYPE_A),
            Some(Answer::Failed)
        );
        fail[3] = 0x83;
        assert_eq!(
            parse(&fail, 0x1234, "strict.bing.com", QTYPE_A),
            Some(Answer::Addrs(vec![]))
        );
        // A query is not an answer; garbage is nothing.
        assert_eq!(parse(&q, 0x1234, "strict.bing.com", QTYPE_A), None);
        assert_eq!(parse(&[0x12, 0x34, 0x81], 0x1234, "x", QTYPE_A), None);
    }

    #[test]
    fn the_network_is_its_default_routes() {
        let route =
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     ens3\t00000000\t0202000A\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                     ens3\t0002000A\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";
        let v6 = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000001 00000000 00000003 ens3\n\
                  00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200 lo\n";
        assert_eq!(
            fingerprint(route, v6),
            "4 ens3 0202000A;6 ens3 fe800000000000000000000000000001"
        );
        let header_only = route.lines().take(1).collect::<Vec<_>>().join("\n");
        assert_eq!(fingerprint(&header_only, ""), "");
    }

    #[test]
    fn a_round_keeps_its_file_shape() {
        let s = resolved();
        let back: SafeSearch = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert!(back.complete());
        let old: SafeSearch = serde_json::from_str("{}").unwrap();
        assert!(!old.attempted());
    }

    /// The real lookup, against the family resolver (network needed):
    /// `cargo test live_lookup -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_lookup() {
        let round = resolve("1.1.1.3".parse().unwrap(), Duration::from_secs(3)).await;
        let mut s = SafeSearch::default();
        s.merge("1.1.1.3", &round, 1);
        println!("{s:#?}");
        assert!(s.complete(), "{round:#?}");
    }
}
