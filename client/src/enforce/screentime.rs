//! Screen-time enforcement: per-OS-user active-seat accounting via `loginctl`,
//! enforcing daily limit + allowed windows + bedtime. When a user's balance hits
//! zero the runner shows the lockout overlay, then this module freezes the user's
//! cgroup (freezer) or ends the session (TAMPER.md).

use crate::policy::{Bedtime, Policy, Window};
use crate::sysusers;
use crate::util::Exec;
use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate, NaiveTime};
use std::collections::HashMap;

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
/// seconds (from the gamify tasks) extend the daily budget.
#[derive(Debug, Default)]
pub struct UsageTracker {
    day: Option<NaiveDate>,
    used_secs: HashMap<String, u32>,
    earned_secs: HashMap<String, u32>,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn roll_day(&mut self) {
        let today = Local::now().date_naive();
        if self.day != Some(today) {
            self.day = Some(today);
            self.used_secs.clear();
            self.earned_secs.clear();
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

    /// True when the tracker's accumulated day is still today. The counters only
    /// roll forward when a seat user is active (`add_active`), so on an idle
    /// machine past local midnight the maps hold yesterday's totals; readers must
    /// treat those as zero rather than report them against the new day.
    fn is_today(&self) -> bool {
        self.day == Some(Local::now().date_naive())
    }

    pub fn used_minutes(&self, user: &str) -> u32 {
        if !self.is_today() {
            return 0;
        }
        self.used_secs.get(user).copied().unwrap_or(0) / 60
    }
    pub fn earned_minutes(&self, user: &str) -> u32 {
        if !self.is_today() {
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

/// Freeze all processes of a user via the cgroup v2 freezer. Reversible.
pub fn freeze_user(exec: &Exec, username: &str, frozen: bool) -> Result<()> {
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
        Err(e) => {
            tracing::warn!("cgroup freeze unavailable ({e}); falling back to loginctl");
            if frozen {
                exec.run("loginctl", &["terminate-user", username])
                    .map(|_| ())
            } else {
                Ok(())
            }
        }
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
}
