//! What the app window shows at a glance, decided by the agent (which has the
//! policy) and published in the person's own status file: today's three
//! rules, and who sees what — the honest footer.
//!
//! The footer is the product's thesis, so it has to be true for the person
//! reading it. What a parent sees follows the server's exposure rules
//! (`server/src/usage.rs`, `Exposure`): the family sees a little, kid or
//! younger teen's time, apps and sites; an older teen's time and apps, not
//! their sites; and nothing but the time of someone who sets their own limits
//! (an adult, a self-managed teen) — a parent's own login not even that.
//! Sites are counted per computer, not per person, so on a computer shared
//! with someone younger a parent sees its site list through them.

use openscreentime_policy::{AgeBracket, ScreenTime};
use serde::{Deserialize, Serialize};

/// Who sees what of this person's day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Sees {
    /// A parent sees their time, the apps and the sites.
    #[default]
    TimeAppsSites,
    /// A parent sees their time and the apps; the sites are not shown.
    TimeApps,
    /// They set their own limits: the apps and sites are theirs alone.
    Own,
}

/// The exposure for a login, from its profile kind (the bracket id or a
/// legacy preset name) and whether the person manages themselves.
pub fn sees(kind: &str, self_managed: bool) -> Sees {
    let bracket = match kind {
        "kids" => AgeBracket::Kid,
        "teen" => AgeBracket::YoungerTeen,
        "adult" | "default" => AgeBracket::Adult,
        k => AgeBracket::parse(k).unwrap_or(AgeBracket::Kid),
    };
    if self_managed || !bracket.is_managed() {
        Sees::Own
    } else if bracket == AgeBracket::OlderTeen {
        Sees::TimeApps
    } else {
        Sees::TimeAppsSites
    }
}

/// The honest footer (board 05c), true for this person. `shared_sites`: their
/// sites are hidden from a parent, but this computer is shared with someone
/// whose aren't — so the computer's site list is seen after all.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn footer(sees: Sees, shared_sites: bool) -> String {
    let counts = match sees {
        Sees::TimeAppsSites => "It counts your time and which apps and sites you use.",
        Sees::TimeApps => {
            "It counts your time and which apps and sites you use. A parent sees your time and \
             apps, not your sites."
        }
        Sees::Own => {
            "It counts your time and which apps and sites you use — for you. No one else sees \
             your apps or sites."
        }
    };
    let shared = if shared_sites && sees != Sees::TimeAppsSites {
        " This computer is shared with someone younger, so a parent sees the sites it looks up."
    } else {
        ""
    };
    format!("{counts}{shared} It can't see your screen, your messages or what you type.")
}

/// Today's rules, in the words the app window lists them (board 05c).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Today {
    /// The daily limit in minutes; `None` = no limit.
    #[serde(default)]
    pub limit_minutes: Option<u32>,
    /// Allowed hours today, e.g. "07:00 – 20:00" (several joined by ", ").
    #[serde(default)]
    pub screens_on: Option<String>,
    /// When bedtime starts.
    #[serde(default)]
    pub bedtime: Option<String>,
}

/// Today's rules for weekday `day` (0 = Sunday, as the policy counts).
pub fn today(st: &ScreenTime, day: u8) -> Today {
    if !st.enabled {
        return Today::default();
    }
    // "7:00", not "07:00" (board 05c).
    let hm = |s: &str| {
        openscreentime_policy::rules::parse_hm(s).map(|m| format!("{}:{:02}", m / 60, m % 60))
    };
    let windows: Vec<String> = st
        .schedule
        .iter()
        .filter(|w| w.days.contains(&day))
        .filter_map(|w| Some(format!("{} – {}", hm(&w.start)?, hm(&w.end)?)))
        .collect();
    Today {
        limit_minutes: (st.daily_limit_minutes > 0).then_some(st.daily_limit_minutes),
        screens_on: (!windows.is_empty()).then(|| windows.join(", ")),
        bedtime: st
            .bedtime
            .as_ref()
            .and_then(|b| hm(&b.start).filter(|_| hm(&b.end).is_some())),
    }
}

