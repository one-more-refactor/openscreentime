//! Screen-time enforcement: per-OS-user active-seat accounting via `loginctl`,
//! enforcing daily limit + allowed windows + bedtime. When a user's balance hits
//! zero the runner shows the lockout overlay, then this module freezes the user's
//! cgroup (freezer) or ends the session (TAMPER.md).

use crate::policy::{Bedtime, Policy, Window};
use crate::sysusers;
use crate::util::Exec;
use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Reboot-surviving usage ledger. Without this, a `systemctl restart` (crash,
/// watchdog kick, self-update — or a kid who guesses the trick) drops the
/// in-memory counters to zero and hands out a fresh daily budget. Root-owned
/// dir; the systemd unit already lists it under `ReadWritePaths`.
/// Persisted usage ledger. Losing this resets how much time a child has
/// already spent today, so it lives in the migrated state directory.
pub fn ledger_path() -> std::path::PathBuf {
    crate::paths::state("usage_ledger.json")
}

/// Why a user is being locked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockReason {
    DailyLimit { used_min: u32, limit_min: u32 },
    OutsideWindow,
    Bedtime,
}

impl LockReason {
    pub fn headline(&self) -> String {
        match self {
            LockReason::DailyLimit { .. } => "TIME'S UP".into(),
            LockReason::OutsideWindow => "NOT NOW".into(),
            LockReason::Bedtime => "BEDTIME".into(),
        }
    }
    pub fn detail(&self) -> String {
        match self {
            LockReason::DailyLimit {
                used_min,
                limit_min,
            } => {
                format!("USED {used_min} / {limit_min} MIN TODAY")
            }
            LockReason::OutsideWindow => "OUTSIDE ALLOWED HOURS".into(),
            LockReason::Bedtime => "SCREENS ARE OFF UNTIL MORNING".into(),
        }
    }
}

/// Accumulates active seconds per user, resetting at local midnight. `earned`
/// seconds (from approved earn-time tasks) extend the daily budget. Serialized to
/// [`ledger_path`] so it survives an agent restart.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UsageTracker {
    day: Option<NaiveDate>,
    used_secs: HashMap<String, u32>,
    earned_secs: HashMap<String, u32>,
}

