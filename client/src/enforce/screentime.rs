//! Screen-time accounting and the verdict: how much time a person has used
//! today, and whether they should be stopped right now, and why.
//!
//! * **What counts** is decided in [`super::activity`] (a foreground seat
//!   session with recent input or sound).
//! * **The day** is the trusted clock's local date ([`crate::clock`]): it rolls
//!   at local midnight, never earlier than real boottime allows, forward only,
//!   and without needing the server.
//! * **The rules** are [`openscreentime_policy::rules::evaluate`] — the same
//!   function the server uses.
//! * **Per person**: the daily limit is one budget across all of a person's
//!   computers. The server reports what they used elsewhere today; this device
//!   adds its own. Offline, the last-known "elsewhere" still applies.
//!
//! Stopping someone (the lock, the freeze) is the runner's business; the
//! freeze primitives at the bottom of this file are shared with it, and what
//! a stop freezes is decided in [`freeze`].

pub mod freeze;

use crate::clock::{Reading, TrustedClock};
use crate::policy::Policy;
use crate::sysusers;
use crate::util::Exec;
use anyhow::Result;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use openscreentime_policy::rules::{self, StopReason, Verdict};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Reboot-surviving usage ledger. Without this, a `systemctl restart` (crash,
/// watchdog kick, self-update — or a kid who guesses the trick) drops the
/// in-memory counters to zero and hands out a fresh daily budget. It also
/// carries the parent overrides and the trusted-clock anchor, so neither a
/// restart nor a reboot loses a parent's "30 more minutes".
pub fn ledger_path() -> std::path::PathBuf {
    crate::paths::state("usage_ledger.json")
}

/// Why a user is being locked out. (The stop presenters key their words off
/// this; [`lock_reason`] maps the rules' verdict onto it.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockReason {
    DailyLimit { used_min: u32, limit_min: u32 },
    OutsideWindow,
    Bedtime,
}

impl LockReason {
    pub fn headline(&self) -> String {
        match self {
            // Sentence case, warm, no shouting — the same words the README
            // promises ("Stop — time's up for today"), on every surface.
            LockReason::DailyLimit { .. } => "Stop".into(),
            LockReason::OutsideWindow => "Not now".into(),
            LockReason::Bedtime => "Goodnight".into(),
        }
    }
    pub fn detail(&self) -> String {
        match self {
            LockReason::DailyLimit {
                used_min,
                limit_min,
            } => {
                format!("Time's up for today — {used_min} of {limit_min} minutes used.")
            }
            LockReason::OutsideWindow => "Screens are off at this time of day.".into(),
            LockReason::Bedtime => "Screens are off until morning.".into(),
        }
    }
}

/// What the family server says a person used on their *other* computers
/// today (and was granted there). Kept per OS user, tagged with the day it
/// belongs to, so yesterday's number never leaks into today.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Elsewhere {
    pub day: Option<NaiveDate>,
    #[serde(default)]
    pub used_secs: u64,
    #[serde(default)]
    pub earned_secs: u64,
    /// The server's record of grants to THIS login today (see `day_for`).
    #[serde(default)]
    pub earned_here_secs: u64,
}

/// Outcome of a parent grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    Applied,
    /// This command id was already applied — a redelivery after a lost ack.
    Duplicate,
}

/// The day's ledger for every managed user on this device.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UsageTracker {
    /// The accounting day (trusted local date). Forward-only.
    day: Option<NaiveDate>,
    /// Seconds of real use on THIS device today.
    used_secs: HashMap<String, u32>,
    /// Seconds granted on this device today (earn-time, "+N min").
    earned_secs: HashMap<String, u32>,
    /// The person's use on other computers today, per the server.
    #[serde(default)]
    elsewhere: HashMap<String, Elsewhere>,
    /// Parent overrides: user → end (trusted UTC). One per user, whatever
    /// wrote it — a grant, a code at the lock screen, `ost unlock`, Resume.
    #[serde(default)]
    overrides: HashMap<String, DateTime<Utc>>,
    /// Grant command ids already applied → the day they landed. A lost ack
    /// makes the server redeliver; the second copy must not credit twice.
    #[serde(default)]
    grants: HashMap<String, NaiveDate>,
    /// "Give me 15 more minutes" taken today, per user (self-set limits only).
    #[serde(default)]
    snoozes: HashMap<String, u32>,
    /// Trusted-clock anchor, persisted so a restart keeps it.
    #[serde(default)]
    pub clock: TrustedClock,
}

