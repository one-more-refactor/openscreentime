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
