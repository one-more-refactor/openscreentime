//! Earn-time: the tasks a child can do to buy back screen time, and the event
//! fired when a parent approves one.
//!
//! Earn-time is server-authoritative (the `screen_time_ledger` table). The
//! agent's job is to (a) surface the offers on the lockout overlay, (b) credit
//! local budget when the server sends `credit_time`, and (c) fire the
//! `screen_time_earned` event. Whether the child really read for 20 minutes is
//! a parent decision, made in the console.
//!
//! This module used to be `gamify.rs` and also carried streak nudges. Those
//! were deleted, not moved: an app that fires "KEEP YOUR STREAK 🔥" at a child
//! is engagement bait, and the product brief is explicit that this one is
//! silent unless a human must act.

use crate::policy::Gamification;
use crate::protocol::{Event, EV_SCREEN_TIME_EARNED, SEV_INFO};
use serde_json::json;

/// Earn-time task offer shown on the lockout screen ("Earn 15 min — read for 20").
#[derive(Debug, Clone)]
pub struct EarnOffer {
    /// Task id: the dedupe key for the once-per-day auto earn-request
    /// (`runner::Agent::auto_request_earn`) and the payload sent to the server.
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

/// Event to emit when a task is completed and approved — i.e. when the runner
/// handles a `credit_time` command after a parent approves an earn-request.
pub fn earned_event(user: &str, task_id: &str, minutes: u32) -> Event {
    Event::new(
        EV_SCREEN_TIME_EARNED,
        SEV_INFO,
        json!({ "task_id": task_id, "reward_minutes": minutes }),
    )
    .for_user(user)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Policy;

    #[test]
    fn offers_are_empty_unless_earn_time_is_enabled() {
        let mut p = Policy::default();
        assert!(earn_offers(&p.gamification).is_empty());

        p.gamification.earn_time.enabled = true;
        p.gamification.earn_time.tasks = vec![crate::policy::EarnTask {
            id: "reading".into(),
            label: "Read for 20 min".into(),
            reward_minutes: 15,
        }];
        let offers = earn_offers(&p.gamification);
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].reward_minutes, 15);
    }
}