impl UsageTracker {
    /// Fresh, empty tracker (tests; the running agent uses [`load`](Self::load)).
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

    /// Read the clocks into the trusted clock, roll the day if the trusted
    /// local date has advanced, and return trusted "now".
    pub fn advance<Tz: TimeZone>(&mut self, reading: &Reading, tz: &Tz) -> DateTime<Utc> {
        let now = self.clock.observe(reading);
        self.roll_to(now.with_timezone(tz).date_naive());
        now
    }

    /// Roll to `today` ONLY when it is later than the day being accounted —
    /// forward-only on purpose: a clock set *backward* must never wipe the
    /// counters (that was an instant free-time cheat). `today` comes from the
    /// trusted clock, so a clock set *forward* by hand doesn't get here early
    /// either. Returns whether a new day started.
    pub fn roll_to(&mut self, today: NaiveDate) -> bool {
        if self.day.is_some_and(|d| today <= d) {
            return false;
        }
        self.day = Some(today);
        self.used_secs.clear();
        self.earned_secs.clear();
        self.elsewhere.retain(|_, e| e.day == Some(today));
        self.snoozes.clear();
        // Keep grant ids a couple of days: a grant delivered late must still
        // be recognised as already applied.
        self.grants
            .retain(|_, d| today.signed_duration_since(*d).num_days() <= 2);
        true
    }

    /// The accounting day.
    pub fn day(&self) -> Option<NaiveDate> {
        self.day
    }

    /// Bill `real_secs` of real use to `user`, scaled by the dev time-accel.
    pub fn add_active(&mut self, user: &str, real_secs: u32, accel: u32) {
        *self.used_secs.entry(user.to_string()).or_insert(0) += real_secs.saturating_mul(accel);
    }

    /// Credit earned minutes to today's budget (no override, no idempotency —
    /// see [`grant`](Self::grant) for a parent's "+N").
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn add_earned(&mut self, user: &str, minutes: u32) {
        *self.earned_secs.entry(user.to_string()).or_insert(0) += minutes.saturating_mul(60);
    }

    /// A parent's "+N minutes": N more minutes on today's budget AND an
    /// override until `now + N`, so it also carries past bedtime or the end of
    /// the allowed hours ("N more minutes, now, whatever the rule").
    /// Idempotent on `id` (the command id).
    pub fn grant(&mut self, id: &str, user: &str, minutes: u32, now: DateTime<Utc>) -> Grant {
        if !id.is_empty() && self.grants.contains_key(id) {
            return Grant::Duplicate;
        }
        if !id.is_empty() {
            self.grants
                .insert(id.to_string(), self.day.unwrap_or_else(|| now.date_naive()));
        }
        self.add_earned_secs(user, u64::from(minutes) * 60);
        self.set_override(user, now + chrono::Duration::minutes(i64::from(minutes)));
        Grant::Applied
    }

    fn add_earned_secs(&mut self, user: &str, secs: u64) {
        let e = self.earned_secs.entry(user.to_string()).or_insert(0);
        *e = e.saturating_add(u32::try_from(secs).unwrap_or(u32::MAX));
    }

    /// Hold the rules off for `user` until `until` (trusted UTC). Never
    /// shortens an override already running.
    pub fn set_override(&mut self, user: &str, until: DateTime<Utc>) {
        let e = self.overrides.entry(user.to_string()).or_insert(until);
        if until > *e {
            *e = until;
        }
    }

    /// The active override's end, if one is running at `now`. Expired ones
    /// are dropped on the way.
    pub fn override_until(&mut self, user: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self.overrides.get(user) {
            Some(u) if *u > now => Some(*u),
            Some(_) => {
                self.overrides.remove(user);
                None
            }
            None => None,
        }
    }

    /// Snoozes `user` has taken today.
    pub fn snoozes(&self, user: &str) -> u32 {
        self.snoozes.get(user).copied().unwrap_or(0)
    }

    /// A self-set snooze: count it and hold the rules off for `minutes`.
    pub fn snooze(&mut self, user: &str, minutes: u32, now: DateTime<Utc>) -> u32 {
        let n = self.snoozes.entry(user.to_string()).or_insert(0);
        *n += 1;
        let n = *n;
        self.set_override(user, now + chrono::Duration::minutes(i64::from(minutes)));
        n
    }

