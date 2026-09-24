//! The screen-time rules — ONE pure function every component asks.
//!
//! "May this person use a screen right now, and if so until when?" used to be
//! answered in three places that disagreed: the agent's `evaluate`, the
//! console's `limit + earned − used`, and the web's reading of the schedule.
//! This module is the single answer. The agent enforces with it, the server
//! computes "left" with it, and its semantics are the ones the web sends.
//!
//! ## Semantics (these are the product's promises, keep them)
//!
//! * **"Any time" means no restriction.** A day that has no allowed-hours
//!   window of its own is unrestricted. Only a day that *has* a window is
//!   limited to its windows. (It used to mean "locked all day".)
//! * **A window ending at `00:00` ends at midnight** (24:00), and
//!   `00:00 – 00:00` is the whole day.
//! * **A window whose end is before its start crosses midnight**: Friday
//!   `20:00 – 01:00` allows Friday 20:00–24:00 *and* Saturday 00:00–01:00. The
//!   tail never makes Saturday a restricted day on its own.
//! * **An empty or unreadable window is ignored — it never locks anyone out.**
//!   (`15:00 – 15:00`, `"late"`, days outside 0–6.) The server rejects these on
//!   save ([`validate_screen_time`]); the agent still refuses to turn one into
//!   a 24/7 lockout if an old profile carries it.
//! * **Bedtime** is every day, may cross midnight, and an end of `00:00` means
//!   midnight. A bedtime covering the whole day (`00:00 – 00:00`, `07:00 –
//!   07:00`) is invalid and ignored, for the same reason.
//! * **Daily limit** `0` (or screen time disabled) means *no limit*, never
//!   "0 left of 0". The budget is `limit + earned`; it resets at local midnight.
//! * **An override beats the limit, bedtime and allowed hours.** A parent's
//!   explicit grant wins. Time used during an override still counts as used.
//! * **A pause beats everything, including an override** — it is the later,
//!   stronger parent action. (A parent code typed at the device lifts the
//!   pause itself; it never needs to out-rank it here.)
//!
//! [`evaluate`] returns when the next stop lands and why, assuming the person
//! keeps using the screen from now on — so `minutes_left` already honours
//! bedtime, the end of the allowed window, the end of an override, and the
//! daily limit, whichever comes first.
//!
//! ## Focus hours (a self-managed person's own site blocks)
//!
//! [`focus_blocking`] answers "are my self-blocked sites blocked right now?".
//! The focus window reads exactly like an allowed-hours window (midnight,
//! crossing midnight, end exclusive). No hours = blocked all day; an empty or
//! unreadable window counts as no hours — the person asked for the block, the
//! hours only narrow it. A focus window never stops a screen.

use crate::{Focus, ScreenTime};
use chrono::{DateTime, Datelike, Duration, NaiveDateTime, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

/// How far ahead [`evaluate`] looks for the next stop. Past this, "no stop".
pub const HORIZON_HOURS: i64 = 48;

const DAY_MIN: u16 = 24 * 60;

/// Why a person is (or will be) stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The day's budget (limit + earned) is used up.
    Limit,
    /// Inside the bedtime window.
    Bedtime,
    /// The day has allowed hours and now is outside all of them.
    OutsideHours,
    /// A parent paused the device.
    Paused,
}

impl StopReason {
    pub fn id(&self) -> &'static str {
        match self {
            StopReason::Limit => "limit",
            StopReason::Bedtime => "bedtime",
            StopReason::OutsideHours => "outside_hours",
            StopReason::Paused => "paused",
        }
    }
}

/// The answer. All times are in the caller's time zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict<Tz: TimeZone> {
    /// May the person use the screen right now?
    pub allowed: bool,
    /// Why the stop at [`stop_at`](Self::stop_at) happens: the reason they are
    /// stopped now (when `!allowed`), or the reason of the next stop.
    pub reason: Option<StopReason>,
    /// When the (next) stop lands: `now` when stopped, the upcoming stop when
    /// allowed, `None` when nothing stops them within [`HORIZON_HOURS`].
    pub stop_at: Option<DateTime<Tz>>,
    /// Whole minutes until `stop_at`, rounded up (so "1 min" until the very
    /// end); `Some(0)` when stopped; `None` when there is no stop ahead.
    pub minutes_left: Option<u32>,
    /// When stopped: the first moment they could use the screen again without
    /// anyone doing anything (bedtime ends, the window opens, the budget
    /// resets at midnight). `None` when allowed, paused, or beyond the horizon.
    pub resume_at: Option<DateTime<Tz>>,
    /// An override is holding the rules off right now.
    pub override_active: bool,
    /// The daily budget left (limit + earned − used), in seconds, clamped at
    /// 0. `None` = no daily limit. This is the number the ring and the console
    /// show; `minutes_left` is when the screen actually stops.
    pub budget_left_secs: Option<u64>,
}