/// The verdict fields of a person's status file (docs/AGENT.md →
/// "status.<user>.json") — the one truth every surface reads "time left" from:
/// the app window, the companion, `ost time`. Flattened into each reader's
/// own view of the file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    /// The day's budget left (limit + earned − used). Only an agent that
    /// predates `allowed`/`minutes_left` needs it; `None` = no daily limit.
    #[serde(default)]
    pub remaining_minutes: Option<i64>,
    /// May they use the screen now? `None` from an agent before the verdict.
    #[serde(default)]
    pub allowed: Option<bool>,
    #[serde(default)]
    pub reason: Option<String>,
    /// Minutes until `stop_at`, rounded up, as of the agent's last tick.
    #[serde(default)]
    pub minutes_left: Option<u32>,
    /// When the screen stops (RFC 3339).
    #[serde(default)]
    pub stop_at: Option<String>,
    /// A parent's override (a code, a grant, a snooze) runs until then.
    #[serde(default)]
    pub override_until: Option<String>,
    /// This minute is being billed.
    #[serde(default)]
    pub counting: bool,
    #[serde(default)]
    pub frozen: bool,
}

/// What "time left" says right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Left {
    /// No daily limit, nothing counting down: show the time used instead.
    NoLimit,
    /// Stopped: time's up, bedtime, outside the hours, a pause.
    Stopped,
    /// The screen stops in `minutes` (rounded up, like every warning).
    /// `unlocked_until`: a parent's override is what keeps it going, and
    /// the stop is when it ends — say so plainly, never a red zero.
    Minutes {
        minutes: i64,
        unlocked_until: Option<chrono::DateTime<chrono::Local>>,
    },
}

/// An RFC 3339 time from the status file, in local time.
pub fn parse_local(s: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&chrono::Local))
}

/// Whole minutes from `now` until `at`, rounded up (the verdict's rule).
pub fn minutes_until(
    at: chrono::DateTime<chrono::Local>,
    now: chrono::DateTime<chrono::Local>,
) -> i64 {
    ((at - now).num_seconds().max(0) + 59) / 60
}

impl Clock {
    /// The stop, when it is a real moment on the clock: bedtime, the end of
    /// the hours or of an override, a pause's countdown — or the daily limit
    /// while the minutes are being used. An idle person's limit stop moves
    /// later every tick (idle time isn't billed), so it is not a moment yet.
    pub fn stop_moment(&self) -> Option<chrono::DateTime<chrono::Local>> {
        let at = self.stop_at.as_deref().and_then(parse_local)?;
        (self.counting || self.reason.as_deref() != Some("limit")).then_some(at)
    }

    /// Time left, the same everywhere: the rules' verdict — which already
    /// knows about bedtime, the hours, the limit and a parent's override —
    /// counted down live to its stop.
    pub fn left(&self, now: chrono::DateTime<chrono::Local>) -> Left {
        if self.frozen {
            return Left::Stopped;
        }
        let Some(allowed) = self.allowed else {
            // An agent from before the verdict: all it had was the budget.
            return match self.remaining_minutes {
                None => Left::NoLimit,
                Some(m) if m <= 0 => Left::Stopped,
                Some(m) => Left::Minutes {
                    minutes: m,
                    unlocked_until: None,
                },
            };
        };
        if !allowed {
            return Left::Stopped;
        }
        let ov = self
            .override_until
            .as_deref()
            .and_then(parse_local)
            .filter(|t| *t > now);
        let stop = self.stop_at.as_deref().and_then(parse_local);
        let minutes = match self.stop_moment() {
            Some(at) => Some(minutes_until(at, now)),
            None => self.minutes_left.map(i64::from),
        };
        match minutes {
            // No daily limit and no override: a far-off bedtime is not a
            // countdown (the rules card says when it is).
            _ if self.remaining_minutes.is_none() && ov.is_none() => Left::NoLimit,
            Some(minutes) => Left::Minutes {
                minutes,
                // The override is what they are running on when the stop is
                // when it ends (a grant with budget to spare just adds time).
                unlocked_until: ov
                    .filter(|o| stop.is_some_and(|s| (s - *o).num_seconds().abs() < 60)),
            },
            // Nothing stops them within two days: a 24-hour limit.
            None => match self.remaining_minutes {
                Some(m) if m > 0 => Left::Minutes {
                    minutes: m,
                    unlocked_until: None,
                },
                _ => Left::NoLimit,
            },
        }
    }
}