    /// Read-only view of an override (for status output).
    pub fn peek_override(&self, user: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.overrides.get(user).copied().filter(|u| *u > now)
    }

    /// Record what the server says the person used elsewhere today. Ignored
    /// unless it is for the day being accounted here.
    pub fn set_elsewhere(&mut self, user: &str, e: Elsewhere) {
        if e.day.is_some() && e.day == self.day {
            self.elsewhere.insert(user.to_string(), e);
        }
    }

    /// Today's report from the server for `user`, if it is for today.
    fn elsewhere_of(&self, user: &str) -> Elsewhere {
        self.elsewhere
            .get(user)
            .filter(|e| e.day.is_some() && e.day == self.day)
            .cloned()
            .unwrap_or_default()
    }

    /// Seconds used on this device today.
    pub fn used_here_secs(&self, user: &str) -> u64 {
        u64::from(self.used_secs.get(user).copied().unwrap_or(0))
    }

    /// The person's whole day: this device plus their other computers.
    /// Grants on this login are the larger of what this device applied and
    /// what the server has on record (it credits its ledger when the parent
    /// grants, before the command arrives) — the larger, never the sum.
    pub fn day_for(&self, user: &str) -> rules::Day {
        let e = self.elsewhere_of(user);
        let earned_here =
            u64::from(self.earned_secs.get(user).copied().unwrap_or(0)).max(e.earned_here_secs);
        rules::Day {
            used_secs: self.used_here_secs(user) + e.used_secs,
            earned_secs: earned_here + e.earned_secs,
        }
    }

    /// Minutes used today by the person (all their computers), floored.
    pub fn used_minutes(&self, user: &str) -> u32 {
        u32::try_from(self.day_for(user).used_secs / 60).unwrap_or(u32::MAX)
    }

    /// Minutes earned today by the person (all their computers), floored.
    pub fn earned_minutes(&self, user: &str) -> u32 {
        u32::try_from(self.day_for(user).earned_secs / 60).unwrap_or(u32::MAX)
    }

    /// Daily budget left in minutes (limit + earned − used, rounded up; may be
    /// negative). `None` = no limit. This is the ring's number; when the screen
    /// actually stops is [`verdict`]'s `minutes_left`.
    pub fn remaining_minutes(&self, user: &str, policy: &Policy) -> Option<i64> {
        let st = &policy.screen_time;
        if !st.enabled || st.daily_limit_minutes == 0 {
            return None;
        }
        let d = self.day_for(user);
        let left =
            i64::from(st.daily_limit_minutes) * 60 + d.earned_secs as i64 - d.used_secs as i64;
        Some((left + 59).div_euclid(60))
    }
}

/// The rules' verdict for `user` at trusted local time `now`. `paused` is a
/// whole-device lock; the override comes from the ledger.
pub fn verdict<Tz: TimeZone>(
    policy: &Policy,
    tracker: &UsageTracker,
    user: &str,
    now: &DateTime<Tz>,
    paused: bool,
) -> Verdict<Tz> {
    let until = tracker
        .peek_override(user, now.with_timezone(&Utc))
        .map(|u| u.with_timezone(&now.timezone()));
    rules::evaluate(
        &policy.screen_time,
        now,
        tracker.day_for(user),
        until.as_ref(),
        paused,
    )
}

/// The screen-time stop the runner acts on, if the verdict says "stop now".
/// (A pause is not a screen-time reason — the runner handles device locks.)
pub fn lock_reason<Tz: TimeZone>(
    v: &Verdict<Tz>,
    tracker: &UsageTracker,
    user: &str,
    policy: &Policy,
) -> Option<LockReason> {
    if v.allowed {
        return None;
    }
    match v.reason? {
        StopReason::Bedtime => Some(LockReason::Bedtime),
        StopReason::OutsideHours => Some(LockReason::OutsideWindow),
        StopReason::Limit => Some(LockReason::DailyLimit {
            used_min: tracker.used_minutes(user),
            limit_min: policy.screen_time.daily_limit_minutes + tracker.earned_minutes(user),
        }),
        StopReason::Paused => None,
    }
}

/// Evaluate whether `user` should be locked at trusted local time `now`.
pub fn evaluate<Tz: TimeZone>(
    policy: &Policy,
    tracker: &UsageTracker,
    user: &str,
    now: &DateTime<Tz>,
) -> Option<LockReason> {
    let v = verdict(policy, tracker, user, now, false);
    lock_reason(&v, tracker, user, policy)
}