/// Everything [`evaluate`] needs about the person's day.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Day {
    /// Seconds of real use today, across every computer the person uses.
    pub used_secs: u64,
    /// Seconds granted on top of the daily limit today (earn-time, "+N min").
    pub earned_secs: u64,
}

/// The one rules function. Pure: no clock, no I/O.
///
/// `now` carries the local time zone; `override_until` is the end of an
/// active parent override (ignored when not in the future); `paused` is a
/// parent's pause of the device.
pub fn evaluate<Tz: TimeZone>(
    st: &ScreenTime,
    now: &DateTime<Tz>,
    day: Day,
    override_until: Option<&DateTime<Tz>>,
    paused: bool,
) -> Verdict<Tz> {
    let rules = Compiled::new(st);
    let limit_secs = rules.limit_secs();
    let budget_left_secs =
        limit_secs.map(|l| (l + day.earned_secs as i64 - day.used_secs as i64).max(0) as u64);
    let ov_end = override_until.filter(|u| *u > now).cloned();
    let override_active = ov_end.is_some();

    let stopped_now = |reason: StopReason, resume_at: Option<DateTime<Tz>>| Verdict {
        allowed: false,
        reason: Some(reason),
        stop_at: Some(now.clone()),
        minutes_left: Some(0),
        resume_at,
        override_active,
        budget_left_secs,
    };

    if paused {
        return stopped_now(StopReason::Paused, None);
    }

    // Walk forward minute by minute (every rule changes only on a minute
    // boundary), accumulating use as if the person keeps going, until
    // something stops them.
    let horizon = now.clone() + Duration::hours(HORIZON_HOURS);
    let mut t = now.clone();
    let mut used = day.used_secs as i64;
    let mut budget = limit_secs.map(|l| l + day.earned_secs as i64);
    loop {
        let ov = ov_end.as_ref().is_some_and(|e| t < *e);
        if !ov {
            let reason = rules
                .clock_reason(&t.naive_local())
                .or_else(|| budget.filter(|b| used >= *b).map(|_| StopReason::Limit));
            if let Some(reason) = reason {
                if t == *now {
                    return stopped_now(reason, rules.resume_at(now, day, limit_secs));
                }
                return Verdict {
                    allowed: true,
                    reason: Some(reason),
                    minutes_left: Some(minutes_until(now, &t)),
                    stop_at: Some(t),
                    resume_at: None,
                    override_active,
                    budget_left_secs,
                };
            }
        }
        if t >= horizon {
            break;
        }
        let mut next = next_minute(&t);
        if let Some(e) = ov_end.as_ref().filter(|e| **e > t && **e < next) {
            next = e.clone();
        }
        if next > horizon {
            next = horizon.clone();
        }
        // The budget can run out mid-minute.
        if let (false, Some(b)) = (ov, budget) {
            let at = t.clone() + Duration::seconds(b - used);
            if at < next {
                return Verdict {
                    allowed: true,
                    reason: Some(StopReason::Limit),
                    minutes_left: Some(minutes_until(now, &at)),
                    stop_at: Some(at),
                    resume_at: None,
                    override_active,
                    budget_left_secs,
                };
            }
        }
        used += (next.clone() - t.clone()).num_seconds();
        if next.date_naive() != t.date_naive() {
            // Local midnight: a fresh day, a fresh budget (earned time and
            // time used elsewhere belonged to yesterday).
            used = 0;
            budget = limit_secs;
        }
        t = next;
    }
    Verdict {
        allowed: true,
        reason: None,
        stop_at: None,
        minutes_left: None,
        resume_at: None,
        override_active,
        budget_left_secs,
    }
}

/// Whole minutes from `a` to `b`, rounded up.
fn minutes_until<Tz: TimeZone>(a: &DateTime<Tz>, b: &DateTime<Tz>) -> u32 {
    let secs = (b.clone() - a.clone()).num_seconds().max(0);
    u32::try_from((secs + 59) / 60).unwrap_or(u32::MAX)
}

/// The next whole minute strictly after `t`. Every real-world UTC offset is a
/// whole number of minutes, so local minute boundaries are UTC ones.
fn next_minute<Tz: TimeZone>(t: &DateTime<Tz>) -> DateTime<Tz> {
    let ts = t.timestamp();
    let next = ts - ts.rem_euclid(60) + 60;
    t.timezone()
        .timestamp_opt(next, 0)
        .single()
        .unwrap_or_else(|| t.clone() + Duration::seconds(60 - ts.rem_euclid(60)))
}

