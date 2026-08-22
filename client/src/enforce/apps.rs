//! App blocks, the native half: a blocked app's *process* is denied for the
//! user whose policy blocks it (docs/CONTRACT-0.4.md §7). The DNS half —
//! sinkholing the app's domains — lives in `dns.rs` via the same catalog.
//!
//! Every 10 s tick the runner calls [`deny`] with the per-user policies. We
//! walk `/proc`, read each process's `comm` (the first 15 bytes of its
//! executable name) and real uid, and SIGKILL anything whose comm is on the
//! expanded block list of the user that owns it. One `app_blocked` event per
//! user/app/day, so a launcher that keeps retrying does not flood the feed.
//!
//! Scope: exact `comm` matches from the catalog only — never a substring,
//! never a generic runtime (`java`, `python3`), never another user's
//! processes, never root's.

use crate::policy::Policy;
use crate::protocol::{Event, EV_APP_BLOCKED, SEV_INFO};
use crate::util::Exec;
use openscreentime_policy::catalog;
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// A running process that matched a blocked app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub pid: u32,
    pub uid: u32,
    pub comm: String,
    pub user: String,
    pub app: String,
}

/// comm → app id, for one user's policy.
fn comm_index(policy: &Policy) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let expanded = catalog::expand(&policy.blocks);
    for app_id in &expanded.apps {
        if let Some(app) = catalog::app(app_id) {
            for p in app.processes {
                out.insert((*p).to_string(), app.id.to_string());
            }
        }
    }
    out
}

/// Read `(uid, comm)` for a pid from /proc. `None` if it vanished or is unreadable.
fn proc_identity(pid: u32) -> Option<(u32, String)> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let uid = status
        .lines()
        .find(|l| l.starts_with("Uid:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|u| u.parse::<u32>().ok())?;
    Some((uid, comm.trim().to_string()))
}

/// Pure matching, for tests: which of `procs` (pid, uid, comm) belong to a
/// user whose policy blocks that comm.
pub fn matches(
    procs: &[(u32, u32, String)],
    users: &[(String, u32, &Policy)],
) -> Vec<Hit> {
    let by_uid: HashMap<u32, (&str, HashMap<String, String>)> = users
        .iter()
        .map(|(name, uid, p)| (*uid, (name.as_str(), comm_index(p))))
        .collect();
    let mut hits = Vec::new();
    for (pid, uid, comm) in procs {
        if *uid == 0 {
            continue;
        }
        if let Some((user, index)) = by_uid.get(uid) {
            if let Some(app) = index.get(comm) {
                hits.push(Hit {
                    pid: *pid,
                    uid: *uid,
                    comm: comm.clone(),
                    user: (*user).to_string(),
                    app: app.clone(),
                });
            }
        }
    }
    hits
}

fn scan_procs() -> Vec<(u32, u32, String)> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
        .filter_map(|pid| proc_identity(pid).map(|(uid, comm)| (pid, uid, comm)))
        .collect()
}

/// Deny blocked apps for every managed user this tick. `reported` is the
/// per-(user, app) daily dedupe the runner keeps. Returns the events to emit.
pub fn deny(
    exec: &Exec,
    policies: &HashMap<String, Policy>,
    reported: &mut HashMap<(String, String), chrono::NaiveDate>,
) -> Vec<Event> {
    // Fast exit: nothing blocks a native client → no /proc walk at all.
    let any = policies
        .values()
        .any(|p| !catalog::expand(&p.blocks).processes.is_empty());
    if !any {
        return Vec::new();
    }
    let users: Vec<(String, u32, &Policy)> = policies
        .iter()
        .filter_map(|(u, p)| crate::sysusers::uid_of(u).map(|uid| (u.clone(), uid, p)))
        .collect();
    let hits = matches(&scan_procs(), &users);
    let today = chrono::Local::now().date_naive();
    let mut events = Vec::new();
    let mut seen_pids = HashSet::new();
    for h in hits {
        if !seen_pids.insert(h.pid) {
            continue;
        }
        if exec.dry_run() {
            tracing::info!(target: "dry_run", "WOULD KILL pid {} ({}) of {} — app {} is blocked", h.pid, h.comm, h.user, h.app);
        } else {
            // SIGKILL, not SIGTERM: a launcher that catches TERM and relaunches
            // would otherwise turn this into a 10-second flicker loop.
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(h.pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
            tracing::info!("blocked app {} ({}) killed for {}", h.app, h.comm, h.user);
        }
        let key = (h.user.clone(), h.app.clone());
        if reported.get(&key) != Some(&today) {
            reported.insert(key, today);
            events.push(
                Event::new(
                    EV_APP_BLOCKED,
                    SEV_INFO,
                    json!({ "app": h.app, "comm": h.comm, "user": h.user }),
                )
                .for_user(h.user.clone()),
            );
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::AppBlocks;

    fn blocking(apps: &[&str], cats: &[&str]) -> Policy {
        Policy {
            blocks: AppBlocks {
                apps: apps.iter().map(|s| s.to_string()).collect(),
                categories: cats.iter().map(|s| s.to_string()).collect(),
                custom_domains: vec![],
            },
            ..Default::default()
        }
    }

    #[test]
    fn only_the_blocking_users_processes_match_and_never_root() {
        let kid = blocking(&["discord"], &[]);
        let teen = blocking(&[], &["games"]);
        let adult = Policy::default();
        let users = vec![
            ("kid".to_string(), 1001u32, &kid),
            ("teen".to_string(), 1002u32, &teen),
            ("dad".to_string(), 1000u32, &adult),
        ];
        let procs = vec![
            (10, 1001, "Discord".to_string()),   // kid runs Discord → hit
            (11, 1002, "Discord".to_string()),   // teen: discord not blocked for them
            (12, 1002, "steam".to_string()),     // teen: games category → steam → hit
            (13, 1000, "steam".to_string()),     // dad: nothing blocked
            (14, 0, "Discord".to_string()),      // root is never touched
            (15, 1001, "discord-helper".to_string()), // not an exact comm match
        ];
        let hits = matches(&procs, &users);
        let pids: Vec<u32> = hits.iter().map(|h| h.pid).collect();
        assert_eq!(pids, vec![10, 12]);
        assert_eq!(hits[0].app, "discord");
        assert_eq!(hits[1].app, "steam");
        assert_eq!(hits[1].user, "teen");
    }

    #[test]
    fn comm_index_comes_from_the_catalog() {
        let p = blocking(&["telegram"], &[]);
        let idx = comm_index(&p);
        assert_eq!(idx.get("telegram-desktop").map(String::as_str), Some("telegram"));
        assert!(idx.get("java").is_none());
    }
}