/// What the kernel says about a user right now: `Some(true)` if anything of
/// theirs is frozen (a stop, or what is left of one — their slice or any
/// cgroup in it reads `cgroup.freeze` 1), `Some(false)` if nothing is, `None`
/// if they have no slice (not logged in) or it can't be read. This — never
/// the agent's intention — is the evidence a restarted agent adopts.
pub fn is_frozen(username: &str) -> Option<bool> {
    let uid = sysusers::uid_of(username)?;
    freeze::read_tree(uid).map(|t| freeze::any_frozen(&t))
}

/// Is a stop of `username` holding — is everything it freezes (see
/// [`freeze`]) frozen right now? `None` without a slice. What gets reported
/// as the device's lock state for someone who is stopped.
pub fn freeze_holds(exec: &Exec, username: &str) -> Option<bool> {
    let uid = sysusers::uid_of(username)?;
    let p = Planner::new(exec, username);
    let tree = freeze::read_tree(uid)?;
    Some(freeze::holds(&tree, &p.plan(&tree, uid)))
}

/// Keep a stop holding, quietly: freeze what appeared since it was applied
/// (an app a timer started, a new login's apps once it has settled), file
/// D-Bus-started apps into scopes, and move an older whole-slice freeze onto
/// the apps. Never escalates. Returns whether the stop holds now.
pub fn refreeze(exec: &Exec, username: &str) -> Option<bool> {
    let uid = sysusers::uid_of(username)?;
    let p = Planner::new(exec, username);
    let tree = freeze::read_tree(uid)?;
    let plan = p.plan(&tree, uid);
    let moved_off_whole = tree.frozen && matches!(plan, freeze::Plan::Apps { .. });
    if exec.dry_run() || (freeze::holds(&tree, &plan) && !moved_off_whole) {
        return Some(freeze::holds(&tree, &plan));
    }
    match apply(exec, uid, username, &tree, &plan) {
        Ok(0) => {}
        Ok(n) => tracing::info!("{username}: froze {n} more of what they run"),
        Err(e) => tracing::debug!("{username}: keeping the stop failed: {e}"),
    }
    let tree = freeze::read_tree(uid)?;
    Some(freeze::holds(&tree, &p.plan(&tree, uid)))
}

/// What a plan needs besides the tree: the person's logins and the desktop
/// apps D-Bus can start.
struct Planner {
    logins: Vec<freeze::Login>,
    apps: Vec<freeze::AppExec>,
}

impl Planner {
    fn new(exec: &Exec, username: &str) -> Self {
        let logins = crate::lock::query_sessions(exec)
            .into_iter()
            .filter(|s| s.user == username && s.class.starts_with("user"))
            .map(|s| freeze::Login {
                age: s.age(),
                graphical: s.graphical,
                id: s.id,
            })
            .collect();
        Planner {
            logins,
            apps: freeze::dbus_apps(),
        }
    }

    fn plan(&self, tree: &freeze::Cgroup, uid: u32) -> freeze::Plan {
        freeze::plan(tree, uid, &self.logins, &self.apps)
    }
}

/// Write `cgroup.freeze` of the cgroup at `rel` (relative to the slice; `""`
/// is the slice itself).
fn write_freeze(uid: u32, rel: &str, on: bool) -> std::io::Result<()> {
    let dir = freeze::slice_dir(uid);
    let dir = if rel.is_empty() { dir } else { dir.join(rel) };
    std::fs::write(dir.join("cgroup.freeze"), if on { "1" } else { "0" })
}

/// Freeze what `plan` says. Returns how many cgroups were newly frozen.
fn apply(
    exec: &Exec,
    uid: u32,
    username: &str,
    tree: &freeze::Cgroup,
    plan: &freeze::Plan,
) -> std::io::Result<usize> {
    let (targets, strays) = match plan {
        freeze::Plan::Whole => {
            if tree.frozen {
                return Ok(0);
            }
            write_freeze(uid, "", true)?;
            return Ok(1);
        }
        freeze::Plan::Apps { targets, strays } => (targets, strays),
    };
    let mut targets = targets.clone();
    for s in strays {
        if let Some(unit) = adopt(exec, uid, username, s) {
            targets.push(format!("user@{uid}.service/app.slice/{unit}"));
        }
    }
    let mut n = 0;
    let mut failed = None;
    for t in &targets {
        if tree.at(t).is_some_and(|c| c.frozen) {
            continue;
        }
        match write_freeze(uid, t, true) {
            Ok(()) => n += 1,
            // It ended in the meantime: nothing left to stop.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                failed.get_or_insert(e);
            }
        }
    }
    if let Some(e) = failed {
        return Err(e);
    }
    // A whole-slice freeze (an older agent's, or a desktop that has since
    // gone) comes off only once the apps are frozen: they never run between.
    if tree.frozen {
        write_freeze(uid, "", false)?;
    }
    Ok(n)
}