/// Parse `"HH:MM"` into minutes after midnight. `None` for anything else.
pub fn parse_hm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// `start`/`end` as a span in minutes, `end` in `1..=1440` (`00:00` = midnight
/// at the end of the day). `None` when unreadable or empty (`start == end`,
/// other than `00:00 – 00:00`, which is the whole day).
fn span(start: &str, end: &str) -> Option<(u16, u16)> {
    let s = parse_hm(start)?;
    let e = match parse_hm(end)? {
        0 => DAY_MIN,
        e => e,
    };
    (s != e).then_some((s, e))
}

/// A policy's clock rules, parsed once.
struct Compiled {
    enabled: bool,
    limit_min: u32,
    /// Per weekday (0 = Sunday): does the day have allowed hours at all?
    restricted: [bool; 7],
    /// Per weekday: allowed `[start, end)` spans in minutes, own windows plus
    /// the after-midnight tails of the previous day's crossing windows.
    allowed: [Vec<(u16, u16)>; 7],
    /// Bedtime as spans within one day (a crossing bedtime is two spans).
    bedtime: Vec<(u16, u16)>,
}

impl Compiled {
    fn new(st: &ScreenTime) -> Self {
        let mut restricted = [false; 7];
        let mut allowed: [Vec<(u16, u16)>; 7] = Default::default();
        for w in &st.schedule {
            let Some((s, e)) = span(&w.start, &w.end) else {
                continue; // an empty/unreadable window never locks anyone out
            };
            for &d in w.days.iter().filter(|d| **d < 7) {
                let d = d as usize;
                restricted[d] = true;
                if s < e {
                    allowed[d].push((s, e));
                } else {
                    allowed[d].push((s, DAY_MIN));
                    allowed[(d + 1) % 7].push((0, e));
                }
            }
        }
        let bedtime = match st.bedtime.as_ref().and_then(|b| span(&b.start, &b.end)) {
            // A whole-day bedtime is a 24/7 lockout — never honoured.
            Some((0, DAY_MIN)) | None => Vec::new(),
            Some((s, e)) if s < e => vec![(s, e)],
            Some((s, e)) => vec![(s, DAY_MIN), (0, e)],
        };
        Compiled {
            enabled: st.enabled,
            limit_min: st.daily_limit_minutes,
            restricted,
            allowed,
            bedtime,
        }
    }

    fn limit_secs(&self) -> Option<i64> {
        (self.enabled && self.limit_min > 0).then(|| i64::from(self.limit_min) * 60)
    }

    /// Bedtime or outside the allowed hours at this local time, if either.
    fn clock_reason(&self, t: &NaiveDateTime) -> Option<StopReason> {
        if !self.enabled {
            return None;
        }
        let m = (t.hour() * 60 + t.minute()) as u16;
        let inside = |spans: &[(u16, u16)]| spans.iter().any(|(s, e)| m >= *s && m < *e);
        if inside(&self.bedtime) {
            return Some(StopReason::Bedtime);
        }
        let d = t.weekday().num_days_from_sunday() as usize;
        if self.restricted[d] && !inside(&self.allowed[d]) {
            return Some(StopReason::OutsideHours);
        }
        None
    }

    /// First moment at or after `now` when a stopped person could use the
    /// screen again with no further use and no parent action.
    fn resume_at<Tz: TimeZone>(
        &self,
        now: &DateTime<Tz>,
        day: Day,
        limit_secs: Option<i64>,
    ) -> Option<DateTime<Tz>> {
        let horizon = now.clone() + Duration::hours(HORIZON_HOURS);
        let mut budget_spent =
            limit_secs.is_some_and(|l| day.used_secs as i64 >= l + day.earned_secs as i64);
        let mut t = next_minute(now);
        let mut prev = now.clone();
        while t <= horizon {
            if t.date_naive() != prev.date_naive() {
                budget_spent = false; // midnight: the budget resets
            }
            if !budget_spent && self.clock_reason(&t.naive_local()).is_none() {
                return Some(t);
            }
            prev = t.clone();
            t = next_minute(&t);
        }
        None
    }
}

/// Is a bedtime in effect at this local time? (Same semantics as `evaluate`.)
pub fn in_bedtime(st: &ScreenTime, t: &NaiveDateTime) -> bool {
    Compiled::new(&ScreenTime {
        enabled: true,
        ..st.clone()
    })
    .clock_reason(t)
        == Some(StopReason::Bedtime)
}

