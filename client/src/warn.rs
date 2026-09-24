//! Warnings before a stop: when they fire, and what they say.
//!
//! Every stop — the daily limit, bedtime, the end of allowed hours, a parent's
//! scheduled pause — is announced at 15, 5 and 1 minute before it lands, so the
//! lock never arrives "randomly". The per-user companion shows these as desktop
//! notifications; the agent writes the same words to the terminals of someone
//! with no desktop. Both use this module, so they can't disagree.
//!
//! [`forecast`] is a stand-in for the decision layer's own `stop_at`/`reason`:
//! the lock only needs *when* and *why*, and it reads whatever the status
//! snapshot publishes.

use crate::policy::{Bedtime, Policy, Window};
use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};

/// The minutes-before-a-stop that get a warning.
pub const THRESHOLDS: [u32; 3] = [15, 5, 1];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Limit,
    Bedtime,
    Window,
    Pause,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Limit => "limit",
            StopReason::Bedtime => "bedtime",
            StopReason::Window => "window",
            StopReason::Pause => "pause",
        }
    }
    #[cfg_attr(not(feature = "tray"), allow(dead_code))] // the companion reads it back
    pub fn parse(s: &str) -> Option<StopReason> {
        Some(match s {
            "limit" | "daily_limit" => StopReason::Limit,
            "bedtime" => StopReason::Bedtime,
            "window" | "outside_window" | "schedule" => StopReason::Window,
            "pause" | "paused" => StopReason::Pause,
            _ => return None,
        })
    }
}

/// The next stop for someone who is within their rules right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Forecast {
    pub reason: StopReason,
    pub at: DateTime<Local>,
}

fn hm(s: &str) -> Option<NaiveTime> {
    let (h, m) = s.trim().split_once(':')?;
    NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)
}

fn at_local(day: chrono::NaiveDate, t: NaiveTime) -> Option<DateTime<Local>> {
    Local.from_local_datetime(&day.and_time(t)).earliest()
}

/// When bedtime next starts (not while it's already on).
fn next_bedtime(bt: &Bedtime, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let start = hm(&bt.start)?;
    hm(&bt.end)?;
    if crate::enforce::screentime::in_bedtime(bt, now.time()) {
        return None;
    }
    let day = if start > now.time() {
        now.date_naive()
    } else {
        now.date_naive() + Duration::days(1)
    };
    at_local(day, start)
}

/// When the allowed window we're in ends — windows that touch count as one.
fn window_end(schedule: &[Window], now: DateTime<Local>) -> Option<DateTime<Local>> {
    let wd = now.weekday().num_days_from_sunday() as u8;
    let today: Vec<(NaiveTime, NaiveTime)> = schedule
        .iter()
        .filter(|w| w.days.contains(&wd))
        .filter_map(|w| Some((hm(&w.start)?, hm(&w.end)?)))
        .collect();
    let t = now.time();
    let mut end = today
        .iter()
        .filter(|(s, e)| t >= *s && t < *e)
        .map(|(_, e)| *e)
        .max()?;
    while let Some(later) = today
        .iter()
        .filter(|(s, e)| *s <= end && *e > end)
        .map(|(_, e)| *e)
        .max()
    {
        end = later;
    }
    at_local(now.date_naive(), end)
}

/// The earliest of: the daily limit running out, bedtime starting, the
/// allowed window ending. `limit_left` is the wall-clock time until the daily
/// limit runs out (`None` = no limit).
pub fn forecast(
    policy: &Policy,
    limit_left: Option<Duration>,
    now: DateTime<Local>,
) -> Option<Forecast> {
    let st = &policy.screen_time;
    if !st.enabled {
        return None;
    }
    let mut next: Vec<Forecast> = Vec::new();
    if let Some(r) = limit_left.filter(|r| *r > Duration::zero()) {
        next.push(Forecast {
            reason: StopReason::Limit,
            at: now + r,
        });
    }
    if let Some(at) = st.bedtime.as_ref().and_then(|b| next_bedtime(b, now)) {
        next.push(Forecast {
            reason: StopReason::Bedtime,
            at,
        });
    }
    if !st.schedule.is_empty() {
        if let Some(at) = window_end(&st.schedule, now) {
            next.push(Forecast {
                reason: StopReason::Window,
                at,
            });
        }
    }
    next.into_iter().min_by_key(|f| f.at)
}

