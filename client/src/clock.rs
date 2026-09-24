//! Time the agent can trust.
//!
//! Screen time is counted against the clock, and the clock is the easiest
//! thing on a computer to lie about. Three kernel clocks, three jobs:
//!
//! * **CLOCK_MONOTONIC** (`std::time::Instant`) — how long the machine was
//!   *awake* between two ticks. It stops while suspended, so billing with it
//!   means a closed lid never costs anyone a minute.
//! * **CLOCK_BOOTTIME** — real time since boot, *including* suspend. Nobody
//!   can set it. It is what lets the agent tell "midnight really passed" from
//!   "someone moved the clock forward".
//! * **The wall clock** — what the family's day is made of (midnight,
//!   bedtime), but only trusted while the kernel says it is NTP-synchronized.
//!   Setting the clock by hand (`date -s`, `timedatectl set-time`) makes the
//!   kernel mark it unsynchronized until NTP fixes it again.
//!
//! [`TrustedClock`] combines them: an anchor `(wall, boottime)` taken from a
//! time source we believe (an NTP-synced wall clock, the family server's
//! `server_time`, or the wall clock at boot), extrapolated by boottime. Every
//! screen-time decision — the day roll, bedtime, windows, override expiry —
//! reads this clock, so moving the wall clock by hand neither rolls the day
//! early nor stretches an override.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Real time since boot, including time spent suspended.
pub fn boottime() -> Duration {
    // SAFETY: clock_gettime writes into the timespec we own.
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) } != 0 {
        return Duration::ZERO;
    }
    Duration::new(
        ts.tv_sec.max(0) as u64,
        ts.tv_nsec.clamp(0, 999_999_999) as u32,
    )
}

/// This boot's id. Changes on every boot; stable across suspend/hibernate.
pub fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Is the kernel's wall clock NTP-synchronized right now? Read-only
/// `adjtimex` (modes = 0), the same source `timedatectl`'s
/// `NTPSynchronized=` comes from. A hand-set clock reads unsynchronized.
pub fn ntp_synced() -> bool {
    const STA_UNSYNC: i32 = 0x0040;
    const TIME_ERROR: i32 = 5;
    // SAFETY: modes = 0 only reads; the struct is ours.
    let mut tx: libc::timex = unsafe { std::mem::zeroed() };
    let state = unsafe { libc::adjtimex(&mut tx) };
    state >= 0 && state != TIME_ERROR && (tx.status & STA_UNSYNC) == 0
}

/// Where the trusted clock's current anchor came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The wall clock at boot (nothing better was known yet).
    #[default]
    Boot,
    /// An NTP-synchronized wall clock.
    Ntp,
    /// The family server's clock.
    Server,
}

/// A wall-clock anchor extrapolated by boottime. Persisted with the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TrustedClock {
    /// Boot the anchor belongs to. Boottime means nothing across boots.
    #[serde(default)]
    pub boot_id: String,
    /// Trusted UTC time at `anchor_boot_ms`.
    #[serde(default)]
    pub anchor_wall: Option<DateTime<Utc>>,
    /// Boottime (ms) when the anchor was taken.
    #[serde(default)]
    pub anchor_boot_ms: u64,
    #[serde(default)]
    pub source: Source,
    /// The latest trusted time ever computed. Time never goes backwards
    /// across a reboot: a boot whose wall clock is earlier than this was set
    /// back while the machine was off.
    #[serde(default)]
    pub high_water: Option<DateTime<Utc>>,
}

/// One reading of the three clocks, taken together.
#[derive(Debug, Clone, Copy)]
pub struct Reading<'a> {
    pub boot_id: &'a str,
    pub boot: Duration,
    pub wall: DateTime<Utc>,
    pub ntp_synced: bool,
}

impl TrustedClock {
    fn anchor(&mut self, r: &Reading, at: DateTime<Utc>, source: Source) {
        self.boot_id = r.boot_id.to_string();
        self.anchor_wall = Some(at);
        self.anchor_boot_ms = r.boot.as_millis() as u64;
        self.source = source;
    }

    /// Advance with a fresh reading and return trusted "now".
    ///
    /// * New boot (or never anchored): anchor at the wall clock, but never
    ///   earlier than the high-water mark — a clock set back while powered off
    ///   buys nothing.
    /// * NTP-synchronized wall clock: re-anchor to it (it *is* the truth, and
    ///   this is also how a clock that was wrong at boot gets corrected).
    /// * Otherwise: extrapolate the anchor by boottime. A hand-set wall clock
    ///   is ignored until NTP (or the server) vouches for it again.
    pub fn observe(&mut self, r: &Reading) -> DateTime<Utc> {
        let vouched = if self.boot_id != r.boot_id || self.anchor_wall.is_none() {
            let floor = self.high_water.filter(|hw| *hw > r.wall);
            self.anchor(r, floor.unwrap_or(r.wall), Source::Boot);
            false
        } else if r.ntp_synced {
            self.anchor(r, r.wall, Source::Ntp);
            true
        } else {
            false
        };
        let now = self.extrapolate(r.boot);
        // A vouched-for clock resets the mark (a wrong-ahead RTC must not stay
        // "the future" forever); otherwise it only moves forward.
        if vouched || self.high_water.is_none_or(|hw| now > hw) {
            self.high_water = Some(now);
        }
        now
    }

