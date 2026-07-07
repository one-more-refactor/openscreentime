//! The gamification layer: earn-time tasks, streaks, and nudges (Duolingo-style).
//! Pairs with `lockout.rs` (the presenter) and `screentime.rs` (the ledger).
//!
//! Earn-time and streaks are ultimately server-authoritative (the
//! `screen_time_ledger` table). The agent's job is to (a) surface earn-time tasks
//! in the overlay, (b) credit local budget when a task is marked done, and (c) fire
//! `screen_time_earned` / `streak` events. Task completion approval (did the kid
//! really read for 20 min?) is a parent/PIN action in the skeleton.

use crate::policy::{Gamification, Policy};
use crate::protocol::{Event, EV_SCREEN_TIME_EARNED, EV_STREAK, SEV_INFO};
use serde_json::json;

/// A nudge the agent may show (streak reminders: bedtime, breaks).
#[derive(Debug, Clone)]
pub struct Nudge {
    pub kind: String,
    pub copy: String,
}

/// Build the streak nudges enabled by the policy.
pub fn nudges_for(policy: &Policy) -> Vec<Nudge> {
    let g = &policy.gamification;
    if !g.streaks.enabled {
        return Vec::new();
    }
    g.streaks
        .nudges
        .iter()
        .map(|k| Nudge {
            kind: k.clone(),
            copy: match k.as_str() {
                "bedtime" => "WIND DOWN — BEDTIME SOON. KEEP YOUR STREAK 🔥".into(),
                "breaks" => "STAND UP, STRETCH. 20-20-20 FOR YOUR EYES.".into(),
                other => format!("NUDGE: {}", other.to_uppercase()),
            },
        })
        .collect()
}

/// Earn-time task offer shown on the lockout screen ("EARN 15 MIN — READ FOR 20").
#[derive(Debug, Clone)]
pub struct EarnOffer {
    pub id: String,
    pub label: String,
    pub reward_minutes: u32,
}

pub fn earn_offers(g: &Gamification) -> Vec<EarnOffer> {
    if !g.earn_time.enabled {
        return Vec::new();
    }
    g.earn_time
        .tasks
        .iter()
        .map(|t| EarnOffer {
            id: t.id.clone(),
            label: t.label.clone(),
            reward_minutes: t.reward_minutes,
        })
        .collect()
}

/// Event to emit when a task is completed & approved. The approval trigger
/// (parent/PIN confirmation that the task was really done) is not wired in the
/// skeleton — see README "what's stubbed" — so this is exercised only by tests.
#[allow(dead_code)]
pub fn earned_event(user: &str, task_id: &str, minutes: u32) -> Event {
    Event::new(
        EV_SCREEN_TIME_EARNED,
        SEV_INFO,
        json!({ "task_id": task_id, "reward_minutes": minutes }),
    )
    .for_user(user)
}

/// Event for a streak milestone / nudge shown.
pub fn streak_event(user: &str, kind: &str, streak_days: u32) -> Event {
    Event::new(
        EV_STREAK,
        SEV_INFO,
        json!({ "nudge": kind, "streak_days": streak_days }),
    )
    .for_user(user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kids_nudges_render() {
        let mut p = Policy::default();
        p.gamification.streaks.enabled = true;
        p.gamification.streaks.nudges = vec!["bedtime".into(), "breaks".into()];
        let n = nudges_for(&p);
        assert_eq!(n.len(), 2);
        assert!(n[0].copy.contains("BEDTIME"));
    }
}