/// "1 h 15 min", "45 min", "3 h" — a limit in a list.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn duration(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_parent_sees_follows_the_servers_rules() {
        for k in ["little", "kid", "younger_teen", "kids", "teen"] {
            assert_eq!(sees(k, false), Sees::TimeAppsSites, "{k}");
        }
        assert_eq!(sees("older_teen", false), Sees::TimeApps);
        // Self-managed (an adult, or a teen trusted with their own day).
        assert_eq!(sees("adult", false), Sees::Own);
        assert_eq!(sees("default", false), Sees::Own);
        assert_eq!(sees("older_teen", true), Sees::Own);
        assert_eq!(sees("kid", true), Sees::Own);
    }

    #[test]
    fn the_footer_is_true_per_bracket() {
        let kid = footer(Sees::TimeAppsSites, false);
        // Board 05c, verbatim, for the brackets it was written for.
        assert_eq!(
            kid,
            "It counts your time and which apps and sites you use. It can't see your screen, \
             your messages or what you type."
        );
        let teen = footer(Sees::TimeApps, false);
        assert!(teen.contains("not your sites"));
        assert!(!teen.contains("shared"));
        let own = footer(Sees::Own, false);
        assert!(own.contains("No one else sees your apps or sites"));
        assert!(!own.contains("A parent sees"));
        // On a computer shared with a younger child, the site list is seen.
        let shared = footer(Sees::TimeApps, true);
        assert!(shared.contains("shared with someone younger"));
        assert_eq!(footer(Sees::TimeAppsSites, true), kid, "already said");
        for f in [kid, teen, own, shared] {
            assert!(f.ends_with("It can't see your screen, your messages or what you type."));
        }
    }

    fn clock(json: serde_json::Value) -> Clock {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn time_left_is_the_verdicts_everywhere() {
        let now = chrono::Local::now();
        let at = |m: i64| (now + chrono::Duration::minutes(m)).to_rfc3339();
        // A code at the lock after time's up (acceptance step 5): the budget
        // is spent (−1), the override gives 29 — that's what's left.
        let c = clock(serde_json::json!({
            "remaining_minutes": -1, "allowed": true, "reason": "limit",
            "minutes_left": 29, "stop_at": at(29), "override_until": at(29), "counting": true
        }));
        match c.left(now) {
            Left::Minutes {
                minutes: 29,
                unlocked_until: Some(t),
            } => assert_eq!(
                t.format("%H:%M").to_string(),
                (now + chrono::Duration::minutes(29))
                    .format("%H:%M")
                    .to_string()
            ),
            other => panic!("{other:?}"),
        }
        // A grant with budget to spare: more time, not "unlocked until".
        let c = clock(serde_json::json!({
            "remaining_minutes": 45, "allowed": true, "reason": "limit",
            "minutes_left": 45, "stop_at": at(45), "override_until": at(15), "counting": true
        }));
        assert_eq!(
            c.left(now),
            Left::Minutes {
                minutes: 45,
                unlocked_until: None
            }
        );
        // Bedtime before the budget runs out: the stop is bedtime's.
        let c = clock(serde_json::json!({
            "remaining_minutes": 40, "allowed": true, "reason": "bedtime",
            "minutes_left": 10, "stop_at": at(10), "counting": false
        }));
        assert!(matches!(c.left(now), Left::Minutes { minutes: 10, .. }));
        // Idle: the limit stop slides; the last tick's minutes stand.
        let c = clock(serde_json::json!({
            "remaining_minutes": 5, "allowed": true, "reason": "limit",
            "minutes_left": 5, "stop_at": at(3), "counting": false
        }));
        assert!(matches!(c.left(now), Left::Minutes { minutes: 5, .. }));
        // Counting: live, rounded up like every warning.
        let c = clock(serde_json::json!({
            "remaining_minutes": 5, "allowed": true, "reason": "limit",
            "minutes_left": 5, "stop_at": (now + chrono::Duration::seconds(61)).to_rfc3339(),
            "counting": true
        }));
        assert!(matches!(c.left(now), Left::Minutes { minutes: 2, .. }));
        // Stopped, frozen, no limit, an older agent.
        assert_eq!(
            clock(serde_json::json!({ "allowed": false, "remaining_minutes": 0 })).left(now),
            Left::Stopped
        );
        assert_eq!(
            clock(serde_json::json!({ "frozen": true })).left(now),
            Left::Stopped
        );
        assert_eq!(
            clock(serde_json::json!({ "allowed": true, "reason": "bedtime",
                "minutes_left": 600, "stop_at": at(600) }))
            .left(now),
            Left::NoLimit,
            "no daily limit: a far bedtime is not a countdown"
        );
        assert_eq!(
            clock(serde_json::json!({ "remaining_minutes": 12 })).left(now),
            Left::Minutes {
                minutes: 12,
                unlocked_until: None
            }
        );
    }

    /// The computer says what the console says, for the same inputs — the
    /// server checks the same vectors (server/src/ledger.rs, `console_day`).
    /// Through the agent's own path: its ledger, its verdict, the status
    /// fields, and what the window reads from them.
    #[test]
    fn time_left_matches_the_console_for_the_same_inputs() {
        use crate::enforce::screentime::{self, UsageTracker};
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../../policy/tests/verdict-vectors.json")).unwrap();
        let now_utc: chrono::DateTime<chrono::Utc> = v["now"].as_str().unwrap().parse().unwrap();
        let now = now_utc.with_timezone(&chrono::FixedOffset::east_opt(0).unwrap());
        for c in v["cases"].as_array().unwrap() {
            let policy: crate::policy::Policy =
                serde_json::from_value(serde_json::json!({ "screen_time": c["screen_time"] }))
                    .unwrap();
            let mut t = UsageTracker::new();
            t.roll_to(now.date_naive());
            t.add_active("mia", c["used_secs"].as_u64().unwrap() as u32, 1);
            t.add_earned("mia", (c["earned_secs"].as_u64().unwrap() / 60) as u32);
            if let Some(m) = c["override_in_min"].as_i64() {
                t.set_override("mia", now_utc + chrono::Duration::minutes(m));
            }
            let verdict = screentime::verdict(&policy, &t, "mia", &now, false);
            let ts = |x: chrono::DateTime<chrono::FixedOffset>| x.to_rfc3339();
            let status = Clock {
                remaining_minutes: t.remaining_minutes("mia", &policy),
                allowed: Some(verdict.allowed),
                reason: verdict.reason.map(|r| r.id().to_string()),
                minutes_left: verdict.minutes_left,
                stop_at: verdict.stop_at.map(ts),
                override_until: t.peek_override("mia", now_utc).map(|x| x.to_rfc3339()),
                counting: true,
                frozen: false,
            };
            let left = match status.left(now_utc.with_timezone(&chrono::Local)) {
                Left::NoLimit => None,
                Left::Stopped => Some(0),
                Left::Minutes { minutes, .. } => Some(minutes),
            };
            assert_eq!(serde_json::json!(left), c["left_minutes"], "{}", c["name"]);
        }
    }

    #[test]
    fn todays_rules_read_like_the_board() {
        let st: ScreenTime = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "daily_limit_minutes": 75,
            "schedule": [
                { "days": [1, 2, 3, 4, 5], "start": "07:00", "end": "20:00" },
                { "days": [0, 6], "start": "09:00", "end": "21:00" }
            ],
            "bedtime": { "start": "20:00", "end": "07:00" }
        }))
        .unwrap();
        let wed = today(&st, 3);
        assert_eq!(wed.limit_minutes, Some(75));
        assert_eq!(wed.screens_on.as_deref(), Some("7:00 – 20:00"));
        assert_eq!(wed.bedtime.as_deref(), Some("20:00"));
        assert_eq!(today(&st, 0).screens_on.as_deref(), Some("9:00 – 21:00"));
        let off = ScreenTime {
            enabled: false,
            ..st
        };
        assert_eq!(today(&off, 3), Today::default());
        assert_eq!(duration(75), "1 h 15 min");
        assert_eq!(duration(45), "45 min");
        assert_eq!(duration(180), "3 h");
    }
}