/// File an app D-Bus started inside `session.slice` into a scope of its own
/// in `app.slice` — `StartTransientUnit` with its PIDs on the person's own
/// service manager, the call GNOME Shell makes for every app it launches —
/// so that it can be frozen without freezing the session bus. Returns the
/// scope's name once its cgroup is there.
fn adopt(exec: &Exec, uid: u32, username: &str, stray: &freeze::Stray) -> Option<String> {
    let first = *stray.pids.first()?;
    let id: String = stray
        .app
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let unit = format!("app-dbus-{id}-{first}.scope");
    let machine = format!("{username}@");
    let count = stray.pids.len().to_string();
    let pids: Vec<String> = stray.pids.iter().map(u32::to_string).collect();
    let mut args = vec![
        "--user",
        "-M",
        machine.as_str(),
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "StartTransientUnit",
        "ssa(sv)a(sa(sv))",
        unit.as_str(),
        "fail",
        "3",
        "Description",
        "s",
        stray.app.as_str(),
        "Slice",
        "s",
        "app.slice",
        "PIDs",
        "au",
        count.as_str(),
    ];
    args.extend(pids.iter().map(String::as_str));
    args.push("0");
    if let Err(e) = exec.run("busctl", &args) {
        tracing::warn!(
            "could not move {} ({username}) into a scope to stop it: {e}",
            stray.app
        );
        return None;
    }
    tracing::info!(
        "{username}: {} ran inside the session bus; moved it into {unit} to stop it",
        stray.app
    );
    // systemd makes the cgroup a moment after the call returns.
    let dir = freeze::slice_dir(uid).join(format!("user@{uid}.service/app.slice/{unit}"));
    for _ in 0..20 {
        if dir.is_dir() {
            return Some(unit);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Some(unit)
}

/// Whether this host can actually freeze a user — cgroup v2 unified with
/// per-user systemd slices. On cgroup v1 / hybrid / a non-systemd init / many
/// containers / WSL-without-systemd, `cgroup.freeze` does not exist, so a
/// screen-time "lock" would silently do nothing. Callers surface this as a
/// degraded gap rather than reporting a healthy lock over an unfrozen session.
pub fn freezer_usable() -> bool {
    // The unified v2 hierarchy exposes `cgroup.controllers` at the mount root;
    // systemd puts each login under `user.slice/user-<uid>.slice`, where the
    // per-cgroup `cgroup.freeze` file lives. Both present ⇒ we can freeze.
    std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
        && std::path::Path::new("/sys/fs/cgroup/user.slice").exists()
}

/// Stop a user's apps with the cgroup v2 freezer — what [`freeze`] decides,
/// never their session — or thaw everything of theirs. Reversible.
///
/// `hard` controls the fallback when the freezer is unavailable: an admin
/// whole-device lock (`hard = true`) may terminate the session as a last
/// resort, but screen-time enforcement (`hard = false`) must NEVER destroy a
/// kid's unsaved work over a time limit — it logs and stays best-effort.
pub fn freeze_user(exec: &Exec, username: &str, frozen: bool, hard: bool) -> Result<()> {
    let Some(uid) = sysusers::uid_of(username) else {
        anyhow::bail!("unknown user {username}");
    };
    if !frozen {
        thaw(exec, uid, username);
        return Ok(());
    }
    let p = Planner::new(exec, username);
    let tree = freeze::read_tree(uid);
    let plan = tree.as_ref().map(|t| p.plan(t, uid));
    if exec.dry_run() {
        tracing::info!(target: "dry_run", "WOULD FREEZE {username}: {plan:?}");
        return Ok(());
    }
    let applied = match (&tree, &plan) {
        (Some(t), Some(pl)) => apply(exec, uid, username, t, pl),
        _ => write_freeze(uid, "", true).map(|_| 1),
    };
    match applied {
        Ok(_) => {
            let what = match &plan {
                Some(freeze::Plan::Apps { targets, strays })
                    if targets.is_empty() && strays.is_empty() =>
                {
                    "nothing to freeze yet".to_string()
                }
                Some(freeze::Plan::Apps { targets, strays }) => {
                    format!("their apps: {} unit(s)", targets.len() + strays.len())
                }
                _ => "the whole slice".to_string(),
            };
            tracing::info!("user {username} freeze=true ({what})");
            Ok(())
        }
        Err(e) if hard => {
            tracing::warn!("cgroup freeze unavailable ({e}); admin lock falls back to loginctl");
            exec.run("loginctl", &["terminate-user", username])
                .map(|_| ())
        }
        Err(e) => {
            tracing::warn!(
                "cgroup freeze unavailable ({e}); screen-time lock NOT escalating to \
                 terminate-user (would destroy unsaved work)"
            );
            Ok(())
        }
    }
}

/// Thaw everything of theirs: every cgroup in their slice that reads frozen,
/// whatever froze it and whatever the plan says now, the slice itself last
/// (so what a whole-slice freeze held resumes at once).
fn thaw(exec: &Exec, uid: u32, username: &str) {
    if exec.dry_run() {
        tracing::info!(target: "dry_run", "WOULD THAW everything of {username}'s");
        return;
    }
    let mut paths = freeze::read_tree(uid)
        .map(|t| freeze::frozen_paths(&t))
        .unwrap_or_default();
    paths.retain(|p| !p.is_empty());
    paths.reverse();
    paths.push(String::new());
    for p in &paths {
        let _ = write_freeze(uid, p, false);
    }
    tracing::info!("user {username} freeze=false");
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Reading;
    use crate::policy::{Bedtime, ScreenTime};
    use chrono::FixedOffset;
    use std::time::Duration;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(2 * 3600).unwrap()
    }
    /// 2026-09-21 (a Monday) + `day` days, local `h:m`.
    fn local(day: u32, h: u32, m: u32) -> DateTime<FixedOffset> {
        tz().with_ymd_and_hms(2026, 9, 21 + day, h, m, 0).unwrap()
    }
    fn policy(limit: u32, bedtime: Option<(&str, &str)>) -> Policy {
        Policy {
            screen_time: ScreenTime {
                enabled: true,
                daily_limit_minutes: limit,
                bedtime: bedtime.map(|(s, e)| Bedtime {
                    start: s.into(),
                    end: e.into(),
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn reading(boot: Duration, wall: DateTime<FixedOffset>, synced: bool) -> Reading<'static> {
        Reading {
            boot_id: "boot-a",
            boot,
            wall: wall.with_timezone(&Utc),
            ntp_synced: synced,
        }
    }
    const H: Duration = Duration::from_secs(3600);

    #[test]
    fn daily_limit_locks_when_exhausted_and_earned_time_extends_it() {
        let p = policy(60, None);
        let mut t = UsageTracker::new();
        t.roll_to(local(0, 12, 0).date_naive());
        t.add_active("kid", 61 * 60, 1);
        let r = evaluate(&p, &t, "kid", &local(0, 12, 0));
        assert!(matches!(
            r,
            Some(LockReason::DailyLimit {
                used_min: 61,
                limit_min: 60
            })
        ));
        t.add_earned("kid", 15);
        assert!(evaluate(&p, &t, "kid", &local(0, 12, 0)).is_none());
        assert_eq!(t.remaining_minutes("kid", &p), Some(14));
    }

    #[test]
    fn bedtime_verdict_says_when_and_why() {
        let p = policy(0, Some(("21:00", "07:00")));
        let t = UsageTracker::new();
        let v = verdict(&p, &t, "kid", &local(0, 20, 50), false);
        assert!(v.allowed);
        assert_eq!(v.reason, Some(StopReason::Bedtime));
        assert_eq!(v.minutes_left, Some(10));
        assert_eq!(
            evaluate(&p, &t, "kid", &local(0, 23, 0)),
            Some(LockReason::Bedtime)
        );
    }

    #[test]
    fn ledger_survives_a_restart_with_overrides_and_clock() {
        let dir =
            std::env::temp_dir().join(format!("openscreentime-ledger-{}", std::process::id()));
        let path = dir.join("usage_ledger.json");
        let mut t = UsageTracker::new();
        let now = t.advance(&reading(H, local(0, 15, 0), true), &tz());
        t.add_active("kid", 40 * 60, 1);
        assert_eq!(t.grant("cmd-1", "kid", 30, now), Grant::Applied);
        t.save_to(&path);

        let mut back = UsageTracker::load_from(&path);
        assert_eq!(back.used_minutes("kid"), 40);
        assert_eq!(back.earned_minutes("kid"), 30);
        assert!(
            back.override_until("kid", now).is_some(),
            "a parent's grant survives a restart"
        );
        assert_eq!(back.grant("cmd-1", "kid", 30, now), Grant::Duplicate);
        assert_eq!(back.clock, t.clock);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grants_are_idempotent_by_command_id() {
        let mut t = UsageTracker::new();
        let now = local(0, 16, 0).with_timezone(&Utc);
        t.roll_to(local(0, 16, 0).date_naive());
        assert_eq!(t.grant("c1", "kid", 15, now), Grant::Applied);
        // The ack was lost; the server redelivers the same command.
        assert_eq!(t.grant("c1", "kid", 15, now), Grant::Duplicate);
        assert_eq!(t.earned_minutes("kid"), 15);
        // A second, distinct grant does add up.
        assert_eq!(t.grant("c2", "kid", 15, now), Grant::Applied);
        assert_eq!(t.earned_minutes("kid"), 30);
    }

    #[test]
    fn a_grant_beats_bedtime_for_its_minutes_then_expires() {
        let p = policy(60, Some(("21:00", "07:00")));
        let mut t = UsageTracker::new();
        let now = local(0, 21, 30);
        t.roll_to(now.date_naive());
        assert_eq!(evaluate(&p, &t, "kid", &now), Some(LockReason::Bedtime));
        t.grant("g", "kid", 15, now.with_timezone(&Utc));
        let v = verdict(&p, &t, "kid", &now, false);
        assert!(v.allowed && v.override_active);
        assert_eq!(v.minutes_left, Some(15));
        // 15 minutes later it has ended cleanly and bedtime holds again.
        let later = local(0, 21, 45);
        assert_eq!(evaluate(&p, &t, "kid", &later), Some(LockReason::Bedtime));
        assert!(t.peek_override("kid", later.with_timezone(&Utc)).is_none());
    }

    #[test]
    fn an_override_never_shortens_a_longer_one() {
        let mut t = UsageTracker::new();
        let now = local(0, 12, 0).with_timezone(&Utc);
        t.set_override("kid", now + chrono::Duration::minutes(60));
        t.set_override("kid", now + chrono::Duration::minutes(5));
        assert_eq!(
            t.peek_override("kid", now),
            Some(now + chrono::Duration::minutes(60))
        );
    }

    #[test]
    fn the_limit_is_per_person_across_computers() {
        let p = policy(60, None);
        let mut t = UsageTracker::new();
        let now = local(0, 17, 0);
        t.roll_to(now.date_naive());
        t.add_active("kid", 20 * 60, 1);
        t.set_elsewhere(
            "kid",
            Elsewhere {
                day: Some(now.date_naive()),
                used_secs: 40 * 60,
                ..Default::default()
            },
        );
        // 20 here + 40 on the other laptop = the whole hour.
        assert_eq!(t.used_minutes("kid"), 60);
        assert!(matches!(
            evaluate(&p, &t, "kid", &now),
            Some(LockReason::DailyLimit { used_min: 60, .. })
        ));
        // Yesterday's "elsewhere" never applies to today.
        let mut t2 = UsageTracker::new();
        t2.roll_to(now.date_naive());
        t2.set_elsewhere(
            "kid",
            Elsewhere {
                day: Some(now.date_naive() - chrono::Days::new(1)),
                used_secs: 60 * 60,
                ..Default::default()
            },
        );
        assert_eq!(t2.used_minutes("kid"), 0);
        // And a new day drops it.
        t.roll_to(now.date_naive() + chrono::Days::new(1));
        assert_eq!(t.used_minutes("kid"), 0);
    }

    #[test]
    fn own_grants_are_the_larger_of_device_and_server_never_the_sum() {
        let mut t = UsageTracker::new();
        let now = local(0, 17, 0);
        t.roll_to(now.date_naive());
        let here = |earned_here_secs| Elsewhere {
            day: Some(now.date_naive()),
            earned_here_secs,
            ..Default::default()
        };
        // The server credited +15 before the command arrived.
        t.set_elsewhere("kid", here(15 * 60));
        assert_eq!(t.earned_minutes("kid"), 15);
        // The command lands: still 15, not 30.
        t.grant("c1", "kid", 15, now.with_timezone(&Utc));
        assert_eq!(t.earned_minutes("kid"), 15);
        // A device that lost its ledger still knows its grants.
        let mut fresh = UsageTracker::new();
        fresh.roll_to(now.date_naive());
        fresh.set_elsewhere("kid", here(30 * 60));
        assert_eq!(fresh.earned_minutes("kid"), 30);
    }

    #[test]
    fn clock_set_back_does_not_reset_usage() {
        let mut t = UsageTracker::new();
        t.advance(&reading(H, local(1, 18, 0), true), &tz());
        t.add_active("kid", 55 * 60, 1);
        // Clock set back a day while the machine runs (unsynchronized now).
        t.advance(
            &reading(H + Duration::from_secs(60), local(0, 18, 1), false),
            &tz(),
        );
        assert_eq!(t.day(), Some(local(1, 0, 0).date_naive()));
        assert_eq!(t.used_minutes("kid"), 55);
        // Even a SYNCED backward correction never un-rolls the day.
        t.advance(
            &reading(H + Duration::from_secs(120), local(0, 18, 2), true),
            &tz(),
        );
        assert_eq!(t.used_minutes("kid"), 55);
    }

    #[test]
    fn clock_set_forward_by_hand_does_not_mint_a_fresh_day() {
        let p = policy(60, None);
        let mut t = UsageTracker::new();
        t.advance(&reading(H, local(0, 16, 0), true), &tz());
        t.add_active("kid", 60 * 60, 1);
        // Wi-Fi off, clock set to tomorrow 09:00 five minutes later.
        let now = t.advance(
            &reading(H + Duration::from_secs(300), local(1, 9, 0), false),
            &tz(),
        );
        assert_eq!(t.day(), Some(local(0, 0, 0).date_naive()), "no roll");
        assert_eq!(now, local(0, 16, 5).with_timezone(&Utc));
        let now_local = now.with_timezone(&tz());
        assert!(
            evaluate(&p, &t, "kid", &now_local).is_some(),
            "still out of time"
        );
        // Real midnight arrives (by boottime): the day rolls — fairly.
        t.advance(&reading(H + 8 * H, local(1, 17, 0), false), &tz());
        assert_eq!(t.day(), Some(local(1, 0, 0).date_naive()));
        assert_eq!(t.used_minutes("kid"), 0);
    }

    #[test]
    fn days_offline_roll_every_midnight_and_never_lock_the_morning() {
        // No NTP, no server, the same boot for three days (or suspends —
        // boottime includes them). Each real midnight gives a fresh budget.
        let p = policy(60, None);
        let mut t = UsageTracker::new();
        t.advance(&reading(H, local(0, 20, 0), false), &tz());
        t.add_active("kid", 60 * 60, 1);
        for day in 1..=3u32 {
            let boot = H + H * (24 * day - 12); // 08:00 on day `day`
            let now = t.advance(&reading(boot, local(day, 8, 0), false), &tz());
            assert_eq!(t.day(), Some(local(day, 0, 0).date_naive()), "day {day}");
            let now_local = now.with_timezone(&tz());
            assert!(
                evaluate(&p, &t, "kid", &now_local).is_none(),
                "day {day}: the morning starts with a full hour"
            );
            t.add_active("kid", 60 * 60, 1);
        }
    }

    #[test]
    fn a_reboot_the_next_morning_rolls_without_the_server() {
        let mut t = UsageTracker::new();
        t.advance(&reading(H, local(0, 21, 0), true), &tz());
        t.add_active("kid", 30 * 60, 1);
        // Powered off overnight; new boot, network down, clock honest.
        let r = Reading {
            boot_id: "boot-b",
            boot: Duration::from_secs(30),
            wall: local(1, 7, 30).with_timezone(&Utc),
            ntp_synced: false,
        };
        t.advance(&r, &tz());
        assert_eq!(t.day(), Some(local(1, 0, 0).date_naive()));
        assert_eq!(t.used_minutes("kid"), 0);
    }
}