/// Are the person's self-blocked sites blocked at this local time?
///
/// False with no sites. True with no focus hours (all day, every day) or with
/// hours that can't mean anything. Otherwise true exactly inside the window,
/// read like an allowed-hours window: `00:00` as an end is midnight,
/// `00:00 – 00:00` is the whole day, an end before the start crosses midnight
/// (the tail belongs to the next weekday), and the end itself is outside.
pub fn focus_blocking(f: &Focus, t: &NaiveDateTime) -> bool {
    if f.sites.is_empty() {
        return false;
    }
    let Some(w) = f.hours.as_ref() else {
        return true;
    };
    let days: Vec<usize> = w
        .days
        .iter()
        .filter(|d| **d < 7)
        .map(|d| *d as usize)
        .collect();
    let Some((s, e)) = span(&w.start, &w.end).filter(|_| !days.is_empty()) else {
        return true; // unreadable hours narrow nothing
    };
    let m = (t.hour() * 60 + t.minute()) as u16;
    let today = t.weekday().num_days_from_sunday() as usize;
    let yesterday = (today + 6) % 7;
    if s < e {
        days.contains(&today) && m >= s && m < e
    } else {
        (days.contains(&today) && m >= s) || (days.contains(&yesterday) && m < e)
    }
}

/// Server-side validation of focus hours and sites, run on every save of a
/// person's own rules. Same window rules as allowed hours.
pub fn validate_focus(f: &Focus) -> Result<(), String> {
    if f.sites.len() > MAX_FOCUS_SITES {
        return Err(format!(
            "that's {} sites — keep it to {MAX_FOCUS_SITES} or fewer",
            f.sites.len()
        ));
    }
    if let Some(w) = &f.hours {
        let label = format!("focus hours {} – {}", w.start, w.end);
        if parse_hm(&w.start).is_none() || parse_hm(&w.end).is_none() {
            return Err(format!("{label}: times must be HH:MM, like 09:00"));
        }
        if w.days.is_empty() {
            return Err(format!("{label}: pick at least one day"));
        }
        if let Some(d) = w.days.iter().find(|d| **d > 6) {
            return Err(format!(
                "{label}: day {d} doesn't exist (0 = Sunday … 6 = Saturday)"
            ));
        }
        if span(&w.start, &w.end).is_none() {
            return Err(format!(
                "{label}: start and end are the same, so the window is empty. \
                 Pick real hours, or remove them to block the sites all day"
            ));
        }
    }
    Ok(())
}

/// How many sites a person may block for themselves.
pub const MAX_FOCUS_SITES: usize = 200;