    /// The family server told us the time (TLS-authenticated): believe it.
    pub fn observe_server(&mut self, r: &Reading, server_time: DateTime<Utc>) {
        self.anchor(r, server_time, Source::Server);
        self.high_water = Some(server_time);
    }

    fn extrapolate(&self, boot: Duration) -> DateTime<Utc> {
        let base = self.anchor_wall.unwrap_or_else(Utc::now);
        let delta_ms = boot.as_millis() as i64 - self.anchor_boot_ms as i64;
        base + chrono::Duration::milliseconds(delta_ms)
    }

    /// How far the wall clock is ahead of trusted time right now (negative =
    /// behind). Diagnostics only: a hand-set clock shows up here.
    pub fn wall_skew(&self, r: &Reading) -> chrono::Duration {
        r.wall - self.extrapolate(r.boot)
    }
}

/// Read all three clocks now.
pub fn read(boot_id: &str) -> Reading<'_> {
    Reading {
        boot_id,
        boot: boottime(),
        wall: Utc::now(),
        ntp_synced: ntp_synced(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 24, h, m, 0).unwrap()
    }
    fn r(boot_id: &str, boot_min: u64, wall: DateTime<Utc>, synced: bool) -> Reading<'_> {
        Reading {
            boot_id,
            boot: Duration::from_secs(boot_min * 60),
            wall,
            ntp_synced: synced,
        }
    }

    #[test]
    fn a_hand_set_clock_is_ignored_until_ntp_vouches() {
        let mut c = TrustedClock::default();
        assert_eq!(c.observe(&r("a", 0, t(20, 0), true)), t(20, 0));
        // 10 real minutes later someone sets the clock 5 hours forward (the
        // kernel marks it unsynchronized): trusted time is 20:10, not 01:10.
        assert_eq!(
            c.observe(&r("a", 10, t(20, 10) + chrono::Duration::hours(5), false)),
            t(20, 10)
        );
        assert!(
            c.wall_skew(&r("a", 10, t(20, 10) + chrono::Duration::hours(5), false))
                > chrono::Duration::hours(4)
        );
        // …and set back instead: still 20:20 by boottime.
        assert_eq!(c.observe(&r("a", 20, t(15, 0), false)), t(20, 20));
        // NTP fixes it; synced wall is believed again.
        assert_eq!(c.observe(&r("a", 30, t(20, 30), true)), t(20, 30));
    }

    #[test]
    fn suspend_counts_as_real_time_for_the_clock() {
        let mut c = TrustedClock::default();
        c.observe(&r("a", 0, t(8, 0), false));
        // Laptop asleep for 9 hours: boottime includes it, unsynced wall too.
        assert_eq!(c.observe(&r("a", 9 * 60, t(17, 0), false)), t(17, 0));
    }

    #[test]
    fn a_new_boot_trusts_the_wall_but_never_goes_back_in_time() {
        let mut c = TrustedClock::default();
        c.observe(&r("a", 0, t(20, 0), true));
        c.observe(&r("a", 60, t(21, 0), true));
        // Rebooted; the clock was set back to 18:00 while off.
        assert_eq!(c.observe(&r("b", 1, t(18, 0), false)), t(21, 0));
        // An honest reboot the next morning is believed.
        let mut c2 = c.clone();
        let next_morning = t(21, 0) + chrono::Duration::hours(10);
        assert_eq!(c2.observe(&r("c", 1, next_morning, false)), next_morning);
    }

    #[test]
    fn the_server_clock_overrides_a_wrong_wall_clock() {
        let mut c = TrustedClock::default();
        // Booted with the RTC 5 hours behind, no NTP.
        c.observe(&r("a", 0, t(10, 0), false));
        c.observe_server(&r("a", 1, t(10, 1), false), t(15, 1));
        assert_eq!(c.observe(&r("a", 2, t(10, 2), false)), t(15, 2));
        assert_eq!(c.source, Source::Server);
    }

    #[test]
    fn the_kernel_clocks_are_readable() {
        // Smoke: these must not panic on any Linux (containers included).
        let _ = boottime();
        let _ = ntp_synced();
        let _ = boot_id();
    }
}
