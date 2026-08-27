//! Where the time goes — the attribution sampler (CONTRACT-0.6 §3).
//!
//! Two signals, both honest about what they are:
//!
//! - **apps**: every tick, one `/proc` walk matched against the WHOLE catalog
//!   (`catalog::comm_to_app`, not just blocked apps). An app earns
//!   tick-seconds while it is *running* for a user who is *active on a seat*
//!   and not frozen — "open", not "focused"; root has no portable way to know
//!   compositor focus.
//! - **sites**: dnsmasq writes an extra-format query log (`dns.rs` enables
//!   it); we tail it, reduce each queried name to its registrable domain, and
//!   count queries per hour, device-wide — resolver traffic has no user.
//!
//! Slices accumulate in memory keyed by (user, hour, kind, key) and are
//! drained to `POST /agent/usage` about once a minute; a failed post keeps
//! the batch (bounded) for the next try.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

/// Where dnsmasq writes its query log (see `enforce/dns.rs`).
pub const DNSQ_LOG: &str = "/var/lib/openscreentime/dnsq.log";
/// Truncate the query log once it grows past this — dnsmasq appends, so a
/// truncate under an O_APPEND writer is safe.
const TRUNCATE_AT: u64 = 20 * 1024 * 1024;
/// At most this many bytes are parsed per tick, so a burst can't stall a tick.
const READ_CAP: usize = 2 * 1024 * 1024;
/// Bounded memory: beyond this many distinct slices, new keys are dropped
/// (existing ones still accumulate) until a drain makes room.
const MAX_PENDING: usize = 2000;

#[derive(Hash, PartialEq, Eq, Clone)]
struct SliceKey {
    /// "" = the whole device (site slices).
    user: String,
    /// RFC3339 of the UTC hour.
    hour: String,
    kind: &'static str,
    key: String,
}

pub struct Attrib {
    pending: HashMap<SliceKey, i64>,
    comm_index: HashMap<&'static str, &'static str>,
    log_offset: u64,
}

fn hour_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:00:00Z")
        .to_string()
}

/// Registrable-domain approximation: the last two labels, or three when the
/// second-to-last is a common public second level (`co.uk`, `com.au`, …).
/// Wrong for exotic suffixes, right for the ones a family actually visits.
pub fn registrable(domain: &str) -> String {
    let d = domain.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = d.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    let second = labels[labels.len() - 2];
    let take = if matches!(second, "co" | "com" | "org" | "net" | "ac" | "gov" | "edu") {
        3
    } else {
        2
    };
    labels[labels.len().saturating_sub(take)..].join(".")
}

/// Pull the queried name out of one extra-format dnsmasq log line:
/// `... query[A] www.youtube.com from 127.0.0.1`.
fn queried_name(line: &str) -> Option<&str> {
    let idx = line.find(" query[")?;
    let rest = &line[idx..];
    let close = rest.find("] ")?;
    let after = &rest[close + 2..];
    let name = after.split_whitespace().next()?;
    (!name.is_empty()).then_some(name)
}

impl Attrib {
    pub fn new() -> Self {
        Attrib {
            pending: HashMap::new(),
            comm_index: openscreentime_policy::catalog::comm_to_app(),
            log_offset: 0,
        }
    }

    fn bump(&mut self, user: &str, kind: &'static str, key: String, amount: i64) {
        let k = SliceKey {
            user: user.to_string(),
            hour: hour_now(),
            kind,
            key,
        };
        if self.pending.len() >= MAX_PENDING && !self.pending.contains_key(&k) {
            return;
        }
        *self.pending.entry(k).or_insert(0) += amount;
    }