/// Server-side validation of the screen-time rules, run on every save. The
/// agent never turns a bad rule into a lockout, but a parent should hear
/// "that window is empty" when they make it, not discover it later.
pub fn validate_screen_time(st: &ScreenTime) -> Result<(), String> {
    if st.daily_limit_minutes > u32::from(DAY_MIN) {
        return Err(format!(
            "the daily limit is {} minutes — a day only has {DAY_MIN}",
            st.daily_limit_minutes
        ));
    }
    for w in &st.schedule {
        let label = format!("allowed hours {} – {}", w.start, w.end);
        if parse_hm(&w.start).is_none() || parse_hm(&w.end).is_none() {
            return Err(format!("{label}: times must be HH:MM, like 07:30"));
        }
        if w.days.is_empty() {
            return Err(format!("{label}: pick at least one day"));
        }
        if let Some(d) = w.days.iter().find(|d| **d > 6) {
            return Err(format!(
                "{label}: day {d} doesn't exist (0 = Sunday … 6 = Saturday)"
            ));
        }
        if span(&w.start, &w.end).is_none() {
            return Err(format!(
                "{label}: start and end are the same, so the window is empty. \
                 Pick real hours, or remove the window to allow any time"
            ));
        }
    }
    if let Some(b) = &st.bedtime {
        let label = format!("bedtime {} – {}", b.start, b.end);
        if parse_hm(&b.start).is_none() || parse_hm(&b.end).is_none() {
            return Err(format!("{label}: times must be HH:MM, like 20:30"));
        }
        match span(&b.start, &b.end) {
            Some((0, DAY_MIN)) | None => {
                return Err(format!(
                    "{label}: that bedtime covers the whole day. Pick when it starts and \
                     when it ends, or remove it"
                ))
            }
            Some(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bedtime, Window};
    use chrono::{FixedOffset, NaiveDate};

    // 2026-09-21 is a Monday; 09-25 a Friday; 09-26 a Saturday; 09-27 a Sunday.
    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(2 * 3600)
            .unwrap()
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(y, mo, d)
                    .unwrap()
                    .and_hms_opt(h, mi, 0)
                    .unwrap(),
            )
            .unwrap()
    }
    fn mon(h: u32, m: u32) -> DateTime<FixedOffset> {
        at(2026, 9, 21, h, m)
    }
    fn win(days: &[u8], s: &str, e: &str) -> Window {
        Window {
            days: days.to_vec(),
            start: s.into(),
            end: e.into(),
        }
    }
    fn st(limit: u32, schedule: Vec<Window>, bedtime: Option<(&str, &str)>) -> ScreenTime {
        ScreenTime {
            enabled: true,
            daily_limit_minutes: limit,
            schedule,
            bedtime: bedtime.map(|(s, e)| Bedtime {
                start: s.into(),
                end: e.into(),
            }),
        }
    }
    fn used(min: u64) -> Day {
        Day {
            used_secs: min * 60,
            earned_secs: 0,
        }
    }
    fn eval(s: &ScreenTime, now: DateTime<FixedOffset>, day: Day) -> Verdict<FixedOffset> {
        evaluate(s, &now, day, None, false)
    }
    const WEEKDAYS: [u8; 5] = [1, 2, 3, 4, 5];
    const WEEKEND: [u8; 2] = [0, 6];

    #[test]
    fn no_rules_is_no_stop() {
        let v = eval(&st(0, vec![], None), mon(12, 0), used(500));
        assert!(v.allowed);
        assert_eq!((v.reason, v.stop_at, v.minutes_left), (None, None, None));
        assert_eq!(
            v.budget_left_secs, None,
            "limit 0 is no limit, never 0 of 0"
        );
        let off = ScreenTime {
            enabled: false,
            ..st(
                30,
                vec![win(&WEEKDAYS, "08:00", "09:00")],
                Some(("00:30", "23:30")),
            )
        };
        let v = eval(&off, mon(12, 0), used(500));
        assert!(v.allowed && v.stop_at.is_none());
    }

    #[test]
    fn any_time_means_unrestricted() {
        // Only school days have hours; the weekend is "any time" — it used to
        // be locked from Saturday 00:00 to Sunday 23:59.
        let s = st(0, vec![win(&WEEKDAYS, "15:00", "19:00")], None);
        let sat = at(2026, 9, 26, 11, 0);
        let v = eval(&s, sat, used(0));
        assert!(v.allowed);
        // …and the next stop is Monday 00:00 (Monday has hours, 00:00 is
        // outside them) — far, but real.
        assert_eq!(v.reason, Some(StopReason::OutsideHours));
        assert_eq!(v.stop_at, Some(at(2026, 9, 28, 0, 0)));
        // A school day outside its window is stopped, and resumes at 15:00.
        let v = eval(&s, mon(10, 0), used(0));
        assert!(!v.allowed);
        assert_eq!(v.reason, Some(StopReason::OutsideHours));
        assert_eq!(v.resume_at, Some(mon(15, 0)));
        assert_eq!(v.minutes_left, Some(0));
    }

    #[test]
    fn window_ending_at_midnight_runs_to_midnight() {
        let s = st(0, vec![win(&[1, 2, 3, 4, 5, 6, 0], "18:00", "00:00")], None);
        let v = eval(&s, mon(23, 30), used(0));
        assert!(v.allowed, "18:00–00:00 used to never match");
        assert_eq!(v.stop_at, Some(at(2026, 9, 22, 0, 0)));
        assert_eq!(v.minutes_left, Some(30));
        assert_eq!(v.reason, Some(StopReason::OutsideHours));
        // 00:00–00:00 is the whole day.
        let s = st(0, vec![win(&WEEKDAYS, "00:00", "00:00")], None);
        assert!(eval(&s, mon(3, 0), used(0)).allowed);
        assert!(eval(&s, mon(23, 59), used(0)).allowed);
    }

    #[test]
    fn window_crossing_midnight_works() {
        // Friday 20:00 – 01:00; Saturday has its own hours 09:00 – 21:00.
        let s = st(
            0,
            vec![win(&[5], "20:00", "01:00"), win(&[6], "09:00", "21:00")],
            None,
        );
        let fri = |h, m| at(2026, 9, 25, h, m);
        let sat = |h, m| at(2026, 9, 26, h, m);
        assert!(eval(&s, fri(22, 0), used(0)).allowed);
        let v = eval(&s, sat(0, 30), used(0));
        assert!(v.allowed, "the Friday window's tail runs into Saturday");
        assert_eq!(v.stop_at, Some(sat(1, 0)));
        let v = eval(&s, sat(2, 0), used(0));
        assert!(!v.allowed);
        assert_eq!(v.resume_at, Some(sat(9, 0)));
        // A tail alone never makes a day restricted: Thursday → Friday tail,
        // Friday has no hours of its own here → Friday is any time.
        let s = st(0, vec![win(&[4], "20:00", "01:00")], None);
        assert!(eval(&s, fri(15, 0), used(0)).allowed);
    }

    #[test]
    fn empty_or_invalid_window_never_locks_all_day() {
        for bad in [
            win(&WEEKDAYS, "15:00", "15:00"),
            win(&WEEKDAYS, "late", "19:00"),
            win(&WEEKDAYS, "25:00", "19:00"),
            win(&[9], "08:00", "09:00"),
        ] {
            let s = st(0, vec![bad.clone()], None);
            let v = eval(&s, mon(12, 0), used(0));
            assert!(v.allowed, "{bad:?} must not lock");
            assert!(
                validate_screen_time(&s).is_err(),
                "{bad:?} must not validate"
            );
        }
        // A whole-day bedtime is ignored too.
        for (bs, be) in [("00:00", "00:00"), ("07:00", "07:00")] {
            let s = st(0, vec![], Some((bs, be)));
            assert!(eval(&s, mon(12, 0), used(0)).allowed);
            assert!(validate_screen_time(&s).is_err());
        }
    }

    #[test]
    fn bedtime_crossing_midnight() {
        let s = st(0, vec![], Some(("21:00", "07:00")));
        let v = eval(&s, mon(20, 50), used(0));
        assert!(v.allowed);
        assert_eq!(v.reason, Some(StopReason::Bedtime));
        assert_eq!(v.stop_at, Some(mon(21, 0)));
        assert_eq!(v.minutes_left, Some(10));
        for (h, m) in [(21, 0), (23, 59), (0, 0), (3, 0), (6, 59)] {
            let v = eval(&s, mon(h, m), used(0));
            assert_eq!(v.reason, Some(StopReason::Bedtime), "{h}:{m}");
            assert!(!v.allowed);
        }
        let v = eval(&s, mon(23, 0), used(0));
        assert_eq!(v.resume_at, Some(at(2026, 9, 22, 7, 0)));
        assert!(eval(&s, mon(7, 0), used(0)).allowed);
        // Bedtime ending at 00:00 means midnight.
        let s = st(0, vec![], Some(("22:00", "00:00")));
        assert!(!eval(&s, mon(23, 0), used(0)).allowed);
        assert!(eval(&s, mon(0, 30), used(0)).allowed);
    }

    #[test]
    fn limit_versus_window_end_versus_bedtime_whichever_first() {
        // 60-minute limit, 30 used → 30 left by budget.
        let s = st(
            60,
            vec![win(&WEEKDAYS, "07:00", "20:00")],
            Some(("21:00", "07:00")),
        );
        // At 12:00 the budget runs out first (12:30).
        let v = eval(&s, mon(12, 0), used(30));
        assert_eq!(
            (v.reason, v.stop_at, v.minutes_left),
            (Some(StopReason::Limit), Some(mon(12, 30)), Some(30))
        );
        assert_eq!(v.budget_left_secs, Some(30 * 60));
        // At 19:45 the window closes first (20:00), 15 minutes, not 30.
        let v = eval(&s, mon(19, 45), used(30));
        assert_eq!(
            (v.reason, v.stop_at, v.minutes_left),
            (Some(StopReason::OutsideHours), Some(mon(20, 0)), Some(15))
        );
        // The ring's number is still the budget.
        assert_eq!(v.budget_left_secs, Some(30 * 60));
        // No window, bedtime first.
        let s = st(60, vec![], Some(("21:00", "07:00")));
        let v = eval(&s, mon(20, 50), used(30));
        assert_eq!(
            (v.reason, v.minutes_left),
            (Some(StopReason::Bedtime), Some(10))
        );
        // Budget out mid-minute: rounds up.
        let v = eval(
            &s,
            mon(12, 0),
            Day {
                used_secs: 59 * 60 + 30,
                earned_secs: 0,
            },
        );
        assert_eq!(v.minutes_left, Some(1));
        assert_eq!(v.stop_at, Some(mon(12, 0) + Duration::seconds(30)));
    }

    #[test]
    fn limit_exhausted_stops_and_resumes_at_midnight() {
        let s = st(60, vec![], None);
        let v = eval(&s, mon(15, 0), used(60));
        assert!(!v.allowed);
        assert_eq!(v.reason, Some(StopReason::Limit));
        assert_eq!(v.resume_at, Some(at(2026, 9, 22, 0, 0)));
        assert_eq!(v.budget_left_secs, Some(0));
        // Earned time extends the budget.
        let v = eval(
            &s,
            mon(15, 0),
            Day {
                used_secs: 60 * 60,
                earned_secs: 15 * 60,
            },
        );
        assert!(v.allowed);
        assert_eq!(v.minutes_left, Some(15));
        // The limit resumes after bedtime when bedtime covers midnight.
        let s = st(60, vec![], Some(("21:00", "07:00")));
        let v = eval(&s, mon(15, 0), used(60));
        assert_eq!(v.resume_at, Some(at(2026, 9, 22, 7, 0)));
    }

    #[test]
    fn a_fresh_day_resets_the_budget_in_the_forecast() {
        // 23:50, 10 min of budget left, no clock rules: the budget would run
        // out at 00:00 — but midnight gives a fresh 60 first. Continuous use
        // stops at 01:00 of the next day.
        let s = st(60, vec![], None);
        let v = eval(&s, mon(23, 50), used(50));
        assert_eq!(v.reason, Some(StopReason::Limit));
        assert_eq!(v.stop_at, Some(at(2026, 9, 22, 1, 0)));
        assert_eq!(v.minutes_left, Some(70));
    }

    #[test]
    fn override_beats_limit_bedtime_and_window() {
        let s = st(
            60,
            vec![win(&WEEKDAYS, "07:00", "20:00")],
            Some(("21:00", "07:00")),
        );
        for now in [mon(22, 0), mon(20, 30), mon(15, 0)] {
            let until = now + Duration::minutes(30);
            let v = evaluate(&s, &now, used(90), Some(&until), false);
            assert!(v.allowed, "override at {now}");
            assert!(v.override_active);
            // The stop comes exactly when the override ends.
            assert_eq!(v.stop_at, Some(until));
            assert_eq!(v.minutes_left, Some(30));
        }
        // A +N grant = earned N + override N: at the limit, both end together.
        let now = mon(15, 0);
        let until = now + Duration::minutes(15);
        let v = evaluate(
            &s,
            &now,
            Day {
                used_secs: 60 * 60,
                earned_secs: 15 * 60,
            },
            Some(&until),
            false,
        );
        assert_eq!(
            (v.reason, v.minutes_left),
            (Some(StopReason::Limit), Some(15))
        );
        // If they took a break, the leftover budget carries past the override.
        let v = evaluate(
            &s,
            &now,
            Day {
                used_secs: 50 * 60,
                earned_secs: 15 * 60,
            },
            Some(&until),
            false,
        );
        assert_eq!(v.minutes_left, Some(25));
    }

    #[test]
    fn override_expiry_and_pause() {
        let s = st(60, vec![], Some(("21:00", "07:00")));
        let now = mon(22, 0);
        // An expired override does nothing.
        let past = now - Duration::minutes(1);
        let v = evaluate(&s, &now, used(0), Some(&past), false);
        assert!(!v.allowed);
        assert!(!v.override_active);
        assert_eq!(v.reason, Some(StopReason::Bedtime));
        // A pause beats an override.
        let later = now + Duration::minutes(30);
        let v = evaluate(&s, &mon(12, 0), used(0), Some(&later), true);
        assert!(!v.allowed);
        assert_eq!(v.reason, Some(StopReason::Paused));
        assert_eq!(v.resume_at, None);
    }

    #[test]
    fn presets_are_valid_and_teen_window_ends_before_bedtime_honestly() {
        // The teen preset: weekday window to 21:00, bedtime 22:00. At 20:50 the
        // stop is the window end, 10 minutes — not "70 min left".
        let s = st(
            150,
            vec![
                win(&WEEKDAYS, "07:00", "21:00"),
                win(&WEEKEND, "08:00", "22:00"),
            ],
            Some(("22:00", "06:30")),
        );
        assert!(validate_screen_time(&s).is_ok());
        let v = eval(&s, mon(20, 50), used(20));
        assert_eq!(
            (v.reason, v.minutes_left),
            (Some(StopReason::OutsideHours), Some(10))
        );
    }

    #[test]
    fn validation_messages_are_plain() {
        let e =
            validate_screen_time(&st(0, vec![win(&WEEKDAYS, "15:00", "15:00")], None)).unwrap_err();
        assert!(e.contains("empty"), "{e}");
        let e = validate_screen_time(&st(0, vec![win(&[], "15:00", "16:00")], None)).unwrap_err();
        assert!(e.contains("day"), "{e}");
        assert!(validate_screen_time(&st(2000, vec![], None)).is_err());
        assert!(validate_screen_time(&st(
            60,
            vec![
                win(&WEEKDAYS, "20:00", "01:00"),
                win(&WEEKEND, "18:00", "00:00")
            ],
            Some(("22:00", "00:00"))
        ))
        .is_ok());
    }

    /// The web editor checks the same vectors (web/src/lib/schedule.test.ts).
    #[test]
    fn shared_schedule_vectors_agree() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../tests/schedule-vectors.json")).unwrap();
        for w in v["windows"].as_array().unwrap() {
            let (s, e) = (w["start"].as_str().unwrap(), w["end"].as_str().unwrap());
            let rules = st(0, vec![win(&WEEKDAYS, s, e)], None);
            assert_eq!(
                validate_screen_time(&rules).is_ok(),
                w["valid"].as_bool().unwrap(),
                "window {s}–{e}"
            );
        }
        for b in v["bedtimes"].as_array().unwrap() {
            let (s, e) = (b["start"].as_str().unwrap(), b["end"].as_str().unwrap());
            let rules = st(0, vec![], Some((s, e)));
            assert_eq!(
                validate_screen_time(&rules).is_ok(),
                b["valid"].as_bool().unwrap(),
                "bedtime {s}–{e}"
            );
        }
        // Focus hours: the same windows, read as "are my sites blocked now?".
        // A window valid for allowed hours is valid for focus hours too.
        for w in v["windows"].as_array().unwrap() {
            let f = Focus {
                sites: vec!["x.org".into()],
                hours: Some(win(
                    &WEEKDAYS,
                    w["start"].as_str().unwrap(),
                    w["end"].as_str().unwrap(),
                )),
            };
            assert_eq!(validate_focus(&f).is_ok(), w["valid"].as_bool().unwrap());
        }
        let focus = v["focus"].as_array().unwrap();
        assert!(!focus.is_empty());
        for row in focus {
            let hours: Option<Window> = serde_json::from_value(row["hours"].clone()).unwrap();
            let f = Focus {
                sites: vec!["reddit.com".into()],
                hours,
            };
            // 2026-09-20 is a Sunday: day 0 → the 20th, day 6 → the 26th.
            let day = row["at"]["day"].as_u64().unwrap() as u32;
            let (h, m) = row["at"]["time"].as_str().unwrap().split_once(':').unwrap();
            let t = NaiveDate::from_ymd_opt(2026, 9, 20 + day)
                .unwrap()
                .and_hms_opt(h.parse().unwrap(), m.parse().unwrap(), 0)
                .unwrap();
            assert_eq!(
                focus_blocking(&f, &t),
                row["blocking"].as_bool().unwrap(),
                "focus {row}"
            );
        }
    }

    #[test]
    fn focus_blocks_only_what_was_asked_for() {
        let t = mon(10, 0).naive_local();
        // No sites: nothing to block, whatever the hours.
        let none = Focus {
            sites: vec![],
            hours: None,
        };
        assert!(!focus_blocking(&none, &t));
        // Unreadable or empty hours never unblock the sites.
        for (s, e) in [("late", "12:00"), ("15:00", "15:00")] {
            let f = Focus {
                sites: vec!["x.org".into()],
                hours: Some(win(&WEEKDAYS, s, e)),
            };
            assert!(focus_blocking(&f, &mon(20, 0).naive_local()), "{s}–{e}");
            assert!(validate_focus(&f).is_err(), "{s}–{e}");
        }
        let f = Focus {
            sites: vec!["x.org".into()],
            hours: Some(win(&[], "09:00", "12:00")),
        };
        assert!(validate_focus(&f).unwrap_err().contains("day"));
        let many = Focus {
            sites: (0..=MAX_FOCUS_SITES).map(|i| format!("s{i}.org")).collect(),
            hours: None,
        };
        assert!(validate_focus(&many).is_err());
        // A focus window never stops a screen: evaluate ignores it.
        assert!(eval(&st(0, vec![], None), mon(10, 0), used(0)).allowed);
    }

    #[test]
    fn dst_day_is_counted_in_real_minutes() {
        // In a zone that springs forward, "minutes_left" is real time. Use a
        // fixed zone for the mechanics; the agent passes chrono::Local.
        let s = st(0, vec![], Some(("21:00", "07:00")));
        let v = evaluate(&s, &mon(20, 0), used(0), None, false);
        assert_eq!(v.minutes_left, Some(60));
        assert!(in_bedtime(&s, &mon(22, 0).naive_local()));
        assert!(!in_bedtime(&s, &mon(12, 0).naive_local()));
    }
}
