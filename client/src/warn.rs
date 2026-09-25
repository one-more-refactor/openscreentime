//! Warnings before a stop: when they fire, and what they say.
//!
//! Every stop — the daily limit, bedtime, the end of allowed hours, a parent's
//! scheduled pause — is announced at 15, 5 and 1 minute before it lands, so the
//! lock never arrives "randomly". The per-user companion shows these as desktop
//! notifications; the agent writes the same words to the terminals of someone
//! with no desktop. Both use this module, so they can't disagree. *When* and
//! *why* come from the rules function's verdict (`stop_at`, `reason` in the
//! status snapshot); this module only decides when to speak and what to say.

use chrono::{DateTime, Local};
pub use openscreentime_policy::rules::StopReason;

/// The minutes-before-a-stop that get a warning.
pub const THRESHOLDS: [u32; 3] = [15, 5, 1];

/// A `reason` id as the status snapshot publishes it.
#[cfg_attr(not(feature = "tray"), allow(dead_code))] // the companion reads it back
pub fn parse_reason(id: &str) -> Option<StopReason> {
    Some(match id {
        "limit" => StopReason::Limit,
        "bedtime" => StopReason::Bedtime,
        "outside_hours" => StopReason::OutsideHours,
        "paused" => StopReason::Paused,
        _ => return None,
    })
}

/// Which thresholds have already been announced for the stop coming up.
#[derive(Debug, Default, Clone)]
pub struct WarnState {
    reason: Option<StopReason>,
    fired: Vec<u32>,
    /// When the stop last announced lands (unix seconds).
    announced_at: Option<i64>,
}

impl WarnState {
    /// [`observe`](Self::observe) a stop landing at `at` (unix seconds). A
    /// warning says *when* ("ends at 23:37"): if the stop has since moved to
    /// another minute (an idle spell isn't billed, so a limit slides later;
    /// a changed rule moves any stop), what was said is no longer true, and
    /// the warning due now is said again with the real time.
    pub fn observe_stop(&mut self, reason: StopReason, secs_left: i64, at: i64) -> Option<u32> {
        if let Some(prev) = self.announced_at {
            let moved = at.div_euclid(60) != prev.div_euclid(60) && (at - prev).abs() >= 30;
            if moved && self.reason == Some(reason) {
                self.fired.clear();
            }
        }
        let due = self.observe(reason, secs_left);
        if due.is_some() {
            self.announced_at = Some(at);
        }
        due
    }

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
        self.announced_at = None;
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
        StopReason::Limit => format!("Your time for today ends at {when}."),
        StopReason::Bedtime => format!("Bedtime starts at {when}."),
        StopReason::OutsideHours => format!("Allowed hours end at {when}."),
        StopReason::Paused => format!("A parent is pausing this computer at {when}."),
    };
    Words {
        title,
        body,
        critical: mins <= 1,
    }
}

/// "Screens come back …" — the same moment for the end of a sentence:
/// "at 07:00", "tomorrow" (at midnight), "tomorrow at 07:00", "Monday at 07:00".
pub fn back_words(at: DateTime<Local>, now: DateTime<Local>) -> String {
    let t = at.format("%H:%M").to_string();
    match (at.date_naive() - now.date_naive()).num_days() {
        d if d <= 0 => format!("at {t}"),
        1 if t == "00:00" => "tomorrow".to_string(),
        1 => format!("tomorrow at {t}"),
        _ => format!("{} at {t}", at.format("%A")),
    }
}

/// When a stopped person may use the screen again, said the way a person
/// would: "07:00", "tomorrow at 07:00", "Monday at 07:00".
pub fn until_words(at: DateTime<Local>, now: DateTime<Local>) -> String {
    let t = at.format("%H:%M").to_string();
    match (at.date_naive() - now.date_naive()).num_days() {
        d if d <= 0 => t,
        1 => format!("tomorrow at {t}"),
        _ => format!("{} at {t}", at.format("%A")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        // A Wednesday.
        Local.with_ymd_and_hms(2026, 9, 23, h, m, 0).unwrap()
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
            StopReason::OutsideHours,
            StopReason::Paused,
        ] {
            assert_eq!(announced(r, 40), vec![15, 5, 1], "{r:?}");
            let w = words(r, 60, Some(at(21, 0)));
            assert!(w.critical, "the last minute is critical for {r:?}");
            assert!(w.body.contains("21:00"));
            assert_ne!(w.title, w.title.to_uppercase(), "no shouting");
            assert_eq!(parse_reason(r.id()), Some(r));
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

    /// Acceptance, step 4: "5 minutes left — ends at 23:37", then the stop
    /// came at 23:38:51. A stop that moved to another minute is announced
    /// again, with the time it will really land; a few seconds of jitter
    /// is not a move.
    #[test]
    fn a_stop_that_moved_is_announced_again_with_its_real_time() {
        let t0 = at(23, 37).timestamp();
        let mut st = WarnState::default();
        assert_eq!(st.observe_stop(StopReason::Limit, 290, t0), Some(5));
        // Still the same moment, ticking down: nothing new to say.
        assert_eq!(st.observe_stop(StopReason::Limit, 250, t0 + 2), None);
        // Nobody used it for a minute and a half: the stop is now 23:38:30.
        let moved = t0 + 90;
        let w = words(StopReason::Limit, 250, Some(at(23, 38)));
        assert_eq!(st.observe_stop(StopReason::Limit, 250, moved), Some(5));
        assert!(w.body.contains("23:38"));
        // …and then counts down to it, once per threshold, as before.
        assert_eq!(st.observe_stop(StopReason::Limit, 200, moved), None);
        assert_eq!(st.observe_stop(StopReason::Limit, 55, moved + 1), Some(1));
        // Jitter across a minute boundary within seconds is not a move.
        let mut st = WarnState::default();
        let edge = at(21, 0).timestamp() - 2; // 20:59:58
        assert_eq!(st.observe_stop(StopReason::Bedtime, 280, edge), Some(5));
        assert_eq!(st.observe_stop(StopReason::Bedtime, 270, edge + 4), None);
    }

    #[test]
    fn until_reads_like_speech() {
        let now = at(22, 0);
        assert_eq!(until_words(at(23, 30), now), "23:30");
        assert_eq!(
            until_words(at(7, 0) + chrono::Duration::days(1), now),
            "tomorrow at 07:00"
        );
        let monday = at(7, 0) + chrono::Duration::days(5);
        assert_eq!(until_words(monday, now), "Monday at 07:00");
        assert_eq!(back_words(at(23, 30), now), "at 23:30");
        assert_eq!(
            back_words(at(0, 0) + chrono::Duration::days(1), now),
            "tomorrow"
        );
        assert_eq!(
            back_words(at(7, 0) + chrono::Duration::days(1), now),
            "tomorrow at 07:00"
        );
    }
}