    /// One `/proc` walk: tick-seconds for every catalog app running under an
    /// active, unfrozen user. Each (user, app) counts once per tick no matter
    /// how many processes match.
    pub fn sample_apps(&mut self, active_uids: &HashMap<String, u32>, tick_secs: i64) {
        if active_uids.is_empty() || tick_secs <= 0 {
            return;
        }
        let by_uid: HashMap<u32, &String> = active_uids.iter().map(|(u, id)| (*id, u)).collect();
        let mut seen: HashSet<(String, &'static str)> = HashSet::new();
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return;
        };
        for entry in dir.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) else {
                continue;
            };
            let Some(app) = self.comm_index.get(comm.trim()).copied() else {
                continue;
            };
            let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
                continue;
            };
            let Some(uid) = status
                .lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|u| u.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(user) = by_uid.get(&uid) {
                if seen.insert(((*user).clone(), app)) {
                    self.bump(user, "app", app.to_string(), tick_secs);
                }
            }
        }
    }

    /// Tail the dnsmasq query log: one hit per query, keyed by registrable
    /// domain, attributed to the device. Handles rotation-by-truncation.
    pub fn ingest_dns_log(&mut self) {
        let Ok(mut f) = std::fs::File::open(DNSQ_LOG) else {
            return;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.log_offset {
            self.log_offset = 0; // someone rotated/truncated it
        }
        if len == self.log_offset {
            return;
        }
        if f.seek(SeekFrom::Start(self.log_offset)).is_err() {
            return;
        }
        let to_read = ((len - self.log_offset) as usize).min(READ_CAP);
        let mut buf = vec![0u8; to_read];
        let Ok(n) = f.read(&mut buf) else {
            return;
        };
        buf.truncate(n);
        self.log_offset += n as u64;
        let text = String::from_utf8_lossy(&buf);
        for line in text.lines() {
            if let Some(name) = queried_name(line) {
                let site = registrable(name);
                if !site.is_empty() && site.contains('.') {
                    self.bump("", "site", site, 1);
                }
            }
        }
        // Keep the log from eating the disk; dnsmasq appends, so this is safe.
        if len > TRUNCATE_AT {
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(DNSQ_LOG);
            self.log_offset = 0;
        }
    }

    /// Take up to `cap` slices for posting. Returns an empty vec when idle.
    pub fn drain(&mut self, cap: usize) -> Vec<Value> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let keys: Vec<SliceKey> = self.pending.keys().take(cap).cloned().collect();
        keys.into_iter()
            .filter_map(|k| {
                let amount = self.pending.remove(&k)?;
                Some(json!({
                    "os_username": k.user,
                    "hour": k.hour,
                    "kind": k.kind,
                    "key": k.key,
                    "amount": amount,
                }))
            })
            .collect()
    }

    /// Put a failed batch back (bounded by MAX_PENDING like everything else).
    pub fn requeue(&mut self, slices: Vec<Value>) {
        for s in slices {
            let (Some(user), Some(hour), Some(kind), Some(key), Some(amount)) = (
                s.get("os_username").and_then(Value::as_str),
                s.get("hour").and_then(Value::as_str),
                s.get("kind").and_then(Value::as_str),
                s.get("key").and_then(Value::as_str),
                s.get("amount").and_then(Value::as_i64),
            ) else {
                continue;
            };
            let kind = if kind == "app" { "app" } else { "site" };
            let k = SliceKey {
                user: user.to_string(),
                hour: hour.to_string(),
                kind,
                key: key.to_string(),
            };
            if self.pending.len() < MAX_PENDING || self.pending.contains_key(&k) {
                *self.pending.entry(k).or_insert(0) += amount;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrable_takes_the_sensible_tail() {
        assert_eq!(registrable("www.youtube.com"), "youtube.com");
        assert_eq!(registrable("m.media-amazon.com"), "media-amazon.com");
        assert_eq!(registrable("news.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(registrable("youtube.com."), "youtube.com");
        assert_eq!(registrable("localhost"), "localhost");
    }

    #[test]
    fn dnsmasq_extra_lines_parse() {
        let line = "Aug 27 14:12:33 dnsmasq[123]: 4711 127.0.0.1/5353 query[A] www.youtube.com from 127.0.0.1";
        assert_eq!(queried_name(line), Some("www.youtube.com"));
        assert_eq!(queried_name("Aug 27 dnsmasq[1]: reply youtube.com is 1.2.3.4"), None);
    }

    #[test]
    fn drain_then_requeue_round_trips() {
        let mut a = Attrib::new();
        a.bump("mia", "app", "discord".into(), 10);
        a.bump("", "site", "youtube.com".into(), 3);
        let batch = a.drain(500);
        assert_eq!(batch.len(), 2);
        assert!(a.drain(500).is_empty());
        a.requeue(batch);
        assert_eq!(a.drain(500).len(), 2);
    }
}