impl UsageTracker {
    /// Fresh, empty tracker. The running agent uses [`load`](Self::load) instead
    /// so a restart resumes the day; kept for tests and callers that want a
    /// clean slate.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the persisted ledger, or a fresh one if it's missing/corrupt.
    pub fn load() -> Self {
        Self::load_from(&ledger_path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist the ledger (best-effort, atomic rename). Callers gate on dry-run.
    pub fn save(&self) {
        self.save_to(&ledger_path());
    }

    pub fn save_to(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(body) = serde_json::to_string(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, &body).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    /// Roll to a new day ONLY when the wall clock has genuinely advanced past
    /// the day we're accounting for. This is forward-only on purpose: setting
    /// the clock *backward* (to earlier today or to yesterday) used to make
    /// `self.day != today` and wipe the counters — an instant free-time cheat.
    /// Now a backward jump keeps the existing day and its accumulated usage;
    /// the clock jump itself is separately surfaced as a tamper event.
    fn roll_day(&mut self) {
        let today = Local::now().date_naive();
        let advanced = match self.day {
            Some(d) => today > d,
            None => true,
        };
        if advanced {
            self.day = Some(today);
            self.used_secs.clear();
            self.earned_secs.clear();
        }
    }

    /// Whether the accumulated counters still apply to the current wall-clock
    /// day. True when the clock has NOT advanced past the accounting day — this
    /// covers both the same-day case and a backward clock jump (a set-back must
    /// not zero the reported usage). False once the clock genuinely crosses into
    /// a later day but no seat user has been active yet to roll the counters, so
    /// readers report 0 for the new day rather than yesterday's stale totals.
    fn counters_current(&self) -> bool {
        match self.day {
            Some(d) => Local::now().date_naive() <= d,
            None => false,
        }
    }

    /// Add `real_secs` of wall time for `user`, scaled by the dev time-accel factor.
    pub fn add_active(&mut self, user: &str, real_secs: u32, accel: u32) {
        self.roll_day();
        *self.used_secs.entry(user.to_string()).or_insert(0) += real_secs.saturating_mul(accel);
    }

    /// Credit earned reward minutes to a user's daily budget. Called from the
    /// runner's `credit_time` command handler once an admin approves an
    /// earn-request (CONTRACT-PROD.md §4).
    pub fn add_earned(&mut self, user: &str, minutes: u32) {
        self.roll_day();
        *self.earned_secs.entry(user.to_string()).or_insert(0) += minutes.saturating_mul(60);
    }

    pub fn used_minutes(&self, user: &str) -> u32 {
        if !self.counters_current() {
            return 0;
        }
        self.used_secs.get(user).copied().unwrap_or(0) / 60
    }
    pub fn earned_minutes(&self, user: &str) -> u32 {
        if !self.counters_current() {
            return 0;
        }
        self.earned_secs.get(user).copied().unwrap_or(0) / 60
    }

    /// Effective remaining minutes given the policy limit (+ earned). None = unlimited.
    pub fn remaining_minutes(&self, user: &str, policy: &Policy) -> Option<i64> {
        if !policy.screen_time.enabled || policy.screen_time.daily_limit_minutes == 0 {
            return None;
        }
        let budget =
            policy.screen_time.daily_limit_minutes as i64 + self.earned_minutes(user) as i64;
        Some(budget - self.used_minutes(user) as i64)
    }
}

/// Evaluate whether `user` should be locked right now.
pub fn evaluate(policy: &Policy, tracker: &UsageTracker, user: &str) -> Option<LockReason> {
    let st = &policy.screen_time;
    if !st.enabled {
        return None;
    }
    let now = Local::now();
    let weekday_sun0 = now.weekday().num_days_from_sunday() as u8;
    let now_t = now.time();

    if let Some(bt) = &st.bedtime {
        if in_bedtime(bt, now_t) {
            return Some(LockReason::Bedtime);
        }
    }
    if !st.schedule.is_empty() && !within_any_window(&st.schedule, weekday_sun0, now_t) {
        return Some(LockReason::OutsideWindow);
    }
    if let Some(remaining) = tracker.remaining_minutes(user, policy) {
        if remaining <= 0 {
            let limit = st.daily_limit_minutes + tracker.earned_minutes(user);
            return Some(LockReason::DailyLimit {
                used_min: tracker.used_minutes(user),
                limit_min: limit,
            });
        }
    }
    None
}

fn parse_hm(s: &str) -> Option<NaiveTime> {
    let mut parts = s.split(':');
    let h: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    NaiveTime::from_hms_opt(h, m, 0)
}

/// Bedtime may wrap past midnight (e.g. 21:00 → 07:00).
pub fn in_bedtime(bt: &Bedtime, now: NaiveTime) -> bool {
    let (Some(start), Some(end)) = (parse_hm(&bt.start), parse_hm(&bt.end)) else {
        return false;
    };
    if start <= end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

/// Minutes until bedtime starts (handles start times past midnight relative to
/// `now`). `Some(0)` while bedtime is already in effect; `None` if the policy's
/// times don't parse. Used for the pre-bedtime wind-down nudge.
pub fn minutes_until_bedtime(bt: &Bedtime, now: NaiveTime) -> Option<i64> {
    let start = parse_hm(&bt.start)?;
    parse_hm(&bt.end)?; // both must parse for bedtime to be enforceable at all
    if in_bedtime(bt, now) {
        return Some(0);
    }
    let mut mins = (start - now).num_minutes();
    if mins < 0 {
        mins += 24 * 60;
    }
    Some(mins)
}

pub fn within_any_window(schedule: &[Window], weekday_sun0: u8, now: NaiveTime) -> bool {
    schedule.iter().any(|w| {
        if !w.days.contains(&weekday_sun0) {
            return false;
        }
        match (parse_hm(&w.start), parse_hm(&w.end)) {
            (Some(s), Some(e)) => now >= s && now < e,
            _ => false,
        }
    })
}

/// Users currently active on a local seat (loginctl). Empty on headless/no-logind.
///
/// Accounting deliberately does NOT consult the session's `IdleHint`: logind lets
/// a session's own owner set that hint (`SetIdleHint` on the session object), so
/// a managed user could mark themselves "idle" while actively using the machine
/// and never burn their daily budget. Screen time must not be gameable, so an
/// active, local (non-remote) session counts regardless of the self-reported
/// idle state.
pub fn active_seat_users(exec: &Exec) -> Vec<String> {
    let listing = exec.probe("loginctl", &["list-sessions", "--no-legend"]);
    let mut users = Vec::new();
    for line in listing.lines() {
        // columns: SESSION UID USER SEAT TTY  (seat present => local)
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 3 {
            continue;
        }
        let session = cols[0];
        let user = cols[2];
        let state = exec.probe(
            "loginctl",
            &["show-session", session, "-p", "Active", "-p", "Remote"],
        );
        let active = state.contains("Active=yes");
        let remote = state.contains("Remote=yes");
        if active && !remote && !users.contains(&user.to_string()) {
            users.push(user.to_string());
        }
    }
    users
}

/// What the kernel says about a user's freezer right now: `Some(true)` if
/// `cgroup.freeze` reads back 1, `Some(false)` if 0, `None` if the slice does
/// not exist (user not logged in) or cannot be read. This — never the agent's
/// intention — is what gets reported as the device's lock state.
pub fn is_frozen(username: &str) -> Option<bool> {
    let uid = sysusers::uid_of(username)?;
    let path = format!("/sys/fs/cgroup/user.slice/user-{uid}.slice/cgroup.freeze");
    std::fs::read_to_string(path).ok().map(|s| s.trim() == "1")
}

/// Freeze all processes of a user via the cgroup v2 freezer. Reversible.
///
/// `hard` controls the fallback when the freezer is unavailable: an admin
/// whole-device lock (`hard = true`) may terminate the session as a last
/// resort, but screen-time enforcement (`hard = false`) must NEVER destroy a
/// kid's unsaved work over a time limit — it logs and stays best-effort.
pub fn freeze_user(exec: &Exec, username: &str, frozen: bool, hard: bool) -> Result<()> {
    let Some(uid) = sysusers::uid_of(username) else {
        anyhow::bail!("unknown user {username}");
    };
    let path = format!("/sys/fs/cgroup/user.slice/user-{uid}.slice/cgroup.freeze");
    let val = if frozen { "1" } else { "0" };
    if exec.dry_run() {
        tracing::info!(target: "dry_run", "WOULD WRITE {} <- {} (freeze user {})", path, val, username);
        return Ok(());
    }
    match std::fs::write(&path, val) {
        Ok(_) => {
            tracing::info!("user {} freeze={}", username, frozen);
            Ok(())
        }
        Err(e) if frozen && hard => {
            tracing::warn!("cgroup freeze unavailable ({e}); admin lock falls back to loginctl");
            exec.run("loginctl", &["terminate-user", username])
                .map(|_| ())
        }
        Err(e) if frozen => {
            tracing::warn!(
                "cgroup freeze unavailable ({e}); screen-time lock NOT escalating to \
                 terminate-user (would destroy unsaved work)"
            );
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::ScreenTime;

    #[test]
    fn bedtime_wraps_midnight() {
        let bt = Bedtime {
            start: "21:00".into(),
            end: "07:00".into(),
        };
        assert!(in_bedtime(&bt, NaiveTime::from_hms_opt(23, 0, 0).unwrap()));
        assert!(in_bedtime(&bt, NaiveTime::from_hms_opt(3, 0, 0).unwrap()));
        assert!(!in_bedtime(&bt, NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
    }

    #[test]
    fn daily_limit_locks_when_exhausted() {
        let policy = Policy {
            screen_time: ScreenTime {
                enabled: true,
                daily_limit_minutes: 60,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut t = UsageTracker::new();
        t.add_active("kid", 61 * 60, 1);
        let r = evaluate(&policy, &t, "kid");
        assert!(matches!(r, Some(LockReason::DailyLimit { .. })));
    }

    #[test]
    fn minutes_until_bedtime_handles_wrap_and_in_effect() {
        let bt = Bedtime {
            start: "22:30".into(),
            end: "06:30".into(),
        };
        // 15 minutes out → wind-down window.
        assert_eq!(
            minutes_until_bedtime(&bt, NaiveTime::from_hms_opt(22, 15, 0).unwrap()),
            Some(15)
        );
        // Already in bedtime (both sides of midnight) → 0.
        assert_eq!(
            minutes_until_bedtime(&bt, NaiveTime::from_hms_opt(23, 0, 0).unwrap()),
            Some(0)
        );
        assert_eq!(
            minutes_until_bedtime(&bt, NaiveTime::from_hms_opt(3, 0, 0).unwrap()),
            Some(0)
        );
        // Morning, bedtime tonight → wraps forward, not negative.
        assert_eq!(
            minutes_until_bedtime(&bt, NaiveTime::from_hms_opt(7, 30, 0).unwrap()),
            Some(15 * 60)
        );
        // Unparseable policy times → None.
        let bad = Bedtime {
            start: "late".into(),
            end: "06:30".into(),
        };
        assert_eq!(
            minutes_until_bedtime(&bad, NaiveTime::from_hms_opt(12, 0, 0).unwrap()),
            None
        );
    }

    #[test]
    fn earned_time_extends_budget() {
        let policy = Policy {
            screen_time: ScreenTime {
                enabled: true,
                daily_limit_minutes: 60,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut t = UsageTracker::new();
        t.add_active("kid", 61 * 60, 1);
        t.add_earned("kid", 15);
        assert!(evaluate(&policy, &t, "kid").is_none());
    }

    #[test]
    fn ledger_survives_a_restart() {
        // A round-trip through disk must preserve the day's usage, so a restart
        // (crash / self-update / watchdog kick) can't hand out a fresh budget.
        let dir =
            std::env::temp_dir().join(format!("openscreentime-ledger-{}", std::process::id()));
        let path = dir.join("usage_ledger.json");
        let mut t = UsageTracker::new();
        t.add_active("kid", 40 * 60, 1);
        t.save_to(&path);

        let reloaded = UsageTracker::load_from(&path);
        assert_eq!(reloaded.used_minutes("kid"), 40);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clock_set_back_does_not_reset_usage() {
        // Simulate a full day's usage recorded for today, then a clock set back
        // to yesterday. roll_day is forward-only, so the counters (and the
        // reported minutes) must NOT reset — the set-back cheat is defused.
        let mut t = UsageTracker::new();
        t.add_active("kid", 55 * 60, 1);
        assert_eq!(t.used_minutes("kid"), 55);

        // Pretend the clock jumped backward: the accounting day is now in the
        // future relative to "today".
        t.day = Some(Local::now().date_naive() + chrono::Duration::days(1));
        t.add_active("kid", 60, 1); // a tick after the set-back
                                    // Still counted against the same budget, never wiped.
        assert!(t.used_minutes("kid") >= 55);
    }

    #[test]
    fn new_day_forward_gives_fresh_budget() {
        // The legitimate case: yesterday's totals must not bleed into today.
        let mut t = UsageTracker::new();
        t.day = Some(Local::now().date_naive() - chrono::Duration::days(1));
        t.used_secs.insert("kid".into(), 60 * 60);
        // Before any activity today, readers see 0 (stale yesterday ignored).
        assert_eq!(t.used_minutes("kid"), 0);
        // First active tick rolls the day and starts fresh: only the new time
        // counts, yesterday's hour is gone.
        t.add_active("kid", 2 * 60, 1);
        assert_eq!(t.used_minutes("kid"), 2);
    }
}