/// When allowed hours next begin, said the way a person would: "15:00",
/// "tomorrow at 15:00", "Monday at 15:00". `None` if no window is coming.
pub fn next_allowed(schedule: &[Window], now: DateTime<Local>) -> Option<String> {
    for ahead in 0..8i64 {
        let day = now.date_naive() + Duration::days(ahead);
        let wd = day.weekday().num_days_from_sunday() as u8;
        let start = schedule
            .iter()
            .filter(|w| w.days.contains(&wd))
            .filter_map(|w| Some((hm(&w.start)?, hm(&w.end)?)))
            .filter(|(s, e)| s < e && (ahead > 0 || *s > now.time()))
            .map(|(s, _)| s)
            .min();
        if let Some(s) = start {
            let t = s.format("%H:%M").to_string();
            return Some(match ahead {
                0 => t,
                1 => format!("tomorrow at {t}"),
                _ => format!("{} at {t}", day.format("%A")),
            });
        }
    }
    None
}

/// Which thresholds have already been announced for the stop coming up.
#[derive(Debug, Default, Clone)]
pub struct WarnState {
    reason: Option<StopReason>,
    fired: Vec<u32>,
}

impl WarnState {
    /// Feed the seconds left before the stop; returns the threshold to
    /// announce now, if one is due. One notice per crossing: someone who logs
    /// in with 4 minutes left hears about it once, not "15" then "5".
    pub fn observe(&mut self, reason: StopReason, secs_left: i64) -> Option<u32> {
        if self.reason != Some(reason) {
            self.reason = Some(reason);
            self.fired.clear();
        }
        // Time was added (a grant, a changed rule): what's comfortably ahead
        // again gets announced again.
        self.fired.retain(|t| secs_left <= i64::from(*t) * 60 + 60);
        if secs_left <= 0 {
            return None;
        }
        let due = THRESHOLDS
            .iter()
            .copied()
            .filter(|t| secs_left <= i64::from(*t) * 60)
            .min()?;
        if self.fired.contains(&due) {
            return None;
        }
        for t in THRESHOLDS {
            if t >= due && !self.fired.contains(&t) {
                self.fired.push(t);
            }
        }
        Some(due)
    }

    /// Nothing is coming (or it already happened): forget.
    pub fn clear(&mut self) {
        self.reason = None;
        self.fired.clear();
    }
}

/// A warning's words: a title that says how long, a body that says what and
/// when. Plain sentence case; no shouting.
#[derive(Debug, Clone, PartialEq)]
pub struct Words {
    pub title: String,
    pub body: String,
    /// The last minute: shown as one critical notification, updated in place.
    pub critical: bool,
}

pub fn words(reason: StopReason, secs_left: i64, at: Option<DateTime<Local>>) -> Words {
    let mins = ((secs_left.max(1) + 59) / 60).max(1);
    let title = if secs_left <= 45 {
        "Less than a minute left — save your work".to_string()
    } else if mins == 1 {
        "1 minute left — save your work".to_string()
    } else if mins <= 5 {
        format!("{mins} minutes left — save your work")
    } else {
        format!("{mins} minutes left")
    };
    let when = at
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_else(|| "soon".into());
    let body = match reason {
        StopReason::Limit => format!("Today's screen time ends at {when}."),
        StopReason::Bedtime => format!("Bedtime starts at {when}."),
        StopReason::Window => format!("Allowed hours end at {when}."),
        StopReason::Pause => format!("A parent is pausing this computer at {when}."),
    };
    Words {
        title,
        body,
        critical: mins <= 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::ScreenTime;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        // A Wednesday, so weekday windows are predictable (Sun = 0 → Wed = 3).
        let d = chrono::NaiveDate::from_ymd_opt(2026, 9, 23).unwrap();
        at_local(d, NaiveTime::from_hms_opt(h, m, 0).unwrap()).unwrap()
    }

    fn policy() -> Policy {
        Policy {
            screen_time: ScreenTime {
                enabled: true,
                daily_limit_minutes: 60,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Walk the minutes up to a stop and collect what gets announced.
    fn announced(reason: StopReason, from_min: i64) -> Vec<u32> {
        let mut st = WarnState::default();
        let mut out = Vec::new();
        let mut secs = from_min * 60;
        while secs > -30 {
            if let Some(t) = st.observe(reason, secs) {
                out.push(t);
            }
            secs -= 10; // the tray polls every 5 s, the agent ticks every 10 s
        }
        out
    }

    #[test]
    fn every_stop_reason_is_announced_at_15_5_and_1() {
        for r in [
            StopReason::Limit,
            StopReason::Bedtime,
            StopReason::Window,
            StopReason::Pause,
        ] {
            assert_eq!(announced(r, 40), vec![15, 5, 1], "{r:?}");
            let w = words(r, 60, Some(at(21, 0)));
            assert!(w.critical, "the last minute is critical for {r:?}");
            assert!(w.body.contains("21:00"));
            assert_ne!(w.title, w.title.to_uppercase(), "no shouting");
        }
        assert_eq!(
            words(StopReason::Limit, 5 * 60, None).title,
            "5 minutes left — save your work"
        );
        assert!(!words(StopReason::Limit, 15 * 60, None).critical);
    }

    #[test]
    fn a_late_arrival_hears_once_and_time_added_rearms() {
        // Logged in with 4 minutes left: one notice (the 5), then the 1.
        assert_eq!(announced(StopReason::Limit, 4), vec![5, 1]);
        let mut st = WarnState::default();
        assert_eq!(st.observe(StopReason::Limit, 14 * 60), Some(15));
        assert_eq!(st.observe(StopReason::Limit, 13 * 60), None);
        // A parent adds 30 minutes: back above every threshold, all re-armed.
        assert_eq!(st.observe(StopReason::Limit, 43 * 60), None);
        assert_eq!(st.observe(StopReason::Limit, 15 * 60), Some(15));
        // A different stop coming up starts fresh.
        assert_eq!(st.observe(StopReason::Bedtime, 14 * 60), Some(15));
    }

    #[test]
    fn forecast_picks_the_earliest_stop() {
        let mut p = policy();
        // Limit only.
        let f = forecast(&p, Some(Duration::minutes(20)), at(16, 0)).unwrap();
        assert_eq!((f.reason, f.at), (StopReason::Limit, at(16, 20)));
        // Bedtime sooner than the limit.
        p.screen_time.bedtime = Some(Bedtime {
            start: "16:10".into(),
            end: "07:00".into(),
        });
        let f = forecast(&p, Some(Duration::minutes(20)), at(16, 0)).unwrap();
        assert_eq!((f.reason, f.at), (StopReason::Bedtime, at(16, 10)));
        // Inside touching windows 15:00–16:05 + 16:05–17:00: ends at 17:00, and
        // a window end sooner than bedtime wins.
        p.screen_time.bedtime = Some(Bedtime {
            start: "21:00".into(),
            end: "07:00".into(),
        });
        p.screen_time.schedule = vec![
            Window {
                days: vec![3],
                start: "15:00".into(),
                end: "16:05".into(),
            },
            Window {
                days: vec![3],
                start: "16:05".into(),
                end: "17:00".into(),
            },
        ];
        let f = forecast(&p, None, at(16, 0)).unwrap();
        assert_eq!((f.reason, f.at), (StopReason::Window, at(17, 0)));
        // Already in bedtime / no rules: nothing to forecast.
        p.screen_time.schedule.clear();
        assert!(forecast(&p, None, at(22, 0)).is_none());
        p.screen_time.enabled = false;
        assert!(forecast(&p, Some(Duration::minutes(5)), at(16, 0)).is_none());
    }

    #[test]
    fn next_allowed_hours_read_like_speech() {
        let sched = vec![
            Window {
                days: vec![3],
                start: "15:00".into(),
                end: "17:00".into(),
            },
            Window {
                days: vec![4],
                start: "09:00".into(),
                end: "10:00".into(),
            },
            Window {
                days: vec![1],
                start: "08:00".into(),
                end: "09:00".into(),
            },
        ];
        assert_eq!(next_allowed(&sched, at(12, 0)).as_deref(), Some("15:00"));
        assert_eq!(
            next_allowed(&sched, at(18, 0)).as_deref(),
            Some("tomorrow at 09:00")
        );
        let fri = at(18, 0) + Duration::days(2);
        assert_eq!(
            next_allowed(&sched, fri).as_deref(),
            Some("Monday at 08:00")
        );
        assert_eq!(next_allowed(&[], at(12, 0)), None);
    }
}
