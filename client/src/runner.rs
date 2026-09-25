//! The `run` subcommand: connect the WS bus (falling back to heartbeat polling),
//! pull per-user policy, apply enforcement continuously, dispatch commands, and
//! stream events. This is the orchestrator that ties every module together.

use crate::client::ServerClient;
use crate::config::{AgentConfig, AgentCtx};
use crate::enforce::{self, screentime};
use crate::lock::{self, Face, LockEvent, LockScreen, ParentKeys};
use crate::policy::Policy;
use crate::protocol::*;
use crate::util::Exec;
use crate::{earn, parentcode, tamper, warn};
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use openscreentime_policy::AgeBracket;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// How often the enforcement tick runs (screen-time accounting granularity).
const TICK: Duration = Duration::from_secs(10);
/// How long after a stop's moment its own tick runs (see `stop_wake`).
const STOP_SLACK: Duration = Duration::from_millis(300);

/// The most one tick may bill. Billing is the measured *awake* time since the
/// last tick (CLOCK_MONOTONIC: a suspended laptop bills nothing); the cap
/// bounds what a stalled agent can bill in one go when it wakes up.
const BILL_CAP: Duration = Duration::from_secs(60);

/// Heads-ups before a stop, in minutes (published as `next_warning_at`) —
/// the same 15/5/1 the companion announces (`warn::THRESHOLDS`).
const WARN_BEFORE_MIN: [i64; 3] = [15, 5, 1];

/// Wall clock this far off the trusted clock is reported (once per episode).
const CLOCK_SKEW_REPORT: chrono::Duration = chrono::Duration::minutes(60);

/// Save-your-work countdown between "the lock decision fired" and the actual
/// freeze, for a stop nobody saw coming (a rule changed, a grant ran out). It
/// is counted down as a notification, never a full-screen takeover. A stop
/// that was announced (the 1-minute warning went out) or that someone logs
/// into gets none: the lock appears at T-0. Admin locks stay immediate.
const FREEZE_GRACE: Duration = Duration::from_secs(60);
/// WS heartbeat (usage push) cadence and the at-least cadence of the `state`
/// frame (CONTRACT-0.4 §5). The enforcement tick itself stays at `TICK`.
const WS_HEARTBEAT: Duration = Duration::from_secs(30);
const STATE_AT_LEAST: Duration = Duration::from_secs(60);
/// How long the HTTP poll fallback runs before trying the WS bus again.
const POLL_ROUND: Duration = Duration::from_secs(60);
/// Reconnect backoff bounds (jittered).
const BACKOFF_MIN_SECS: u64 = 1;
const BACKOFF_MAX_SECS: u64 = 60;
/// A stop counts as announced when its last-minute warning was published this
/// recently.
const ANNOUNCED_WITHIN: Duration = Duration::from_secs(180);

/// Default fail-closed offline grace period: how long the agent tolerates no
/// server contact (WS message or successful poll/heartbeat) before treating
/// itself as offline-beyond-grace. Overridable via `OST_OFFLINE_GRACE_SECS`
/// (no new Cargo dependency — plain env var).
const DEFAULT_OFFLINE_GRACE_SECS: u64 = 900;

fn offline_grace_from_env() -> Duration {
    let secs = std::env::var("OST_OFFLINE_GRACE_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_OFFLINE_GRACE_SECS);
    Duration::from_secs(secs)
}

/// Turn enforcement gaps into `critical` events for the console.
///
/// A device that accepted a policy it cannot enforce is the one case where
/// staying quiet is worse than being noisy: the parent believes filtering is on.
/// The agent's test verdict on a VPN profile, as the event the server's
/// profile row is updated from.
fn vpn_report_event(report: Option<enforce::vpn::VpnReport>) -> Option<Event> {
    report.map(|r| {
        Event::new(
            "vpn_profile",
            if r.ok { SEV_INFO } else { SEV_CRITICAL },
            json!({
                "profile_id": r.profile_id,
                "result": if r.ok { "active" } else { "failed" },
                "error": r.error,
            }),
        )
    })
}

/// The gaps standing now → the `enforcement_degraded` events worth sending:
/// one when a gap appears, none while it stays (the `state` frame carries the
/// standing ones for the console), a fresh one if it comes back after going.
fn new_gap_events(standing: &[String], now: &[(String, String)]) -> Vec<Event> {
    now.iter()
        .filter(|(kind, _)| !standing.contains(kind))
        .map(|(kind, detail)| {
            Event::new(
                EV_ENFORCEMENT_DEGRADED,
                SEV_CRITICAL,
                json!({ "kind": kind, "detail": detail }),
            )
        })
        .collect()
}

/// The standing gap for a network apply that failed outright.
const GAP_NETWORK_APPLY_FAILED: &str = "network_apply_failed";

/// Where the reboot-surviving last-contact wall-clock lives (root-only dir;
/// tampering with it requires root, at which point the game is over anyway).
fn last_contact_path() -> std::path::PathBuf {
    crate::paths::state("last_contact")
}

/// Where the whole-device admin lock is persisted.
///
/// The lock used to live only in memory, so a power-cycle cleared it — while
/// the server kept `devices.status = 'locked'` (heartbeats deliberately never
/// clear it, and an acked `lock` command is never redelivered). A parent locked
/// the device, the kid held the power button, and the machine came back fully
/// usable with the console still showing it locked. That is the same
/// console-disagrees-with-reality failure as the rest of this codebase's
/// history, just pointing the other way.
fn device_locked_path() -> std::path::PathBuf {
    crate::paths::state("device_locked")
}

/// Was the device admin-locked when we last shut down? Absent file = unlocked,
/// which is the right default for a device that has never been locked.
fn load_device_locked() -> bool {
    std::fs::read_to_string(device_locked_path())
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

fn save_device_locked(locked: bool) {
    let path = device_locked_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, if locked { "1" } else { "0" }) {
        // warn, not debug: losing this silently is exactly the bug being fixed.
        tracing::warn!("could not persist device lock state: {e}");
    }
}

/// A parent standing at the device with a valid code, or running `ost unlock`,
/// is an authority a dead server cannot override. `ost unlock` runs in a
/// SEPARATE process, so it records the recovery here and the live agent honors
/// it on its next tick. Without this, the running agent's in-memory
/// `device_locked` re-froze the machine every 30 minutes for as long as the
/// server stayed unreachable — a permanent brick hiding behind the promise
/// that "the parent PIN always unlocks". Root-only state dir: a child cannot
/// forge the marker.
fn local_recovery_marker_path() -> std::path::PathBuf {
    crate::paths::state("local_recovery")
}

/// `(written_at_unix_secs, override_minutes)`. Older markers carry only the
/// timestamp (no override).
fn parse_local_recovery_marker(s: &str) -> Option<(u64, u64)> {
    let mut parts = s.split_whitespace();
    let ts = parts.next()?.parse().ok()?;
    let minutes = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((ts, minutes))
}

fn read_local_recovery_marker() -> Option<(u64, u64)> {
    std::fs::read_to_string(local_recovery_marker_path())
        .ok()
        .and_then(|s| parse_local_recovery_marker(&s))
}

/// Called by `ost unlock` (a separate process): clear the persisted lock so a
/// reboot doesn't reload it, and leave a marker the live agent picks up —
/// with the minutes the parent asked for, so the live agent holds the
/// screen-time rules off for exactly that long (it used to clear the lock and
/// then re-stop the person ~70 s later, while the CLI said "suspended").
pub fn record_local_recovery(minutes: u64) {
    save_device_locked(false);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = local_recovery_marker_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, format!("{now} {minutes}")) {
        tracing::warn!("could not record local recovery: {e}");
    }
}

/// Where the rest of the reboot-surviving enforcement state lives. The freeze
/// set and the save-your-work countdowns used to be memory-only, so holding
/// the power button was a complete reset: a fresh 60-second grace per boot,
/// repeatable all night. `device_locked` was persisted for exactly this
/// reason; these were missed. The lock on screen is recorded here too, so a
/// restarted agent adopts it instead of forgetting it.
fn freeze_state_path() -> std::path::PathBuf {
    crate::paths::state("freeze_state.json")
}

/// Enforcement state that must survive a power-cycle (see [`freeze_state_path()`]).
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct FreezeState {
    /// Users frozen — or already inside the save-your-work countdown — when
    /// this was last saved. Loaded as an *expired* countdown: if they are
    /// still outside policy on their first active tick, the freeze lands
    /// immediately, with no fresh grace.
    #[serde(default)]
    frozen: Vec<String>,
    /// The lock on screen, if any (see `lock::Shown`).
    #[serde(default)]
    lock: Option<lock::Shown>,
    /// A confirmed-evasion lockdown must outlast a reboot too — it is cleared
    /// by a parent PIN or an admin unlock, never by the power button.
    #[serde(default)]
    tamper_lockdown: bool,
    /// Wall-clock at save time. A boot where `now` is *earlier* than this
    /// means the clock was set back while the agent was off — the one clock
    /// cheat the per-tick skew detector structurally cannot see, because
    /// `expected_wall` starts every run as `None`.
    #[serde(default)]
    saved_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The lock the agent last recorded as on screen — for `ost unlock` /
/// `ost recover`, which run in their own process and must take it down too.
pub fn recorded_lock() -> Option<lock::Shown> {
    load_freeze_state().lock
}

fn load_freeze_state() -> FreezeState {
    std::fs::read_to_string(freeze_state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_freeze_state(st: &FreezeState) {
    let path = freeze_state_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let json = match serde_json::to_string(st) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("could not serialize freeze state: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(path, json) {
        // warn, not debug: losing this silently is the power-button bypass.
        tracing::warn!("could not persist freeze state: {e}");
    }
}

/// Load the persisted last-contact wall-clock. A fresh install (no file) gets
/// `now` — the hard-lockdown clock starts at first run, it doesn't punish a
/// brand-new device for history it doesn't have.
fn load_last_contact_wall() -> chrono::DateTime<chrono::Utc> {
    std::fs::read_to_string(last_contact_path())
        .ok()
        .and_then(|s| s.trim().parse::<chrono::DateTime<chrono::Utc>>().ok())
        .unwrap_or_else(chrono::Utc::now)
}

fn save_last_contact_wall(ts: chrono::DateTime<chrono::Utc>) {
    let path = last_contact_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, ts.to_rfc3339()) {
        tracing::debug!("could not persist last-contact timestamp: {e}");
    }
}

/// Server-contact state (TAMPER.md fail-closed offline decision): grace period,
/// then keep the last-known policy fully (and aggressively) enforced — never a
/// hard network blackout, since the device must stay usable under its existing
/// strict allowlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContactState {
    /// Heard from the server within the last tick.
    Online,
    /// No contact for a while, but still within the grace window — not alarming.
    OfflineWithinGrace,
    /// Grace period exceeded: emit one alert, re-assert the last-known policy
    /// every loop so nothing drifts open while the command server is unreachable.
    OfflineFailClosed,
}

pub struct Agent {
    ctx: Arc<AgentCtx>,
    cfg: AgentConfig,
    client: ServerClient,
    exec: Exec,
    /// Effective per-user policies (os_username → Policy).
    policies: HashMap<String, Policy>,
    /// os_username → profile kind (the age bracket id, or a legacy preset
    /// name). Drives overlay wording and the managed-sudo list.
    kinds: HashMap<String, String>,
    /// OS logins whose person sets their own limits (the bundle's
    /// `self_managed`; the adult bracket always counts).
    self_managed: HashSet<String>,
    /// Who the lock went up for, and when — what the self-set snooze's
    /// one-minute wait is measured from.
    lock_since: Option<(String, Instant)>,
    /// The device's unlock-code secret from the last bundle.
    parent_totp_secret: Option<String>,
    /// Unused one-time recovery codes from the last bundle.
    parent_recovery: Vec<crate::policy::RecoveryCode>,
    /// (user, app) → date an `app_blocked` event was already emitted.
    app_reported: HashMap<(String, String), chrono::NaiveDate>,
    /// Standing enforcement gap kinds from the last network apply.
    standing_gaps: Vec<String>,
    /// Reports each tamper / degraded observation of the tick once per
    /// incident, not every ten seconds.
    incidents: tamper::Incidents,
    /// The server retired this computer (it was removed from its household):
    /// nothing is enforced any more — see `retire`.
    retired: bool,
    /// Active seat users as of the last tick.
    active_users: Vec<String>,
    /// The last `state` frame sent, and when — to send on change / at least
    /// every `STATE_AT_LEAST`.
    last_state: Option<DeviceState>,
    last_state_sent: Instant,
    /// Device-level VPN profile from the last policy bundle (None = no tunnel).
    vpn: Option<crate::policy::VpnProfile>,
    tracker: screentime::UsageTracker,
    /// Users currently frozen by screen-time enforcement.
    frozen: HashSet<String>,
    /// Whole-device lock (from a `lock` command).
    device_locked: bool,
    /// Effective tamper level: what the server asked for, within this
    /// computer's ceiling (`tamper::clamp_tamper_level`).
    tamper_level: u8,
    /// The requested level last reported as capped, so a bundle carrying the
    /// same capped request on every pull says so once, not every time.
    tamper_cap_reported: Option<u8>,
    policy_version: String,
    /// This boot's id (the trusted clock's boottime is only valid within it).
    boot_id: String,
    /// Monotonic instant of the last accounting tick — what the next tick
    /// bills from (awake time only; see `BILL_CAP`).
    last_tick: Option<Instant>,
    /// The part of a second the last tick measured but didn't bill: carried,
    /// so whole-second billing neither loses time nor moves a stop later.
    bill_carry: Duration,
    /// When the nearest stop lands that is a fixed moment (see
    /// `update_forecasts`): the tick loop wakes for it, so the stop comes at
    /// the minute the warnings announced, not up to a tick later.
    next_stop_at: Option<Instant>,
    /// Trusted "now" as of the last tick (see `crate::clock`).
    trusted_now: chrono::DateTime<chrono::Utc>,
    /// Users whose time counted on the last tick (present AND active).
    counting: Vec<String>,
    /// Input activity could be read for every present seat on the last tick
    /// (false = counting falls back to presence).
    measured: bool,
    /// Root-side input watcher (evdev, non-exclusive) behind `counting`.
    input: crate::enforce::activity::InputTracker,
    /// (os_username, task_id) → the local date an earn-request was already sent,
    /// so asking twice doesn't spam the server more than once a day
    /// (CONTRACT-PROD.md §4 — the server also dedupes, this just avoids the noise).
    requested_earn: HashMap<(String, String), chrono::NaiveDate>,
    /// Last time the agent successfully reached the server (WS message received
    /// or a successful poll/heartbeat) — the fail-closed offline grace clock.
    last_contact: Instant,
    /// Current offline/online contact state (see `ContactState`).
    contact_state: ContactState,
    /// Configured grace period before we consider ourselves offline-beyond-grace.
    offline_grace: Duration,
    /// Wall-clock of the last successful server contact, persisted to disk so
    /// the offline hard-lockdown threshold (days!) survives reboots — `Instant`
    /// can't, and a device that's been cut off for a week has certainly
    /// rebooted. Loaded at startup, saved (throttled) on contact.
    last_contact_wall: chrono::DateTime<chrono::Utc>,
    /// Last time `last_contact_wall` was flushed to disk (write throttle).
    last_contact_saved: Instant,
    /// Whether the offline hard-lockdown (policy `offline_lockdown_days`
    /// exceeded) is currently engaged — freezes all users like an admin lock;
    /// the parent PIN still always unlocks.
    offline_hard_lockdown: bool,
    /// Whether a *confirmed* evasion attempt (sustained firewall tampering, per
    /// `TamperMonitor`) has locked the device down. Freezes all users like an
    /// admin lock; cleared by an admin unlock or a parent PIN at the machine.
    tamper_lockdown: bool,
    /// The `ost unlock` recovery marker last honored (unix secs), so a marker
    /// left by a previous boot isn't re-applied, and a fresh one is applied once.
    last_local_recovery: Option<u64>,
    /// A console "pause" may carry a save-your-work window: the overlay shows
    /// at once, the freeze lands when this instant passes. None = immediate.
    device_lock_grace_until: Option<Instant>,
    /// Last `local_network_up` probe (set in the offline check, reused after).
    local_net_up: bool,
    /// Once-per-episode gates for the two new warnings.
    clock_skew_reported: bool,
    no_credential_reported: bool,
    /// Confirmation gate that separates a real, sustained evasion attempt from a
    /// transient blip before escalating to `tamper_lockdown`.
    tamper_monitor: tamper::TamperMonitor,
    /// Armed save-your-work countdowns (user → freeze deadline).
    pending_freeze: HashMap<String, Instant>,
    /// The lock screen: its own session on its own VT (see `lock`). Every
    /// freeze and thaw goes through its host, so the lock can't be skipped.
    lock: LockScreen,
    /// What the lock UIs read, and how they wake us.
    lock_shared: lock::SharedRef,
    lock_tx: lock::LockTx,
    /// Taken by `run` to select on.
    lock_rx: Option<mpsc::Receiver<LockEvent>>,
    /// Where the parent-code replay counter / wrong-code lockout live.
    parent_state: std::path::PathBuf,
    /// The next stop per user (what the warnings count down to).
    forecasts: HashMap<String, (warn::StopReason, chrono::DateTime<chrono::Local>)>,
    /// When each user's last-minute warning was published: that stop was
    /// announced, and gets no extra grace.
    announced: HashMap<String, Instant>,
    /// Warnings written to the terminals of users with no desktop.
    tty_warn: HashMap<String, warn::WarnState>,
    /// Active users at the previous tick (`None` before the first), to tell a
    /// fresh login from someone who was already here.
    prev_active: Option<HashSet<String>>,
    /// Events that couldn't be delivered yet (server unreachable). Events are
    /// the audit trail — offline tamper events are exactly the ones that
    /// matter — so failed posts are kept (capped, oldest dropped) and retried
    /// every tick until they land. In-memory only: a restart while offline
    /// loses the buffer, but the outage itself stays visible server-side as
    /// gone-dark time.
    pending_events: Vec<Event>,
    /// Recent user-facing notifications published to the status snapshot for the
    /// per-user tray to deliver as desktop notifications. See [`UserNotification`].
    notifications: VecDeque<UserNotification>,
    /// Monotonic id for the next notification (so the tray shows each once).
    notif_seq: u64,
    /// Sign-in / confirm codes for people on this computer, with the OS logins
    /// each is for (logincode.rs). Published only in those logins' private
    /// status files; dropped when they expire.
    login_codes: Vec<(crate::logincode::LoginCode, Vec<String>)>,
    /// Where-the-time-goes sampler (apps by /proc, sites by dnsmasq log).
    /// Its batches are posted by the network loop, never from the tick.
    attrib: crate::attrib::Attrib,
    /// Ticks since the last usage post (posts every 6 ticks ≈ 1 min).
    attrib_ticks: u32,
    /// Once-per-day dedupe for enforcement probe findings (kind[/user] → day).
    probe_reported: HashMap<String, chrono::NaiveDate>,
    /// Filtering temporarily relaxed because the family DNS upstream is
    /// unreachable (captive portal, a network that blocks public DNS) — so a
    /// kid isn't bricked off wifi entirely. Reported as a degraded gap.
    dns_relaxed: bool,
    /// Consecutive ticks the upstream has been unreachable while a block was in
    /// force — relax only after this is sustained, so a blip doesn't flap.
    dns_unreach_ticks: u32,
    /// Users whose focus hours were blocking their own sites when the network
    /// was last applied; a change re-applies it. `None` = that re-apply failed,
    /// try again.
    focus_applied: Option<Vec<String>>,
}

/// Upper bound on buffered undelivered events (oldest dropped beyond this) —
/// a week of offline ticks must not become an unbounded allocation.
const PENDING_EVENTS_CAP: usize = 512;

/// Max events per `POST /agent/events` request.
///
/// MUST stay <= the server's `MAX_EVENTS` (100, `server/src/agent.rs`), which
/// rejects an oversized batch with a 400. The two constants live in different
/// crates and nothing links them, so the invariant is asserted in tests rather
/// than assumed.
const EVENT_BATCH_MAX: usize = 100;

/// The server's own cap, mirrored here so the invariant is checkable. If
/// `server/src/agent.rs` ever lowers `MAX_EVENTS`, this must follow — the build
/// fails below rather than the fleet silently losing its audit trail.
const SERVER_MAX_EVENTS: usize = 100;
const _: () = assert!(
    EVENT_BATCH_MAX > 0 && EVENT_BATCH_MAX <= SERVER_MAX_EVENTS,
    "EVENT_BATCH_MAX must be within the server's MAX_EVENTS or every event \
     post 400s and the retry buffer can never drain"
);

/// Can we reach the family DNS upstream right now? A 1-second TCP connect to
/// its :53 — resolvers accept TCP, root's socket is allowed past force_dns, and
/// a fast timeout keeps the tick snappy. Used only to decide whether to relax
/// filtering on a network that can't reach it (captive portal / public-DNS
/// block), never as a security signal.
fn upstream_reachable(upstream: &str) -> bool {
    let Ok(ip) = upstream.parse::<std::net::IpAddr>() else {
        return true; // not an IP we can probe — don't relax on that basis
    };
    let addr = std::net::SocketAddr::new(ip, 53);
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(1)).is_ok()
}

/// The per-user on-demand earn-request marker. The kid's tray drops a file here
/// (in its own `/run/user/<uid>`, which only that user and root can touch); the
/// root agent consumes it. Returns `None` if the username has no uid.
fn ondemand_earn_marker(user: &str) -> Option<std::path::PathBuf> {
    let uid = crate::sysusers::uid_of(user)?;
    Some(std::path::PathBuf::from(format!(
        "/run/user/{uid}/openscreentime/earn_request"
    )))
}

/// Atomically write a managed user's private status snapshot: `0600`, chowned to
/// the user so their (unprivileged) tray can read it while no other local user
/// can. Created via `create_new` so the restrictive mode always applies to a
/// fresh file rather than an inherited-perms one.
fn write_private_status(dir: &std::path::Path, user: &str, uid: u32, contents: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = dir.join(format!("status.{user}.json.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut f = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("could not stage status for {user}: {e}");
            return;
        }
    };
    if let Err(e) = f.write_all(contents.as_bytes()) {
        tracing::warn!("could not write status for {user}: {e}");
        return;
    }
    drop(f);
    let _ = std::os::unix::fs::chown(&tmp, Some(uid), None);
    let _ = std::fs::rename(&tmp, dir.join(format!("status.{user}.json")));
}

/// A normal (non-blocking) user-facing message the per-user tray should show as
/// a desktop notification. Full-screen takeovers are reserved for the moments
/// that actually block the screen (a lock, or the freeze countdown); everything
/// else — an approval, a denial, a heads-up — rides this channel. The agent runs
/// as root and has no session bus, so the tray is what actually displays these;
/// the monotonic `id` lets it show each exactly once.
#[derive(Debug, Clone)]
struct UserNotification {
    id: u64,
    title: String,
    body: String,
    critical: bool,
    user: Option<String>,
}

/// How many recent notifications the status snapshot carries. The tray polls
/// every 5s and ticks are 10s, so a handful is plenty of overlap to never miss
/// one; older entries age out.
const NOTIFY_QUEUE_CAP: usize = 16;

impl Agent {
    pub fn new(ctx: Arc<AgentCtx>, cfg: AgentConfig) -> Result<Self> {
        let client = ServerClient::new(&cfg.server_url, &cfg.device_token)?;
        let exec = Exec::new(ctx.clone());
        // Reboot-surviving: the freeze set, challenge-unlock counter and a
        // tamper lockdown must not reset because someone held the power button.
        let carried = load_freeze_state();
        let mut pending_events = Vec::new();
        // `--tamper-max` starts the computer at 3; agent.toml can't raise it
        // past the local ceiling any more than the server can.
        let start_level =
            tamper::clamp_tamper_level(cfg.tamper_level.max(ctx.tamper_max), ctx.tamper_max);
        if start_level.capped() {
            pending_events.push(tamper::tamper_level_capped_event(&start_level));
        }
        if let Some(saved) = carried.saved_at {
            if let Some(ev) = tamper::clock_rollback_event(saved, chrono::Utc::now()) {
                pending_events.push(ev);
            }
        }
        // Pre-expired countdowns: the grace was already granted before the
        // restart. If the user is still outside policy on their first active
        // tick the lock and freeze land immediately; if they are back within
        // policy (a reboot the next morning) the entry is simply disarmed.
        let pending_freeze: HashMap<String, Instant> = carried
            .frozen
            .iter()
            .map(|u| (u.clone(), Instant::now()))
            .collect();
        let (lock_tx, lock_rx) = mpsc::channel(32);
        let lock_shared = lock::shared();
        let host = lock::SystemHost::new(exec.clone(), lock_shared.clone(), lock_tx.clone());
        // A lock this boot's previous run left on screen is adopted, not forgotten.
        let lock = LockScreen::new(Box::new(host), lock_shared.clone(), carried.lock.clone());
        Ok(Agent {
            tamper_level: start_level.applied,
            tamper_cap_reported: start_level.capped().then_some(start_level.requested),
            ctx,
            cfg,
            client,
            exec,
            policies: HashMap::new(),
            kinds: HashMap::new(),
            parent_totp_secret: None,
            parent_recovery: Vec::new(),
            app_reported: HashMap::new(),
            standing_gaps: Vec::new(),
            incidents: tamper::Incidents::default(),
            retired: false,
            active_users: Vec::new(),
            last_state: None,
            last_state_sent: Instant::now(),
            vpn: None,
            // Reboot-surviving: reload the day's usage so a restart can't reset it.
            tracker: screentime::UsageTracker::load(),
            frozen: HashSet::new(),
            // Reboot-surviving: a parent's lock must outlast a power-cycle.
            device_locked: load_device_locked(),
            policy_version: String::new(),
            boot_id: crate::clock::boot_id(),
            last_tick: None,
            bill_carry: Duration::ZERO,
            next_stop_at: None,
            trusted_now: chrono::Utc::now(),
            counting: Vec::new(),
            measured: true,
            input: crate::enforce::activity::InputTracker::new(),
            requested_earn: HashMap::new(),
            self_managed: HashSet::new(),
            lock_since: None,
            last_contact: Instant::now(),
            contact_state: ContactState::Online,
            offline_grace: offline_grace_from_env(),
            last_contact_wall: load_last_contact_wall(),
            last_contact_saved: Instant::now(),
            offline_hard_lockdown: false,
            tamper_lockdown: carried.tamper_lockdown,
            last_local_recovery: read_local_recovery_marker().map(|(ts, _)| ts),
            device_lock_grace_until: None,
            local_net_up: true,
            clock_skew_reported: false,
            no_credential_reported: false,
            tamper_monitor: tamper::TamperMonitor::new(),
            pending_freeze,
            lock,
            lock_shared,
            lock_tx,
            lock_rx: Some(lock_rx),
            parent_state: parentcode::state_path(),
            forecasts: HashMap::new(),
            announced: HashMap::new(),
            tty_warn: HashMap::new(),
            prev_active: None,
            pending_events,
            notifications: VecDeque::new(),
            notif_seq: 0,
            login_codes: Vec::new(),
            attrib: crate::attrib::Attrib::new(),
            attrib_ticks: 0,
            probe_reported: HashMap::new(),
            dns_relaxed: false,
            dns_unreach_ticks: 0,
            focus_applied: Some(Vec::new()),
        })
    }

    /// Publish a normal (non-blocking) desktop notification for the per-user
    /// companion to deliver. `user = None` means device-wide. Someone with no
    /// desktop at all hears it on their own terminals — never anyone else's
    /// (the old `wall` reached every terminal on the machine).
    fn notify_user(&mut self, user: Option<&str>, title: &str, body: &str, critical: bool) {
        self.notif_seq += 1;
        self.notifications.push_back(UserNotification {
            id: self.notif_seq,
            title: title.to_string(),
            body: body.to_string(),
            critical,
            user: user.map(str::to_string),
        });
        while self.notifications.len() > NOTIFY_QUEUE_CAP {
            self.notifications.pop_front();
        }
        tracing::info!("notify {}: {title} — {body}", user.unwrap_or("everyone"));
        if let Some(u) = user {
            let sessions = self.lock.host().sessions();
            if !lock::has_graphical_session(&sessions, u) {
                self.lock.host().tell_ttys(u, &format!("{title} — {body}"));
            }
        }
    }

    /// What the lock needs to verify a parent at this machine: the device's
    /// unlock-code secret and recovery codes plus this user's backup-code hash.
    fn parent_keys(&self, policy: &Policy) -> ParentKeys {
        ParentKeys {
            pin_hash: policy.parent_pin_hash.clone(),
            totp_secret: self.parent_totp_secret.clone(),
            recovery: self.parent_recovery.clone(),
        }
    }

    /// The age bracket a user's profile kind maps to (legacy presets: `kids`
    /// → Kid, `teen` → YoungerTeen, `default`/unknown → Adult-ish handling
    /// falls to Kid for overlay wording, since only managed users see one).
    fn bracket_of(&self, user: &str) -> AgeBracket {
        match self.kinds.get(user).map(String::as_str) {
            Some("kids") => AgeBracket::Kid,
            Some("teen") => AgeBracket::YoungerTeen,
            Some("adult") | Some("default") => AgeBracket::Adult,
            Some(k) => AgeBracket::parse(k).unwrap_or(AgeBracket::Kid),
            None => AgeBracket::Kid,
        }
    }

    /// The honest device state (CONTRACT-0.4 §5). `locked` is derived from the
    /// kernel freezer, never from what we meant to do.
    fn device_state(&self) -> DeviceState {
        let lock_intent = self.device_locked || self.offline_hard_lockdown || self.tamper_lockdown;
        let mut frozen_users: Vec<String> = if self.exec.dry_run() {
            self.frozen.iter().cloned().collect()
        } else {
            self.policies
                .keys()
                .filter(|u| screentime::is_frozen(u) == Some(true))
                .cloned()
                .collect()
        };
        frozen_users.sort();
        // Locked = the lock is meant AND every managed user who is actually
        // here is frozen (nobody here at all also counts: whoever logs in is
        // frozen on their first tick).
        let present: Vec<&String> = self
            .active_users
            .iter()
            .filter(|u| self.policies.contains_key(*u))
            .collect();
        let locked =
            lock_intent && (present.is_empty() || present.iter().all(|u| frozen_users.contains(u)));
        // A host that can't freeze isn't enforcing screen time even with no
        // network gaps — surface it as a gap so the console isn't a green lie.
        let mut gaps = self.standing_gaps.clone();
        let screen_time_active = self
            .policies
            .values()
            .any(|p| p.screen_time.enabled && p.screen_time.daily_limit_minutes > 0);
        if screen_time_active && !self.exec.dry_run() && !screentime::freezer_usable() {
            gaps.push("screen_time_no_freezer".to_string());
        }
        DeviceState {
            locked,
            lock_intent,
            frozen_users,
            enforcing: !self.policies.is_empty() && gaps.is_empty(),
            gaps,
            agent_version: crate::client::AGENT_VERSION.to_string(),
            active_users: self.active_users.clone(),
            features: FEATURES.iter().map(|f| f.to_string()).collect(),
            overrides: self
                .policies
                .keys()
                .filter_map(|u| {
                    self.tracker
                        .peek_override(u, self.trusted_now)
                        .map(|t| (u.clone(), t))
                })
                .collect(),
        }
    }

    /// The `state` frame to send now, if it changed or is due. Records it as sent.
    fn state_frame_due(&mut self, force: bool) -> Option<AgentFrame> {
        let st = self.device_state();
        let due = force
            || self.last_state.as_ref() != Some(&st)
            || self.last_state_sent.elapsed() >= STATE_AT_LEAST;
        if !due {
            return None;
        }
        self.last_state = Some(st.clone());
        self.last_state_sent = Instant::now();
        Some(AgentFrame::State { state: st })
    }

    /// Queue events for delivery by the network loop (`flush_queued`). The
    /// enforcement tick never waits on the network to report what it did.
    fn queue_events(&mut self, fresh: Vec<Event>) {
        self.pending_events.extend(fresh);
        self.cap_pending_events();
    }

    /// Keep the undelivered-event buffer bounded (oldest dropped). Delivery
    /// is in server-sized batches (`EVENT_BATCH_MAX`) — posting the whole
    /// buffer in one request once meant a buffer past the server's cap could
    /// never drain, silently discarding the offline audit trail.
    fn cap_pending_events(&mut self) {
        if self.pending_events.len() > PENDING_EVENTS_CAP {
            let excess = self.pending_events.len() - PENDING_EVENTS_CAP;
            self.pending_events.drain(..excess);
        }
    }

    /// Record successful server contact (WS message received, or a successful
    /// poll/heartbeat). Resets the fail-closed offline clock and (throttled)
    /// persists the wall-clock for the reboot-surviving hard-lockdown timer.
    fn record_contact(&mut self) {
        self.last_contact = Instant::now();
        self.last_contact_wall = chrono::Utc::now();
        if self.last_contact_saved.elapsed() > Duration::from_secs(60) {
            self.last_contact_saved = Instant::now();
            save_last_contact_wall(self.last_contact_wall);
        }
    }

    /// A parent has proven themselves AT the device (a valid code at the lock,
    /// or `ost unlock`). That is the authority the whole-device
    /// locks defer to, so clear every one of them persistently — the admin
    /// lock, the offline hard-lockdown and the confirmed-evasion lockdown — not
    /// just a 30-minute grace that let the lock reassert itself for as long as
    /// the server stayed dead. Resets the hard-lockdown clock too, otherwise
    /// the next tick would re-engage it on the same stale last-contact.
    fn local_recovery(&mut self, source: &str) -> Vec<Event> {
        let mut events = Vec::new();
        let was_locked = self.device_locked || self.offline_hard_lockdown || self.tamper_lockdown;
        self.device_locked = false;
        save_device_locked(false);
        self.device_lock_grace_until = None;
        self.offline_hard_lockdown = false;
        self.tamper_lockdown = false;
        // A parent at the machine counts as contact for the days-scale clock.
        self.last_contact_wall = chrono::Utc::now();
        save_last_contact_wall(self.last_contact_wall);
        for user in self.frozen.drain().collect::<Vec<_>>() {
            self.lock.host().freeze(&user, false, false);
        }
        self.pending_freeze.clear();
        if !self.exec.dry_run() {
            self.persist_freeze_state();
        }
        if was_locked {
            tracing::warn!("whole-device lock cleared locally via {source}");
            events.push(Event::new(
                EV_UNLOCK,
                SEV_INFO,
                json!({ "source": "local_recovery", "via": source }),
            ));
        }
        events
    }

    /// The device-wide offline hard-lockdown threshold: the strictest (smallest
    /// non-zero) `lockdown.offline_lockdown_days` across all managed users.
    /// 0 = feature off.
    fn offline_lockdown_days(&self) -> u32 {
        // Floored: the strictest user governs the whole device, so a stray `1`
        // on one profile plus a flaky self-hosted server would strand every
        // kid's device on the first outage.
        const MIN_OFFLINE_LOCKDOWN_DAYS: u32 = 3;
        self.policies
            .values()
            .map(|p| p.lockdown.offline_lockdown_days)
            .filter(|d| *d > 0)
            .min()
            .map(|d| d.max(MIN_OFFLINE_LOCKDOWN_DAYS))
            .unwrap_or(0)
    }

    /// Escalation past the fail-closed grace: a device that hasn't reached the
    /// command server for `offline_lockdown_days` DAYS is treated as tampered-
    /// with (SIM pulled, DNS blackholed, firewall boxed…) and freezes every
    /// user like an admin lock. The parent PIN always unlocks — a dead VPS can
    /// never permanently brick the family's laptop.
    fn offline_hard_lockdown_check(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let days = self.offline_lockdown_days();
        // Only a device that is ONLINE but can't reach OUR server should count
        // down to a hard-lockdown (a real "someone cut it off from us" signal).
        // A laptop that's simply off the network for a week — a holiday, a dead
        // router, no wifi — must not freeze the whole family; that's an innocent
        // outage, not tampering. Without this, one self-hosted-server outage
        // (ISP/VPS/cert) would strand every kid's device at once.
        let local_net_up = tamper::local_network_up(&self.exec);
        self.local_net_up = local_net_up;
        let past_threshold = days > 0
            && local_net_up
            && (chrono::Utc::now() - self.last_contact_wall).num_days() >= i64::from(days);
        // A lockdown that has NO offline way back is a brick, not a defense:
        // the unlock code lives on the console this device can't reach. Only
        // engage when a recovery credential (recovery codes / backup code) is
        // on the device; otherwise say so, loudly, and don't.
        let engaged = past_threshold
            && if parentcode::Verifier::from_device().configured() {
                true
            } else {
                if !self.no_credential_reported {
                    self.no_credential_reported = true;
                    events.push(tamper::tamper_event(
                        "offline_lockdown_no_credential",
                        SEV_CRITICAL,
                        "offline hard-lockdown threshold reached but this device has no offline \
                         unlock credential (no recovery codes / backup code) — NOT locking, \
                         because nobody could get back in. Generate recovery codes in the console.",
                    ));
                }
                false
            };
        if engaged && !self.offline_hard_lockdown {
            events.push(tamper::tamper_event(
                "offline_hard_lockdown",
                SEV_CRITICAL,
                &format!(
                    "no server contact since {} (threshold {days}d) — device locked; \
                     the parent PIN unlocks",
                    self.last_contact_wall.format("%Y-%m-%d %H:%M UTC")
                ),
            ));
        } else if !engaged && self.offline_hard_lockdown {
            events.push(tamper::tamper_event(
                "offline_hard_lockdown_lifted",
                SEV_INFO,
                "server contact resumed — offline hard-lockdown lifted",
            ));
        }
        self.offline_hard_lockdown = engaged;
        events
    }

    /// Fail-closed offline check: once `offline_grace` has elapsed since the
    /// last successful server contact, emit a `network_offline` tamper event
    /// (once per offline episode) and aggressively re-assert the last-known
    /// network policy every tick so nothing drifts open while the command
    /// server is unreachable. Never blacks out traffic — the device stays
    /// usable under its existing strict allowlist. Emits `network_online` once
    /// when contact resumes after having exceeded the grace period.
    fn offline_grace_check(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let elapsed = self.last_contact.elapsed();
        if elapsed > self.offline_grace {
            if self.contact_state != ContactState::OfflineFailClosed {
                events.push(tamper::tamper_event(
                    "network_offline",
                    SEV_WARN,
                    &format!(
                        "no server contact for {}s (grace {}s exceeded); re-asserting \
                         last-known policy every loop — fail-closed, not a blackout",
                        elapsed.as_secs(),
                        self.offline_grace.as_secs()
                    ),
                ));
            }
            self.contact_state = ContactState::OfflineFailClosed;
            // Aggressively re-assert the last-known policy (dns + firewall +
            // resolv pin) so nothing drifts open while unreachable.
            events.extend(self.apply_network().0);
        } else {
            if self.contact_state == ContactState::OfflineFailClosed {
                events.push(tamper::tamper_event(
                    "network_online",
                    SEV_INFO,
                    "server contact resumed after exceeding the offline grace period",
                ));
            }
            self.contact_state = if elapsed <= TICK {
                ContactState::Online
            } else {
                ContactState::OfflineWithinGrace
            };
        }
        events
    }

    /// Boot-time enforcement: tamper hardening + initial policy pull + apply.
    pub async fn bootstrap(&mut self) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        // Tamper level 1+ hardening that we own at runtime (unit/watchdog are systemd).
        tamper::install_polkit(&self.exec, self.tamper_level)?;
        if self.tamper_level >= 3 {
            tamper::apply_level3_tty_lockdown(&self.exec)?;
            events.push(tamper::level3_boot_guidance_event());
        }
        tamper::touch_heartbeat(&self.exec);

        match self.client.get_policy().await {
            Ok(bundle) => {
                self.record_contact();
                events.extend(self.apply_bundle(bundle)?)
            }
            Err(e) => {
                // Fail closed, not open. Booting with an empty policy map means
                // enforcing nothing — and it also zeroes offline_lockdown_days,
                // which is read from that map, so the "cut off from the server"
                // countermeasure is disabled by exactly the condition it exists
                // to catch. Re-apply the last known bundle instead and let the
                // next successful pull replace it.
                tracing::warn!("initial policy pull failed ({e}); falling back to cached bundle");
                match crate::policy::load_bundle_cache() {
                    Ok(cached) => {
                        let version = cached.policy_version.clone();
                        events.extend(self.apply_bundle(cached)?);
                        tracing::warn!(
                            "enforcing cached policy v{version} until the server answers"
                        );
                        events.push(Event::new(
                            EV_POLICY_APPLIED,
                            SEV_WARN,
                            json!({
                                "policy_version": version,
                                "source": "cache",
                                "detail": "server unreachable at boot; re-applied the last known \
                                           policy from disk rather than starting unenforced",
                            }),
                        ));
                    }
                    // Genuinely nothing to enforce: never enrolled, or the
                    // cache was removed. Say so loudly — this device is open.
                    Err(ce) => tracing::error!(
                        "no cached policy to fall back on ({ce}); device is UNENFORCED until \
                         the server is reachable"
                    ),
                }
            }
        }
        Ok(events)
    }

    /// Store a policy bundle and (re)apply the network-level enforcement.
    fn apply_bundle(&mut self, bundle: crate::policy::PolicyBundle) -> Result<Vec<Event>> {
        let cacheable = bundle.clone();
        self.policy_version = bundle.policy_version.clone();
        // The bundle only ever raises the level (a `set_tamper_level` command
        // is how it comes down), and never past this computer's ceiling.
        let mut tamper_events = Vec::new();
        if bundle.device_tamper_level > self.tamper_level {
            let (_, evs, polkit) = self.adopt_tamper_level(bundle.device_tamper_level);
            if let Err(e) = polkit {
                tracing::warn!(
                    "polkit rule not updated for level {}: {e}",
                    self.tamper_level
                );
            }
            tamper_events = evs;
        }
        self.policies.clear();
        self.kinds.clear();
        self.self_managed.clear();
        for up in bundle.users {
            if up.self_managed {
                self.self_managed.insert(up.os_username.clone());
            }
            self.kinds.insert(up.os_username.clone(), up.profile_kind);
            self.policies.insert(up.os_username, up.policy);
        }
        self.vpn = bundle.vpn;
        let parent_code = bundle.parent_code.unwrap_or_default();
        self.parent_totp_secret = Some(parent_code.totp_secret).filter(|s| !s.is_empty());
        self.parent_recovery = parent_code.recovery_codes;
        // sudo on this machine: managed users authenticate with the unlock code.
        let users_by_kind: Vec<(String, String)> = self
            .kinds
            .iter()
            .map(|(u, k)| (u.clone(), k.clone()))
            .collect();
        crate::service::sync_managed_sudoers(&self.exec, &users_by_kind);
        // DNS/nftables are host-global: apply the most restrictive effective
        // policy. A computer that can't filter the network (no dnsmasq, no
        // nftables) still gets everything else: the rules are held, cached,
        // and screen time is enforced — loudly degraded, never half-applied.
        let effective = self.effective_network_policy();
        self.focus_applied = Some(self.focus_now());
        let (net_events, _) = self.apply_network();
        // Best-effort cache so `ost unlock` can work without a live
        // agent process or server connection (parent PIN + recovery teardown).
        crate::policy::save_cache(&effective);
        // …and the whole bundle, so a reboot while the server is unreachable
        // re-enforces the last known policy instead of coming up wide open.
        // Cached verbatim — rebuilding it from `self.policies` would silently
        // drop `profile_kind`.
        crate::policy::save_bundle_cache(&cacheable);
        tracing::info!(
            "policy v{} applied for {} user(s), {} gap(s)",
            self.policy_version,
            self.policies.len(),
            self.standing_gaps.len()
        );
        // "Applied" is reported alongside, not instead of, the gaps: the policy
        // really was written, it just isn't all being enforced.
        let mut events = vec![Event::new(
            EV_POLICY_APPLIED,
            SEV_INFO,
            json!({
                "policy_version": self.policy_version,
                "users": self.policies.len(),
                "dns_gaps": self.standing_gaps.len(),
            }),
        )];
        events.extend(net_events);
        events.extend(tamper_events);
        Ok(events)
    }

    /// This computer was removed from its household (the server said so,
    /// twice): free everyone and take the rules off (`crate::retire`).
    /// Thaw first, then the lock comes down — back to their own session —
    /// then the network rules, then the rest outside the sandbox.
    async fn retire(&mut self) {
        if self.retired {
            return;
        }
        self.retired = true;
        tracing::warn!(
            "this computer was removed from its household — taking OpenScreenTime off it"
        );
        let mut people: HashSet<String> = self.frozen.iter().cloned().collect();
        people.extend(self.pending_freeze.keys().cloned());
        people.extend(self.policies.keys().cloned());
        if !self.exec.dry_run() {
            people.extend(
                crate::sysusers::login_users()
                    .into_iter()
                    .map(|u| u.username),
            );
        }
        let mut people: Vec<String> = people.into_iter().collect();
        people.sort();
        for user in &people {
            self.lock.host().freeze(user, false, false);
        }
        self.frozen.clear();
        self.pending_freeze.clear();
        self.device_locked = false;
        self.device_lock_grace_until = None;
        self.tamper_lockdown = false;
        self.offline_hard_lockdown = false;
        self.policies.clear();
        self.kinds.clear();
        self.self_managed.clear();
        self.lock.release();
        if !self.exec.dry_run() {
            save_device_locked(false);
            self.persist_freeze_state();
        }
        crate::retire::teardown_enforcement(&self.exec);
        crate::retire::mark(&self.exec);
        crate::retire::spawn_helper(&self.exec);
    }

    /// Apply the network side of the effective policy (DNS, firewall, VPN).
    /// Never aborts the caller: an apply that fails outright is a standing
    /// gap like any other. Updates the standing gaps and returns the events
    /// worth sending (a gap when it appears, the VPN verdict), and whether
    /// the apply ran through.
    fn apply_network(&mut self) -> (Vec<Event>, bool) {
        let effective = self.effective_network_policy();
        let server_host = crate::client::server_host(&self.cfg.server_url);
        let mut events = Vec::new();
        let (gaps, ok): (Vec<(String, String)>, bool) = match enforce::apply_network_policy(
            self.ctx.clone(),
            &self.exec,
            server_host.as_deref(),
            &effective,
            &enforce::vpn::VpnState::Sync(self.vpn.as_ref()),
        ) {
            Ok((gaps, report)) => {
                events.extend(vpn_report_event(report));
                let gaps = gaps
                    .iter()
                    .map(|g| (g.kind().to_string(), g.explain().to_string()))
                    .collect();
                (gaps, true)
            }
            Err(e) => {
                tracing::error!("network policy not applied: {e:#}");
                let detail = format!(
                    "this computer's network rules could not be applied ({e:#}); \
                     screen time is still enforced"
                );
                (vec![(GAP_NETWORK_APPLY_FAILED.to_string(), detail)], false)
            }
        };
        events.extend(new_gap_events(&self.standing_gaps, &gaps));
        self.standing_gaps = gaps.into_iter().map(|(kind, _)| kind).collect();
        (events, ok)
    }

    /// Our nft table was loaded by the last apply (no firewall gap stands).
    fn firewall_loaded(&self) -> bool {
        !self
            .standing_gaps
            .iter()
            .any(|g| g.starts_with("firewall_") || g == GAP_NETWORK_APPLY_FAILED)
    }

    /// Move to the tamper level the server asked for, within this computer's
    /// ceiling, and re-apply that level's hardening. A request above the
    /// ceiling is capped and reported (once per requested level) — never
    /// applied, and never dropped without a word. The last value is whether
    /// the level's polkit rule could be written.
    fn adopt_tamper_level(
        &mut self,
        requested: u8,
    ) -> (tamper::TamperLevel, Vec<Event>, Result<()>) {
        let level = tamper::clamp_tamper_level(requested, self.ctx.tamper_max);
        let mut events = Vec::new();
        if level.capped() {
            if self.tamper_cap_reported != Some(level.requested) {
                tracing::warn!(
                    "server asked for tamper level {}; running at {} (level 3 needs --tamper-max)",
                    level.requested,
                    level.applied
                );
                events.push(tamper::tamper_level_capped_event(&level));
                self.tamper_cap_reported = Some(level.requested);
            }
        } else {
            self.tamper_cap_reported = None;
        }
        let raised_to_3 = level.applied >= 3 && self.tamper_level < 3;
        self.tamper_level = level.applied;
        let polkit = tamper::install_polkit(&self.exec, self.tamper_level);
        if raised_to_3 {
            let _ = tamper::apply_level3_tty_lockdown(&self.exec);
            events.push(tamper::level3_boot_guidance_event());
        }
        (level, events, polkit)
    }

    /// Merge every user's network policy into ONE host-global ruleset — dnsmasq
    /// and the nft table are per-host, not per-user, so a shared machine must
    /// serve the tightest of everyone present or the strictest child's
    /// protections silently evaporate (this used to be a coin-flip: after the
    /// allow-by-default flip every preset keyed identically, so `min_by_key` on
    /// a HashMap returned an arbitrary, restart-varying winner).
    ///
    /// "Tightest" is now computed field by field, deterministically:
    /// - blocks, `dns.blocklist`: union (most restrictive);
    /// - `safe_search`: on if any user needs it;
    /// - `dns.upstream`: the most-filtering family resolver present;
    /// - lockdown flags: on if any user sets them;
    /// - the base (firewall ports etc.) comes from a stable pick: a managed
    ///   user first, then lowest username, so it never changes across restarts.
    fn effective_network_policy(&self) -> Policy {
        // Deterministic base: managed (screen-time on) beats unmanaged; ties
        // break on the username so the choice is stable across restarts.
        let mut base = self
            .policies
            .iter()
            .min_by(|(ua, pa), (ub, pb)| {
                let managed = |p: &Policy| !p.screen_time.enabled; // false (managed) sorts first
                managed(pa).cmp(&managed(pb)).then_with(|| ua.cmp(ub))
            })
            .map(|(_, p)| p.clone())
            .unwrap_or_default();

        // How much a family resolver filters, for "most-filtering wins".
        let upstream_rank = |u: &str| match u {
            "1.1.1.3" => 3, // malware + adult
            "1.1.1.2" => 2, // malware
            _ => 1,
        };

        let mut blocks = crate::policy::AppBlocks::default();
        let mut blocklist: Vec<String> = base.dns.blocklist.clone();
        let now = self.local_now();
        for p in self.policies.values() {
            blocks.apps.extend(p.blocks.apps.iter().cloned());
            blocks
                .categories
                .extend(p.blocks.categories.iter().cloned());
            blocks
                .custom_domains
                .extend(p.blocks.custom_domains.iter().cloned());
            // Sites someone blocks for themselves, inside their focus hours
            // (all day without hours) — a block like any other while it holds.
            if crate::policy::rules::focus_blocking(&p.focus, &now) {
                blocks.custom_domains.extend(p.focus.sites.iter().cloned());
            }
            blocklist.extend(p.dns.blocklist.iter().cloned());
            // Any user who needs it turns it on for the shared host.
            base.dns.safe_search |= p.dns.safe_search;
            base.lockdown.force_dns |= p.lockdown.force_dns;
            base.lockdown.block_doh |= p.lockdown.block_doh;
            base.lockdown.block_dot |= p.lockdown.block_dot;
            base.lockdown.block_tor |= p.lockdown.block_tor;
            base.lockdown.block_vpn |= p.lockdown.block_vpn;
            if upstream_rank(&p.dns.upstream) > upstream_rank(&base.dns.upstream) {
                base.dns.upstream = p.dns.upstream.clone();
            }
        }
        for v in [
            &mut blocks.apps,
            &mut blocks.categories,
            &mut blocks.custom_domains,
            &mut blocklist,
        ] {
            v.sort();
            v.dedup();
        }
        base.blocks = blocks;
        base.dns.blocklist = blocklist;

        // CONTRACT-0.6 §1/§4: what is blocked must be *really* blocked. A block
        // realized only as a dnsmasq sinkhole is theater the moment a browser
        // resolves names elsewhere — a plaintext alt-resolver or DoH. So the
        // presence of ANY block implies the anti-bypass posture: force all
        // plaintext DNS through our resolver, and drop DoT + the known DoH
        // providers. (DoH to an arbitrary IP pinned by hand still costs more
        // than it should — see docs/TAMPER.md, which says so honestly.)
        if (!base.blocks.is_empty() || !base.dns.blocklist.is_empty()) && !self.dns_relaxed {
            base.lockdown.force_dns = true;
            base.lockdown.block_doh = true;
            base.lockdown.block_dot = true;
        }
        base
    }

    /// Whether the effective policy wants to force DNS (i.e. has any block).
    fn wants_force_dns(&self) -> bool {
        let now = self.local_now();
        self.policies.values().any(|p| {
            !p.blocks.is_empty()
                || !p.dns.blocklist.is_empty()
                || crate::policy::rules::focus_blocking(&p.focus, &now)
        })
    }

    /// The trusted clock, as local wall time (what focus hours are read in).
    fn local_now(&self) -> chrono::NaiveDateTime {
        self.trusted_now.with_timezone(&chrono::Local).naive_local()
    }

    /// Who has their self-blocked sites blocked right now (sorted).
    fn focus_now(&self) -> Vec<String> {
        let now = self.local_now();
        let mut users: Vec<String> = self
            .policies
            .iter()
            .filter(|(_, p)| crate::policy::rules::focus_blocking(&p.focus, &now))
            .map(|(u, _)| u.clone())
            .collect();
        users.sort();
        users
    }

    /// Focus hours began or ended for someone since the network was last
    /// applied: the host-global DNS must follow. Records the new state.
    fn focus_flipped(&mut self) -> bool {
        let now = self.focus_now();
        if self.focus_applied.as_ref() == Some(&now) {
            return false;
        }
        self.focus_applied = Some(now);
        true
    }

    /// Keep DNS from bricking a device off a captive-portal / public-DNS-
    /// blocking network. If a block is in force but the family upstream can't
    /// be reached for a sustained stretch, relax the force-DNS posture and say
    /// so (degraded); restore it the moment the upstream answers again.
    /// Returns events + whether the relaxed state flipped (⇒ re-apply network).
    fn update_dns_reachability(&mut self) -> (Vec<Event>, bool) {
        let mut events = Vec::new();
        if self.exec.dry_run() || !self.wants_force_dns() {
            // Nothing to force; make sure we're not stuck relaxed.
            if self.dns_relaxed {
                self.dns_relaxed = false;
                self.dns_unreach_ticks = 0;
                return (events, true);
            }
            return (events, false);
        }
        let upstream = self.effective_network_policy().dns.upstream;
        let reachable = upstream_reachable(&upstream);
        let mut flipped = false;
        if reachable {
            self.dns_unreach_ticks = 0;
            if self.dns_relaxed {
                self.dns_relaxed = false;
                flipped = true;
                events.push(tamper::tamper_event(
                    "dns_filter_restored",
                    SEV_INFO,
                    "family DNS is reachable again — filtering restored",
                ));
            }
        } else {
            self.dns_unreach_ticks = self.dns_unreach_ticks.saturating_add(1);
            // ~30s of sustained unreachability before relaxing.
            if self.dns_unreach_ticks >= 3 && !self.dns_relaxed {
                self.dns_relaxed = true;
                flipped = true;
                events.push(Event::new(
                    EV_ENFORCEMENT_DEGRADED,
                    SEV_WARN,
                    json!({
                        "kind": "dns_upstream_unreachable",
                        "message": "couldn't reach the family DNS — filtering temporarily relaxed so this computer can get online"
                    }),
                ));
            }
        }
        (events, flipped)
    }

    /// The periodic enforcement tick: screen-time accounting + lockout + tamper
    /// re-assertion + heartbeat. Returns events to emit.
    /// Per-user usage snapshot for the ledger (CONTRACT-PROD.md §5), keyed on the
    /// users we hold policy for. Shared by the WS `heartbeat` frame and the poll
    /// HTTP heartbeat so both paths report identically.
    fn usage_snapshot(&self) -> Vec<crate::client::UsageReport> {
        let offset = self
            .trusted_now
            .with_timezone(&chrono::Local)
            .offset()
            .local_minus_utc();
        self.policies
            .keys()
            .map(|u| {
                // Only this device's own use: the server sums the person.
                let here = self.tracker.used_here_secs(u);
                crate::client::UsageReport {
                    os_username: u.clone(),
                    used_minutes_today: u32::try_from(here / 60).unwrap_or(u32::MAX),
                    used_seconds_today: here,
                    day: self.tracker.day(),
                    utc_offset_secs: Some(offset),
                }
            })
            .collect()
    }

    /// The server's answer to a usage report: the person's day elsewhere, and
    /// its clock. Offline, the last answer keeps applying (it is persisted in
    /// the ledger, tagged with its day).
    fn apply_person_days(
        &mut self,
        server_time: Option<chrono::DateTime<chrono::Utc>>,
        users: Vec<PersonDay>,
    ) {
        // The server's clock is the fallback truth for a device whose own
        // clock isn't NTP-synchronized (no NTP, or set by hand). A synced
        // clock is at least as good — and flipping between two sources that
        // disagree could roll the day early.
        if let Some(t) = server_time {
            let reading = crate::clock::read(&self.boot_id);
            if !reading.ntp_synced {
                self.tracker.clock.observe_server(&reading, t);
            }
        }
        for p in users {
            self.tracker.set_elsewhere(
                &p.os_username,
                screentime::Elsewhere {
                    day: Some(p.day),
                    used_secs: p.used_elsewhere_secs,
                    earned_secs: p.earned_elsewhere_secs,
                    earned_here_secs: p.earned_here_secs,
                },
            );
        }
    }

    /// Hold the rules off for `users` until `until` (one override per user,
    /// persisted in the ledger right away).
    fn override_users(&mut self, users: &[String], until: chrono::DateTime<chrono::Utc>) {
        for u in users {
            self.tracker.set_override(u, until);
        }
        if !self.exec.dry_run() {
            self.tracker.save();
        }
    }

    /// The next local midnight on the trusted clock ("until end of day").
    fn end_of_day(&self) -> chrono::DateTime<chrono::Utc> {
        use chrono::TimeZone;
        let now = self.trusted_now.with_timezone(&chrono::Local);
        let tomorrow = now.date_naive() + chrono::Days::new(1);
        chrono::Local
            .from_local_datetime(&tomorrow.and_time(chrono::NaiveTime::MIN))
            .earliest()
            .map(|t| t.with_timezone(&chrono::Utc))
            .unwrap_or(self.trusted_now + chrono::Duration::hours(24))
    }

    async fn enforcement_tick(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if self.retired {
            return events;
        }
        tamper::touch_heartbeat(&self.exec);

        // The trusted clock (crate::clock): the NTP-synced wall clock, the
        // server's clock, or boottime extrapolation — never a hand-set wall
        // clock. It drives the day roll and every rule below. A wall clock that
        // disagrees with it by a lot was moved by hand; say so once per episode
        // (suspend doesn't trip this — boottime includes the time asleep).
        let reading = crate::clock::read(&self.boot_id);
        self.trusted_now = self.tracker.advance(&reading, &chrono::Local);
        let skew = self.tracker.clock.wall_skew(&reading);
        if skew.abs() > CLOCK_SKEW_REPORT {
            if !self.clock_skew_reported {
                self.clock_skew_reported = true;
                events.push(tamper::tamper_event(
                    "clock_skew",
                    SEV_WARN,
                    &format!(
                        "the system clock is {} min {} the trusted time; screen time keeps \
                         counting on the trusted clock",
                        skew.num_minutes().abs(),
                        if skew > chrono::Duration::zero() {
                            "ahead of"
                        } else {
                            "behind"
                        }
                    ),
                ));
            }
        } else {
            self.clock_skew_reported = false;
        }

        // Keep DNS filtering from bricking a captive-portal / public-DNS-
        // blocking network; re-apply the (relaxed or restored) policy on a flip.
        let (dns_events, dns_flipped) = self.update_dns_reachability();
        events.extend(dns_events);
        // Focus hours began or ended: someone's own blocked sites come or go.
        let focus_flipped = self.focus_flipped();
        if (dns_flipped || focus_flipped) && !self.exec.dry_run() {
            let (evs, ok) = self.apply_network();
            events.extend(evs);
            if !ok && focus_flipped {
                // Try the focus change again next tick.
                self.focus_applied = None;
            }
        }

        // Tamper re-assertion (resolv.conf / nft drift, NM disconnect). What
        // it sees goes to the monitor raw, every tick; what is *reported* is
        // each incident once.
        let mut observed = tamper::reassert_all(&self.exec, self.firewall_loaded());
        if let Some(ev) = tamper::nm_guard_probe(&self.exec) {
            observed.push(ev);
        }
        let raw_tamper: Vec<String> = observed
            .iter()
            .filter(|e| e.ev_type == EV_TAMPER)
            .filter_map(|e| e.payload.get("kind").and_then(|k| k.as_str()))
            .map(str::to_string)
            .collect();
        let resolver_went = observed.iter().any(|e| {
            e.ev_type == EV_ENFORCEMENT_DEGRADED
                && e.payload.get("kind").and_then(|k| k.as_str())
                    == Some(tamper::KIND_RESOLVER_STOPPED)
        });
        events.extend(self.incidents.report(observed));

        // reassert_all flags a missing nft table (critical event) but can't
        // rebuild it — it has no policy. Repair it here with the effective
        // policy so a flush/delete can't leave the device with NO firewall
        // (fail-open) until the next full policy apply. The same re-apply
        // brings a resolver that stopped back up (it restarts dnsmasq) and
        // records the gap if it can't.
        // `Some(true)` only — if the probe itself couldn't run (`None`),
        // applying a ruleset through the same broken spawn path won't work
        // either; the reassert above already reported it, retry next tick.
        let flushed = raw_tamper.iter().any(|k| k == "nft_flush");
        if (flushed || resolver_went) && !self.exec.dry_run() {
            let (evs, ok) = self.apply_network();
            if ok && flushed {
                tracing::info!("nft table was missing — re-applied firewall");
            }
            events.extend(evs);
        }

        // Fail-closed offline grace: alert + aggressively re-assert last-known
        // policy once we've gone too long without hearing from the server.
        events.extend(self.offline_grace_check());
        // …and the days-scale escalation on top of it (policy-configurable).
        // `ost unlock` ran in another process: honor its recovery marker once.
        if let Some((ts, minutes)) = read_local_recovery_marker() {
            if self.last_local_recovery != Some(ts) {
                self.last_local_recovery = Some(ts);
                events.extend(self.local_recovery("ost unlock"));
                // …and hold the screen-time rules off for the minutes the
                // parent asked for, for everyone on this machine.
                if minutes > 0 {
                    let users: Vec<String> = self.policies.keys().cloned().collect();
                    let until =
                        self.trusted_now + chrono::Duration::minutes(minutes.min(24 * 60) as i64);
                    self.override_users(&users, until);
                }
            }
        }
        events.extend(self.offline_hard_lockdown_check());

        // Confirm sustained evasion (vs. a transient blip) and escalate to a
        // whole-device lockdown. We feed the monitor the tamper-signal kinds
        // seen this tick — raw, including the ones not re-reported; a kind
        // that crosses its confirmation threshold is a real attempt (the
        // "check it's real, not a packet drop" gate).
        let kinds: Vec<&str> = events
            .iter()
            .filter(|e| e.ev_type == EV_TAMPER)
            .filter_map(|e| e.payload.get("kind").and_then(|k| k.as_str()))
            .chain(raw_tamper.iter().map(String::as_str))
            .collect();
        let confirmed = self.tamper_monitor.observe(&kinds);
        if !confirmed.is_empty() && !self.tamper_lockdown {
            self.tamper_lockdown = true;
            tracing::warn!("tamper lockdown engaged: {}", confirmed.join(", "));
            events.push(tamper::tamper_event(
                "evasion_confirmed",
                SEV_CRITICAL,
                &format!(
                    "confirmed evasion attempt ({}) — device locked; the parent PIN unlocks",
                    confirmed.join(", ")
                ),
            ));
        }

        // Screen-time accounting: who is at a seat, and whose minute is real
        // use (recent input or sound — enforce::activity). Bill the measured
        // awake time since the last tick, not a fixed 10 s.
        self.input.rescan();
        let activity = crate::enforce::activity::sample(&crate::enforce::activity::SystemProbe {
            exec: &self.exec,
            input: &self.input,
        });
        let active = activity.present.clone();
        self.active_users = active.clone();
        self.measured = activity.measured;
        let now_mono = Instant::now();
        let (elapsed, carry) =
            whole_seconds(billable_elapsed(self.last_tick, now_mono) + self.bill_carry);
        self.bill_carry = carry;
        self.last_tick = Some(now_mono);
        // A frozen user is NOT spending screen time: their processes are
        // suspended at the lock screen, but logind still reports the seat
        // "active", so counting them burned budget while locked — silently
        // eating an earn-time grant ("granted, but still locked").
        let counting: Vec<String> = activity
            .billable
            .iter()
            .filter(|u| !self.frozen.contains(*u))
            .cloned()
            .collect();
        for user in &counting {
            self.tracker
                .add_active(user, elapsed.as_secs() as u32, self.ctx.time_accel);
        }
        self.counting = counting.clone();

        // Where the time goes (CONTRACT-0.6): sample running catalog apps for
        // the users whose time is counting (an app idling in an idle session
        // is not time spent), and tail the resolver's query log. The batches
        // are posted by the network loop, never from this tick.
        {
            let uids: std::collections::HashMap<String, u32> = counting
                .iter()
                .filter_map(|u| crate::sysusers::uid_of(u).map(|id| (u.clone(), id)))
                .collect();
            self.attrib.sample_apps(&uids, elapsed.as_secs() as i64);
            self.attrib.ingest_dns_log();
            self.attrib_ticks += 1;
            if self.attrib_ticks >= 6 {
                self.attrib_ticks = 0;
                // The lock must never lie (CONTRACT-0.6 §4): probe that the
                // stops we believe in are real, on a 1-minute cadence.
                events.extend(self.probe_enforcement());
            }
        }

        // Blocked apps with a native client: deny their processes (CONTRACT-0.4 §7).
        events.extend(enforce::apps::deny(
            &self.exec,
            &self.policies,
            &mut self.app_reported,
        ));
        // Consider every user we have a policy for (so we can also UNfreeze).
        let users: Vec<String> = self.policies.keys().cloned().collect();
        for user in users {
            let policy = self.policies.get(&user).cloned().unwrap_or_default();
            let is_active = active.contains(&user);
            let currently_frozen = self.frozen.contains(&user);
            // Frozen means frozen — every tick. A slice that reads thawed (a
            // re-login made a new one, or someone wrote 0) is stopped again —
            // through the lock, so nobody meets a silent frozen desktop.
            if currently_frozen && self.lock.host().is_frozen(&user) == Some(false) {
                let hard = self.device_lock_effective();
                self.stop_user(&user, hard).await;
            }

            // An active parent override (a grant, a code at the lock screen,
            // `ost unlock`, a Resume) holds the screen-time rules off for this
            // user. It does not beat a pause — every source that should lift a
            // pause clears it directly (`local_recovery`, `unlock`).
            let in_grace = self
                .tracker
                .override_until(&user, self.trusted_now)
                .is_some();

            // Evaluate when the user is at the machine — and also when they are
            // already frozen, even if their session has gone inactive.
            //
            // Treating "inactive" as "no verdict" made a frozen user unfreeze
            // the moment their session stopped being the active one, and every
            // re-lock re-armed the full FREEZE_GRACE. On a machine with a
            // second session (a sibling, or just the greeter on another VT),
            // flipping away and back yielded a fresh ~60 seconds of usable time
            // per flip, repeatable all night, each cycle logging an ordinary
            // looking lockout event. Bedtime and the daily limit are properties
            // of the clock and the ledger, not of who currently holds the seat.
            //
            // Still gated on `is_active` for users who are NOT frozen, so an
            // absent user is never newly frozen (and never shown an overlay)
            // just for existing in the policy.
            // Bedtime / allowed-window rules are about the clock, not the
            // seat: an SSH-only login (Remote=yes, never a "seat") used to
            // escape them entirely. Evaluate those for every policy user.
            let has_clock_rule = policy.screen_time.enabled
                && (policy.screen_time.bedtime.is_some()
                    || !policy.screen_time.schedule.is_empty());
            let lock = if should_evaluate_screen_time(
                in_grace,
                is_active,
                currently_frozen,
                has_clock_rule,
            ) {
                screentime::evaluate(
                    &policy,
                    &self.tracker,
                    &user,
                    &self.trusted_now.with_timezone(&chrono::Local),
                )
            } else {
                None
            };
            if lock.is_none() {
                // Lock reason cleared while a save-your-work countdown was
                // armed (e.g. time credited): disarm it. A freeze carried over
                // from before a restart is disarmed the same way — rebooting
                // into a new day within policy is not an evasion.
                self.pending_freeze.remove(&user);
            }

            // A pause beats an override (policy::rules): a parent who gives
            // "+30" and then pauses means the pause. A code typed at the
            // device, `ost unlock` and a console Resume clear the lock itself.
            // (An admin lock with a save-your-work window isn't effective
            // until the window closes; the other whole-device locks are
            // immediate.)
            let effective_device_locked = self.device_lock_effective();
            match decide_freeze(effective_device_locked, lock.as_ref(), currently_frozen) {
                FreezeAction::Freeze => {
                    if effective_device_locked {
                        // Overrides screen-time and is immediate (and may
                        // hard-fall-back to session termination — it's an
                        // explicit parent action / tamper response).
                        self.stop_user(&user, true).await;
                    } else if let Some(reason) = &lock {
                        self.screen_time_lockout(&user, reason, &mut events).await;
                    }
                }
                FreezeAction::Unfreeze => {
                    // Policy now allows (and no admin lock is active): thaw.
                    // The lock comes down after the loop — thaw first, always.
                    self.lock.host().freeze(&user, false, false);
                    self.frozen.remove(&user);
                    tracing::info!("{user} unlocked (within policy again)");
                    // The verdict's time — the stop the warnings will count
                    // down to — never the budget, which a grant on a day
                    // already over the limit leaves short of it.
                    let v = self.stop_verdict(&user, &policy);
                    let body = back_words(&v);
                    self.notify_user(Some(&user), "You're back", &body, false);
                }
                FreezeAction::None => {}
            }
        }

        // The next stop per user: published for the companion's 15/5/1-minute
        // warnings, and written to the terminals of anyone with no desktop.
        self.update_forecasts(&active);

        // On-demand "request more time" markers dropped by users' companions.
        self.check_ondemand_earn().await;

        // Sign-in codes run out after a few minutes; stop publishing them.
        let now = chrono::Utc::now();
        self.login_codes.retain(|(c, _)| c.is_live(now));

        // The lock follows the freeze set: up in front of whoever is stopped
        // and on screen, down (after the thaw above) once they are not.
        self.reconcile_lock().await;
        self.prev_active = Some(active.iter().cloned().collect());

        // Persist the freeze/grant state every tick, like the usage ledger
        // above — a power-cycle at any moment must resume, not reset.
        if !self.exec.dry_run() {
            self.tracker.save();
            self.persist_freeze_state();
        }
        self.write_status_file();
        events
    }

    /// Probe that enforcement is real, not just logged (CONTRACT-0.6 §4).
    ///
    /// Two active checks, each finding deduped to once per day:
    /// - every user we believe frozen must read back frozen from the kernel
    ///   freezer — a lock screen over an unfrozen session is a lie;
    /// - if anything is blocked, the system resolver must actually sinkhole a
    ///   blocked domain — `getent hosts` walks the same path the child's apps
    ///   do, so a routable answer means the block is theater.
    fn probe_enforcement(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if self.exec.dry_run() {
            return events;
        }
        let today = chrono::Local::now().date_naive();
        let report = |me: &mut HashMap<String, chrono::NaiveDate>, key: String, msg: String| {
            if me.get(&key) == Some(&today) {
                return None;
            }
            me.insert(key.clone(), today);
            tracing::error!("enforcement probe: {msg}");
            Some(Event::new(
                EV_ENFORCEMENT_DEGRADED,
                SEV_CRITICAL,
                json!({ "kind": key, "message": msg }),
            ))
        };

        // 0. Can this host freeze at all? On cgroup v1 / non-systemd / many
        // containers the freezer is absent, so a screen-time lock silently
        // no-ops — and `is_frozen` returns None, not Some(false), so check #1
        // below can't see it. If any user is actually screen-time managed here,
        // that's a degraded state the console must show, not a green lie.
        let screen_time_active = self
            .policies
            .values()
            .any(|p| p.screen_time.enabled && p.screen_time.daily_limit_minutes > 0);
        if screen_time_active && !screentime::freezer_usable() {
            if let Some(ev) = report(
                &mut self.probe_reported,
                "screen_time_no_freezer".to_string(),
                "this computer can't actually stop a session (no cgroup2 freezer) — \
                 screen-time limits are not being enforced here"
                    .to_string(),
            ) {
                events.push(ev);
            }
        }

        // 1. Frozen means frozen — and stays frozen. A recreated slice
        // (re-login, linger toggle) or a manual `echo 0` is reported here; the
        // per-user loop right after re-stops it, through the lock, so nobody
        // meets a silent frozen desktop.
        for user in self.frozen.clone() {
            if screentime::is_frozen(&user) == Some(false) {
                if let Some(ev) = report(
                    &mut self.probe_reported,
                    format!("freeze_ineffective:{user}"),
                    format!(
                        "{user} was found running while they should be stopped — stopped again"
                    ),
                ) {
                    self.notify_user(
                        None,
                        "A stop isn't holding",
                        &format!(
                            "{user}'s screen should be stopped but isn't — check the console."
                        ),
                        true,
                    );
                    events.push(ev);
                }
            }
        }

        // 2. Blocked means blocked. One domain per probe round is enough —
        //    the sinkhole is one dnsmasq config; if one entry fails they all do.
        let probe_domain = self.policies.values().find_map(|p| {
            openscreentime_policy::catalog::expand(&p.blocks)
                .domains
                .first()
                .cloned()
                .or_else(|| {
                    p.dns.blocklist.first().map(|d| {
                        d.trim_start_matches("*.")
                            .trim_start_matches('.')
                            .to_string()
                    })
                })
        });
        if let Some(domain) = probe_domain.filter(|d| !d.is_empty()) {
            if let Some(out) = self.exec.try_probe("getent", &["hosts", &domain]) {
                let answered_routable = out.lines().any(|l| {
                    let addr = l.split_whitespace().next().unwrap_or("");
                    // Sinkhole answers, in every form glibc may render them —
                    // including the v4-mapped-v6 shape ::ffff:0.0.0.0, which
                    // must NOT be read as routable (that was a false
                    // "the lock isn't biting" on a perfectly healthy device).
                    !addr.is_empty()
                        && addr != "0.0.0.0"
                        && addr != "::"
                        && addr != "127.0.0.1"
                        && addr != "::1"
                        && addr != "::ffff:0.0.0.0"
                        && !addr.ends_with(":0.0.0.0")
                });
                if answered_routable {
                    if let Some(ev) = report(
                        &mut self.probe_reported,
                        "sinkhole_ineffective".to_string(),
                        format!(
                            "blocked domain {domain} still resolves — the DNS block is not biting"
                        ),
                    ) {
                        events.push(ev);
                    }
                }
            }
        }
        events
    }

    /// Snapshot the reboot-surviving enforcement state to disk. Users inside a
    /// save-your-work countdown are recorded as frozen on purpose: the grace
    /// was already granted, and a reboot mid-countdown must not re-arm it.
    fn persist_freeze_state(&self) {
        let mut frozen: Vec<String> = self
            .frozen
            .iter()
            .chain(self.pending_freeze.keys())
            .cloned()
            .collect();
        frozen.sort();
        frozen.dedup();
        save_freeze_state(&FreezeState {
            frozen,
            lock: self.lock.shown().cloned(),
            tamper_lockdown: self.tamper_lockdown,
            saved_at: Some(chrono::Utc::now()),
        });
    }

    /// A screen-time stop. The first tick with a lock reason emits the event
    /// and decides how long the person gets to save their work: nothing more
    /// if the stop was announced (its 1-minute warning went out) or they only
    /// just logged in into it; otherwise the bracket's grace, counted down by
    /// the companion as a notification — never a full-screen takeover. Then
    /// the lock goes up and the session is frozen. Never terminates the
    /// session. Someone who isn't logged in is not stopped: they meet the lock
    /// when they log in.
    async fn screen_time_lockout(
        &mut self,
        user: &str,
        reason: &screentime::LockReason,
        events: &mut Vec<Event>,
    ) {
        if !self.lock.host().logged_in(user) {
            return;
        }
        match self.pending_freeze.get(user).copied() {
            None => {
                let bracket = self.bracket_of(user);
                let grace = if self.stop_was_announced(user) || self.just_logged_in(user) {
                    Duration::ZERO
                } else {
                    FREEZE_GRACE.max(Duration::from_secs(u64::from(bracket.wind_down_secs())))
                };
                let sev = if matches!(reason, screentime::LockReason::Bedtime) {
                    SEV_WARN
                } else {
                    SEV_INFO
                };
                events.push(
                    Event::new(
                        EV_SCREEN_TIME_EXCEEDED,
                        sev,
                        json!({
                            "reason": reason.headline(),
                            "detail": reason.detail(),
                            "freeze_grace_secs": grace.as_secs(),
                            "bracket": bracket.id(),
                        }),
                    )
                    .for_user(user),
                );
                if grace.is_zero() {
                    self.stop_user(user, false).await;
                } else {
                    self.pending_freeze
                        .insert(user.to_string(), Instant::now() + grace);
                }
            }
            // The countdown ran out — or it was carried over from before a
            // restart, already spent.
            Some(deadline) if deadline <= Instant::now() => {
                self.pending_freeze.remove(user);
                self.stop_user(user, false).await;
            }
            Some(_) => {} // countdown still running
        }
    }

    fn stop_was_announced(&self, user: &str) -> bool {
        self.announced
            .get(user)
            .is_some_and(|t| t.elapsed() <= ANNOUNCED_WITHIN)
    }

    fn just_logged_in(&self, user: &str) -> bool {
        self.prev_active
            .as_ref()
            .is_some_and(|prev| !prev.contains(user))
    }

    /// A whole-device lock in force right now: a parent's pause (once its
    /// save-your-work window closed), the offline hard-lockdown, or a
    /// confirmed evasion attempt.
    fn device_lock_effective(&self) -> bool {
        let admin = self.device_locked
            && self
                .device_lock_grace_until
                .is_none_or(|t| Instant::now() >= t);
        admin || self.offline_hard_lockdown || self.tamper_lockdown
    }

    /// Stop `user` now. If they are the one on screen, the lock goes up in
    /// front of them first — switching away while their compositor is still
    /// alive — and only then is their whole slice frozen. Someone who isn't
    /// logged in is never frozen; someone with no desktop is told on their own
    /// terminals. If no lock can be shown they are NOT frozen (a frozen desktop
    /// with nothing on it is a brick, not a lock) and the console hears why.
    async fn stop_user(&mut self, user: &str, hard: bool) {
        if !self.lock.host().logged_in(user) {
            return;
        }
        let sessions = self.lock.host().sessions();
        let in_front = lock::on_screen_user(&sessions).as_deref() == Some(user)
            || self.lock.subject() == Some(user);
        if in_front {
            let face = self.face_for(user);
            if !self.lock.present(user, face).await {
                self.lock_unavailable(user);
                return;
            }
            self.note_lock_subject();
        } else if !lock::has_graphical_session(&sessions, user) {
            let face = self.face_for(user);
            self.lock.host().tell_ttys(
                user,
                &format!("{} — this session is stopping now.", face.title),
            );
        }
        self.lock.host().freeze(user, true, hard);
        self.frozen.insert(user.to_string());
        if !self.exec.dry_run() {
            self.persist_freeze_state();
        }
    }

    /// Once a day: the lock couldn't be shown, so a stop was not enforced.
    fn lock_unavailable(&mut self, user: &str) {
        let today = chrono::Local::now().date_naive();
        let key = format!("lock_screen_unavailable:{user}");
        if self.probe_reported.get(&key) == Some(&today) {
            return;
        }
        self.probe_reported.insert(key, today);
        tracing::error!("no lock could be shown for {user}; their session keeps running");
        self.pending_events.push(Event::new(
            EV_ENFORCEMENT_DEGRADED,
            SEV_CRITICAL,
            json!({
                "kind": "lock_screen_unavailable",
                "message": format!(
                    "{user} should be stopped, but no lock screen could be shown on this \
                     computer, so their session was left running rather than frozen behind \
                     a blank screen. Re-run `ost install-service` on the device."
                ),
            }),
        ));
    }

    /// The rules' stop reason for `user` right now, on the trusted clock (an
    /// active override already counts: it returns `None`).
    fn rules_now(&self, user: &str, policy: &Policy) -> Option<screentime::LockReason> {
        screentime::evaluate(
            policy,
            &self.tracker,
            user,
            &self.trusted_now.with_timezone(&chrono::Local),
        )
    }

    /// The rules' verdict for `u` with the whole-device lock folded in: a
    /// pause is a stop, and a console pause with a save-your-work window is a
    /// stop that is coming. What the status snapshot publishes and what the
    /// warnings count down to.
    fn stop_verdict(
        &self,
        u: &str,
        p: &Policy,
    ) -> openscreentime_policy::rules::Verdict<chrono::Local> {
        use openscreentime_policy::rules::StopReason;
        let now = self.trusted_now.with_timezone(&chrono::Local);
        let mut v = screentime::verdict(p, &self.tracker, u, &now, false);
        if self.device_locked || self.offline_hard_lockdown || self.tamper_lockdown {
            let pending = self
                .device_lock_grace_until
                .filter(|_| !self.offline_hard_lockdown && !self.tamper_lockdown)
                .map(|t| t.saturating_duration_since(Instant::now()))
                .filter(|d| !d.is_zero());
            match pending {
                Some(d) => {
                    let at = now + chrono::Duration::from_std(d).unwrap_or_default();
                    if v.allowed && v.stop_at.is_none_or(|s| at < s) {
                        v.stop_at = Some(at);
                        v.reason = Some(StopReason::Paused);
                        v.minutes_left = Some(d.as_secs().div_ceil(60) as u32);
                    }
                }
                None => {
                    v.allowed = false;
                    v.reason = Some(StopReason::Paused);
                    v.stop_at = Some(now);
                    v.minutes_left = Some(0);
                    v.resume_at = None;
                }
            }
        }
        v
    }

    /// The next stop per user — what the companion's 15/5/1-minute warnings
    /// count down to (published as `stop_at`/`reason`), remembered here to
    /// know which stops were announced, and written to the terminals of
    /// anyone here with no desktop (their only way to hear it).
    fn update_forecasts(&mut self, active: &[String]) {
        let now = self.trusted_now.with_timezone(&chrono::Local);
        let mut next: HashMap<String, (warn::StopReason, chrono::DateTime<chrono::Local>)> =
            HashMap::new();
        for (u, p) in &self.policies {
            if self.frozen.contains(u) {
                continue;
            }
            let f = if let Some(deadline) = self.pending_freeze.get(u) {
                // The save-your-work countdown of a stop that already tripped.
                let reason = self
                    .rules_now(u, p)
                    .map(|r| stop_reason_of(&r))
                    .unwrap_or(warn::StopReason::Limit);
                let left = deadline.saturating_duration_since(Instant::now());
                Some((
                    reason,
                    now + chrono::Duration::from_std(left).unwrap_or_default(),
                ))
            } else {
                let v = self.stop_verdict(u, p);
                match (v.allowed, v.reason, v.stop_at) {
                    (true, Some(r), Some(at)) => Some((r, at)),
                    _ => None,
                }
            };
            if let Some(f) = f {
                next.insert(u.clone(), f);
            }
        }
        for (u, (_, at)) in &next {
            if (*at - now).num_seconds() <= 90 {
                self.announced.insert(u.clone(), Instant::now());
            }
        }
        // The nearest stop that is a fixed moment — a clock rule, the end of
        // an override, a pause's window, a countdown, or a limit being used
        // up right now (an idle person's limit slides later every tick).
        self.next_stop_at = next
            .iter()
            .filter(|(u, (reason, _))| {
                *reason != warn::StopReason::Limit
                    || self.counting.contains(*u)
                    || self.pending_freeze.contains_key(*u)
            })
            .filter_map(|(_, (_, at))| (*at - now).to_std().ok())
            .min()
            .map(|d| Instant::now() + d);
        let here: Vec<String> = active
            .iter()
            .filter(|u| self.policies.contains_key(*u))
            .cloned()
            .collect();
        if !here.is_empty() {
            let sessions = self.lock.host().sessions();
            for u in here {
                if lock::has_graphical_session(&sessions, &u) {
                    continue; // their companion warns them
                }
                let counting = self.counting.contains(&u);
                let st = self.tty_warn.entry(u.clone()).or_default();
                match next.get(&u) {
                    // An idle person's limit slides later every tick: no
                    // time to announce yet (warn::WarnState::observe_stop).
                    Some((warn::StopReason::Limit, _))
                        if !counting && !self.pending_freeze.contains_key(&u) => {}
                    Some((reason, at)) => {
                        let secs = (*at - now).num_seconds();
                        if st.observe_stop(*reason, secs, at.timestamp()).is_some() {
                            let w = warn::words(*reason, secs, Some(*at));
                            self.lock
                                .host()
                                .tell_ttys(&u, &format!("{} {}", w.title, w.body));
                        }
                    }
                    None => st.clear(),
                }
            }
        }
        self.forecasts = next;
    }

    /// The person sets their own limits: the bundle says so, or they're an adult.
    fn is_self_set(&self, user: &str) -> bool {
        self.self_managed.contains(user) || self.bracket_of(user) == AgeBracket::Adult
    }

    /// Why `user` is stopped, for the lock's words.
    fn stop_of(&self, user: &str, policy: &Policy) -> Option<lock::Stop> {
        use lock::Stop;
        use openscreentime_policy::rules::StopReason;
        if self.tamper_lockdown {
            return Some(Stop::Tamper);
        }
        if self.offline_hard_lockdown {
            return Some(Stop::Offline);
        }
        if self.device_locked {
            return Some(Stop::Paused);
        }
        let now = self.trusted_now.with_timezone(&chrono::Local);
        let v = screentime::verdict(policy, &self.tracker, user, &now, false);
        let until = v.resume_at.map(|t| warn::until_words(t, now));
        Some(match v.reason.filter(|_| !v.allowed)? {
            StopReason::Limit => Stop::Limit {
                minutes: policy.screen_time.daily_limit_minutes + self.tracker.earned_minutes(user),
                back: v.resume_at.map(|t| warn::back_words(t, now)),
            },
            StopReason::Bedtime => Stop::Bedtime { until },
            StopReason::OutsideHours => Stop::OutsideHours { until },
            StopReason::Paused => Stop::Paused,
        })
    }

    /// Everything the lock says to `user` right now.
    fn face_for(&self, user: &str) -> Face {
        use lock::{AskState, CodeState, Look, Stop};
        let policy = self.policies.get(user).cloned().unwrap_or_default();
        let verifier = self
            .parent_keys(&policy)
            .verifier()
            .with_state_path(self.parent_state.clone());
        let code = lock::code_state(&verifier);
        let self_set = self.is_self_set(user);
        let stop = self.stop_of(user, &policy);
        let (look, title, detail) = match &stop {
            Some(s) => lock::stop_words(s, self_set),
            // Frozen, and the rules allow again: the thaw is a moment away.
            None => (
                Look::Wall,
                "This computer is stopped for now".into(),
                String::new(),
            ),
        };
        let own_rules = matches!(
            stop,
            Some(Stop::Limit { .. } | Stop::Bedtime { .. } | Stop::OutsideHours { .. })
        );
        // A child asks a parent; someone who set their own limits has nobody
        // to ask (they get the snooze instead), and a pause or a device-wide
        // stop isn't about time.
        let can_ask = !self_set && self.bracket_of(user).can_request_time() && own_rules;
        let today = chrono::Local::now().date_naive();
        let asked = self
            .requested_earn
            .iter()
            .any(|((u, _), d)| u == user && *d == today);
        let ask = match (can_ask, asked) {
            (false, _) => AskState::Hidden,
            (true, true) => AskState::Sent,
            (true, false) => AskState::Ready,
        };
        let back = match &stop {
            Some(Stop::Limit { back, .. }) => back.clone(),
            _ => {
                let now = self.trusted_now.with_timezone(&chrono::Local);
                screentime::verdict(&policy, &self.tracker, user, &now, false)
                    .resume_at
                    .map(|t| warn::back_words(t, now))
            }
        };
        let snooze = lock::snooze_state(
            self_set,
            own_rules,
            self.lock_waited(user),
            self.tracker.snoozes(user),
            back,
        );
        let (help, code_hint) = lock::way_out(self_set, code != CodeState::Unavailable);
        Face {
            look,
            title,
            detail,
            who: user.to_string(),
            code,
            ask,
            snooze,
            help: help.to_string(),
            code_hint: code_hint.to_string(),
        }
    }

    /// Seconds the lock has been up in front of `user` (0 if it isn't).
    fn lock_waited(&self, user: &str) -> u64 {
        self.lock_since
            .as_ref()
            .filter(|(u, _)| u == user)
            .map(|(_, t)| t.elapsed().as_secs())
            .unwrap_or(0)
    }

    /// Remember when the lock went up in front of whom (the snooze's wait).
    fn note_lock_subject(&mut self) {
        match self.lock.subject() {
            Some(s) if self.lock_since.as_ref().map(|(u, _)| u.as_str()) != Some(s) => {
                self.lock_since = Some((s.to_string(), Instant::now()));
            }
            None => self.lock_since = None,
            _ => {}
        }
    }

    /// The agent owns the lock's lifetime. Called after every tick, command,
    /// lock request and VT change:
    /// * the lock's person still stopped → keep it on screen (bring it back if
    ///   it died, hung or was switched away from) and keep its words current;
    /// * the lock's person thawed (by any path — the thaw already happened) →
    ///   switch back to their session and stop the lock;
    /// * no lock, but whoever is on screen is stopped (they switched or logged
    ///   in to a frozen session) → put it up in front of them.
    ///
    /// If a lock can't be shown, whoever it was for is thawed rather than left
    /// behind a blank screen.
    pub async fn reconcile_lock(&mut self) {
        if let Some(subject) = self.lock.subject().map(str::to_string) {
            if self.frozen.contains(&subject) {
                let face = self.face_for(&subject);
                self.lock.publish(face);
                if self.lock.reassert().await {
                    if !self.exec.dry_run() {
                        self.persist_freeze_state();
                    }
                    return;
                }
                self.lock.host().freeze(&subject, false, false);
                self.frozen.remove(&subject);
                self.lock_unavailable(&subject);
            }
            self.lock.release();
        }
        let sessions = self.lock.host().sessions();
        if let Some(u) = lock::on_screen_user(&sessions) {
            if self.frozen.contains(&u) {
                let face = self.face_for(&u);
                if !self.lock.present(&u, face).await {
                    self.lock.host().freeze(&u, false, false);
                    self.frozen.remove(&u);
                    self.lock_unavailable(&u);
                }
            }
        }
        self.note_lock_subject();
        if !self.exec.dry_run() {
            self.persist_freeze_state();
        }
    }

    /// Something woke us between ticks: a lock UI's request, or a VT change.
    pub async fn on_lock_event(&mut self, ev: LockEvent) {
        match ev {
            LockEvent::Request(p) => {
                let reply = self.on_lock_request(p.req).await;
                let _ = p.reply.send(reply);
            }
            LockEvent::VtChanged => self.on_seat_changed().await,
        }
    }

    async fn on_lock_request(&mut self, req: lock::socket::Request) -> lock::socket::Reply {
        use lock::socket::{Outcome, Reply, Request};
        let Some(subject) = self.lock.subject().map(str::to_string) else {
            return Reply {
                face: None,
                result: Some(Outcome::no("Nothing is locked right now.")),
            };
        };
        let result = match req {
            Request::Face => None,
            Request::Code { code } => Some(self.try_code(&subject, &code)),
            Request::Ask => Some(self.ask_from_lock(&subject).await),
            Request::Snooze => Some(self.snooze_from_lock(&subject)),
        };
        // A code that worked takes the lock down right here.
        self.reconcile_lock().await;
        let face = self.lock.subject().map(|s| self.face_for(s));
        if let Some(f) = &face {
            self.lock.publish(f.clone());
        }
        Reply { face, result }
    }

    /// A code typed at the lock, checked here (root), never by the lock.
    fn try_code(&mut self, user: &str, code: &str) -> lock::socket::Outcome {
        use lock::socket::Outcome;
        let policy = self.policies.get(user).cloned().unwrap_or_default();
        let verdict = self
            .parent_keys(&policy)
            .verifier()
            .with_state_path(self.parent_state.clone())
            .verify(code);
        self.pending_events
            .push(parentcode::event(&verdict, "lock_screen", user));
        if !verdict.accepted() {
            return Outcome::no(&match verdict {
                parentcode::Verdict::LockedOut(s) => format!("Too many tries — wait {s} seconds."),
                parentcode::Verdict::NotConfigured => {
                    "There's no unlock code on this computer yet.".to_string()
                }
                _ => lock::WRONG_CODE.to_string(),
            });
        }
        let minutes = lock::UNLOCK_MINUTES;
        // The one override (persisted in the ledger): survives a restart,
        // beats limit/bedtime/window, expires on the trusted clock.
        self.tracker.set_override(
            user,
            self.trusted_now + chrono::Duration::minutes(i64::from(minutes)),
        );
        // A parent at the machine with a valid code is the authority every
        // whole-device lock defers to: clear them all, persistently (this
        // thaws everyone) — not just this user, not just for 30 minutes.
        let evs = self.local_recovery("the unlock code at the lock screen");
        self.pending_events.extend(evs);
        self.pending_events.push(tamper::tamper_event(
            "parent_pin_override",
            SEV_INFO,
            &format!(
                "{user} was unlocked for {minutes} min with the unlock code at the lock screen"
            ),
        ));
        self.notify_user(
            Some(user),
            &format!("You're back — {minutes} minutes"),
            "A parent unlocked this computer with the code.",
            false,
        );
        Outcome::yes("Unlocked")
    }

    /// "Give me 15 more minutes" at the lock — for someone who set their own
    /// limits, after the lock has been up a minute, three times a day.
    /// Decided here, never by the lock UI: a child's lock can send the same
    /// request and is refused.
    fn snooze_from_lock(&mut self, user: &str) -> lock::socket::Outcome {
        use lock::socket::Outcome;
        use lock::{SnoozeRefusal, Stop};
        let policy = self.policies.get(user).cloned().unwrap_or_default();
        let own_rules = matches!(
            self.stop_of(user, &policy),
            Some(Stop::Limit { .. } | Stop::Bedtime { .. } | Stop::OutsideHours { .. })
        );
        let used = self.tracker.snoozes(user);
        let check = lock::snooze_check(
            self.is_self_set(user),
            own_rules,
            self.lock_waited(user),
            used,
        );
        if let Err(why) = check {
            tracing::warn!("snooze for {user} refused: {why:?}");
            return Outcome::no(match why {
                SnoozeRefusal::NotSelfSet => "Only a parent can add time here.",
                SnoozeRefusal::NotTheirStop => "This stop isn't yours to skip.",
                SnoozeRefusal::TooSoon => "In a moment — it opens after a minute.",
                SnoozeRefusal::UsedUp => "That's today's extra time.",
            });
        }
        let minutes = lock::SNOOZE_MINUTES;
        let n = self.tracker.snooze(user, minutes, self.trusted_now);
        if !self.exec.dry_run() {
            self.tracker.save();
        }
        // Thaw now (the override makes the rules allow it); the lock comes
        // down in the reconcile that follows every request.
        self.lock.host().freeze(user, false, false);
        self.frozen.remove(user);
        self.pending_freeze.remove(user);
        self.pending_events.push(Event::new(
            EV_SCREEN_TIME_EARNED,
            SEV_INFO,
            json!({
                "user": user,
                "minutes": minutes,
                "via": "self",
                "today": n,
                "of": lock::SNOOZES_PER_DAY,
            }),
        ));
        self.notify_user(
            Some(user),
            &format!("{minutes} more minutes"),
            "You gave yourself a little more time.",
            false,
        );
        Outcome::yes("15 more minutes")
    }

    /// "Ask for more time" at the lock: the same request `ost ask` files.
    async fn ask_from_lock(&mut self, user: &str) -> lock::socket::Outcome {
        use lock::socket::Outcome;
        let offer = self.earn_offer_for(user);
        match self.auto_request_earn(user, &offer).await {
            Some(_) => Outcome::yes("Asked — a parent will see it."),
            None => Outcome::no("Couldn't reach a parent right now — try again in a moment."),
        }
    }

    /// The VT on screen changed: whoever is on screen now and stopped meets
    /// the lock in a moment, not a frozen (or still-running) desktop until
    /// the next tick. Logging in into a stop gets no grace — there is no work
    /// to save yet.
    async fn on_seat_changed(&mut self) {
        let sessions = self.lock.host().sessions();
        if let Some(u) = lock::on_screen_user(&sessions) {
            let in_grace = self.tracker.peek_override(&u, self.trusted_now).is_some();
            if self.policies.contains_key(&u)
                && !self.frozen.contains(&u)
                && !self.pending_freeze.contains_key(&u)
                && !in_grace
            {
                let policy = self.policies.get(&u).cloned().unwrap_or_default();
                if self.device_lock_effective() {
                    self.stop_user(&u, true).await;
                } else if let Some(reason) = self.rules_now(&u, &policy) {
                    let mut events = Vec::new();
                    self.screen_time_lockout(&u, &reason, &mut events).await;
                    self.pending_events.extend(events);
                }
            }
        }
        self.reconcile_lock().await;
    }

    /// After a restart: whoever the kernel still has frozen is frozen. A
    /// managed user is adopted (the first tick thaws them if they are back
    /// within their rules, and the lock they left comes back if not); anyone
    /// else is thawed — nobody stays frozen by a rule that no longer applies.
    pub fn adopt_frozen(&mut self) {
        for u in self.lock.host().login_users() {
            if self.lock.host().is_frozen(&u) != Some(true) {
                continue;
            }
            if self.policies.contains_key(&u) {
                self.frozen.insert(u);
            } else {
                tracing::warn!("{u} was left frozen but isn't managed; thawing");
                self.lock.host().freeze(&u, false, false);
            }
        }
    }

    /// What "Ask for more time" files — at the lock, in the app, from the
    /// companion, `ost ask`: a plain ask, worded as one. (It used to pick the
    /// first earn task, so the console read "Read for 20 min" for a child who
    /// had only asked.)
    fn earn_offer_for(&self, _user: &str) -> earn::EarnOffer {
        earn::plain_ask()
    }

    /// An ask from `user` is waiting on a parent today.
    fn ask_pending(&self, user: &str) -> bool {
        let today = chrono::Local::now().date_naive();
        self.requested_earn
            .iter()
            .any(|((u, _), d)| u == user && *d == today)
    }

    /// The verdict for one user as the status file publishes it (documented
    /// field by field in docs/AGENT.md → "status.<user>.json"). Everything is
    /// computed on the trusted clock with the same rules function the
    /// enforcement tick uses, so what the app says is what will happen.
    fn user_status(&self, u: &str, p: &Policy) -> serde_json::Value {
        let now = self.trusted_now.with_timezone(&chrono::Local);
        let mut v = self.stop_verdict(u, p);
        // A stop with a save-your-work countdown hasn't landed yet: they can
        // still use the screen until the countdown ends, and that is when it
        // stops — not a red zero a minute early.
        if let Some(deadline) = self.pending_freeze.get(u) {
            if !v.allowed && !self.frozen.contains(u) && v.reason != Some(warn::StopReason::Paused)
            {
                let left = deadline.saturating_duration_since(Instant::now());
                v.allowed = true;
                v.stop_at = Some(now + chrono::Duration::from_std(left).unwrap_or_default());
                v.minutes_left = Some(left.as_secs().div_ceil(60) as u32);
                v.resume_at = None;
            }
        }
        // Heads-ups land 15, 5 and 1 minute before a stop (docs/AGENT.md).
        let next_warning_at = v
            .stop_at
            .filter(|_| v.allowed)
            .and_then(|stop| next_warning(stop, now));
        let ts = |t: chrono::DateTime<chrono::Local>| t.to_rfc3339();
        // Who sees what (the app window's honest footer), and today's rules.
        let kind_of = |x: &str| self.kinds.get(x).map(String::as_str).unwrap_or("");
        let sees = crate::glance::sees(kind_of(u), self.is_self_set(u));
        let shared_sites = sees != crate::glance::Sees::TimeAppsSites
            && self.policies.keys().any(|o| {
                o != u
                    && crate::glance::sees(kind_of(o), self.is_self_set(o))
                        == crate::glance::Sees::TimeAppsSites
            });
        let weekday = chrono::Datelike::weekday(&now).num_days_from_sunday() as u8;
        json!({
            "name": u,
            "self_managed": self.is_self_set(u),
            "can_ask": !self.is_self_set(u) && self.bracket_of(u).can_request_time(),
            "sees": sees,
            "shared_sites": shared_sites,
            "today": crate::glance::today(&p.screen_time, weekday),
            "used_minutes": self.tracker.used_minutes(u),
            "used_here_minutes": self.tracker.used_here_secs(u) / 60,
            "remaining_minutes": self.tracker.remaining_minutes(u, p),
            "frozen": self.frozen.contains(u),
            "freeze_in_secs": self.pending_freeze.get(u).map(|d|
                d.saturating_duration_since(Instant::now()).as_secs()),
            "allowed": v.allowed,
            "reason": v.reason.map(|r| r.id()),
            "minutes_left": v.minutes_left,
            "stop_at": v.stop_at.map(ts),
            "resume_at": v.resume_at.map(ts),
            "next_warning_at": next_warning_at.map(ts),
            "override_until": self
                .tracker
                .peek_override(u, self.trusted_now)
                .map(|t| ts(t.with_timezone(&chrono::Local))),
            "counting": self.counting.iter().any(|c| c == u),
            // An ask is waiting on a parent (a grant or a "not now" clears it).
            "ask_pending": self.ask_pending(u),
            "measured": self.measured,
            "day": self.tracker.day(),
        })
    }

    /// Transparency surface for the per-user tray/companion — time remaining,
    /// freeze state, server connection, and whether a remote shell is open (the
    /// teen deserves to know).
    ///
    /// Split so one managed user can't read another's activity: the shared
    /// `/run/openscreentime/status.json` is world-readable but carries ONLY device-wide
    /// state (lock/connection/remote-shell + device-wide notifications). Each
    /// managed user's usage and their own notifications go in a private
    /// `/run/openscreentime/status.<user>.json`, chowned to that user and `0600`.
    fn write_status_file(&self) {
        if self.exec.dry_run() {
            // No /run writes in a dry run — but say what the app would show,
            // once a minute, so a dry run can be checked end to end.
            if self.attrib_ticks == 0 {
                for (u, p) in &self.policies {
                    tracing::info!(target: "dry_run", "STATUS {u}: {}", self.user_status(u, p));
                }
            }
            return;
        }
        let dir = std::path::Path::new(crate::paths::RUN_DIR);
        let _ = std::fs::create_dir_all(dir);

        // Global, non-sensitive fields shared by every view.
        let base = json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "connection": match self.contact_state {
                ContactState::Online => "online",
                ContactState::OfflineWithinGrace => "offline",
                ContactState::OfflineFailClosed => "offline_fail_closed",
            },
            "device_locked": self.device_locked,
            "offline_hard_lockdown": self.offline_hard_lockdown,
            "tamper_lockdown": self.tamper_lockdown,
        });
        let notif_json = |n: &UserNotification| {
            json!({
                "id": n.id,
                "title": n.title,
                "body": n.body,
                "urgency": if n.critical { "critical" } else { "normal" },
                "user": n.user,
            })
        };
        // Device-wide notifications (no target user) are safe for everyone.
        let device_notifs: Vec<serde_json::Value> = self
            .notifications
            .iter()
            .filter(|n| n.user.is_none())
            .map(&notif_json)
            .collect();

        // Shared world-readable file: device-wide state only, no per-user data.
        let mut global = base.clone();
        global["users"] = json!([]);
        global["notifications"] = json!(device_notifs);
        let tmp = dir.join("status.json.tmp");
        if std::fs::write(&tmp, global.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("status.json"));
        }

        // Per-user private files.
        for (u, p) in &self.policies {
            let Some(uid) = crate::sysusers::uid_of(u) else {
                continue;
            };
            let mut notifs = device_notifs.clone();
            notifs.extend(
                self.notifications
                    .iter()
                    .filter(|n| n.user.as_deref() == Some(u.as_str()))
                    .map(&notif_json),
            );
            let mut view = base.clone();
            view["users"] = json!([self.user_status(u, p)]);
            view["notifications"] = json!(notifs);
            view["login_codes"] = self.login_codes_for(u);
            write_private_status(dir, u, uid, &view.to_string());
        }
        // A code can be for a login with no rules on this computer (yet): it
        // still gets its own private file, with the code and nothing else.
        let mut extra: Vec<&String> = self
            .login_codes
            .iter()
            .flat_map(|(_, users)| users)
            .filter(|u| !self.policies.contains_key(*u))
            .collect();
        extra.sort();
        extra.dedup();
        for u in extra {
            let Some(uid) = crate::sysusers::uid_of(u) else {
                continue;
            };
            let mut view = base.clone();
            view["users"] = json!([]);
            view["notifications"] = json!(device_notifs);
            view["login_codes"] = self.login_codes_for(u);
            write_private_status(dir, u, uid, &view.to_string());
        }
    }

    /// The live codes for one OS login, as its status file carries them.
    fn login_codes_for(&self, user: &str) -> serde_json::Value {
        json!(self
            .login_codes
            .iter()
            .filter(|(_, users)| users.iter().any(|x| x == user))
            .map(|(c, _)| c)
            .collect::<Vec<_>>())
    }

    /// Consume any on-demand "request more time" markers a user's tray dropped
    /// in its own runtime dir, and turn each into an earn-request. This is the
    /// spoof-proof privilege bridge: the unprivileged tray can only write inside
    /// `/run/user/<uid>` (its own, 0700), and only root (this agent) reads it —
    /// so a request is authentically from that user. Deduped per day like the
    /// automatic lockout path.
    async fn check_ondemand_earn(&mut self) {
        let users: Vec<String> = self.policies.keys().cloned().collect();
        for user in users {
            let Some(path) = ondemand_earn_marker(&user) else {
                continue;
            };
            // lstat, not stat: don't let a symlink in the child's own dir make
            // this trigger off some unrelated root file. (No content is read
            // and remove_file unlinks the link itself, so the risk is nil — but
            // keep the whole channel symlink-averse on principle.)
            match std::fs::symlink_metadata(&path) {
                Ok(m) if m.file_type().is_symlink() => {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                Ok(_) => {}
                Err(_) => continue,
            }
            let _ = std::fs::remove_file(&path); // single-use
            let offer = self.earn_offer_for(&user);
            if let Some(copy) = self.auto_request_earn(&user, &offer).await {
                self.notify_user(Some(&user), "Request sent", &copy, false);
            }
        }
    }

    /// File an earn-time / more-time request once per (user, task) per day (the
    /// server also dedupes by returning the existing pending row). Returns the
    /// words to show, if a request was sent or is already pending today.
    async fn auto_request_earn(&mut self, user: &str, offer: &earn::EarnOffer) -> Option<String> {
        let today = chrono::Local::now().date_naive();
        let key = (user.to_string(), offer.id.clone());
        if self.requested_earn.get(&key) == Some(&today) {
            return Some("Asked — waiting for a parent.".to_string());
        }
        match self
            .client
            .post_earn_request(user, &offer.id, &offer.label, offer.reward_minutes)
            .await
        {
            Ok(resp) => {
                tracing::info!(
                    "earn-request {} for {user}/{} is {}",
                    resp.request.id,
                    offer.id,
                    resp.request.status
                );
                self.requested_earn.insert(key, today);
                Some("Asked — waiting for a parent.".to_string())
            }
            Err(e) => {
                tracing::warn!("earn-request for {user}/{} failed: {e}", offer.id);
                None
            }
        }
    }

    /// Dispatch one server command.
    async fn handle_command(&mut self, cmd: Command) -> (CommandAck, Vec<Event>) {
        let mut events = Vec::new();
        // Commands land between ticks: take "now" fresh, so a "+15 min" is
        // 15 minutes from when it arrived, not from the last tick.
        let reading = crate::clock::read(&self.boot_id);
        self.trusted_now = self.tracker.advance(&reading, &chrono::Local);
        let result = match cmd.cmd_type.as_str() {
            CMD_LOCK => {
                self.device_locked = true;
                save_device_locked(true);
                // A pause from the console can carry a save-your-work window
                // (block-account sends one): the overlay appears now, the
                // freeze lands when the window closes — see
                // effective_device_locked. No window = the old instant lock.
                let grace = cmd
                    .payload
                    .get("grace_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                self.device_lock_grace_until =
                    (grace > 0).then(|| Instant::now() + Duration::from_secs(grace));
                if !parentcode::Verifier::from_device().configured() {
                    events.push(tamper::tamper_event(
                        "lock_without_offline_credential",
                        SEV_WARN,
                        "locked, but this device has no offline unlock credential — if the \
                         server becomes unreachable while locked, only the root recovery \
                         account can free it. Generate recovery codes in the console.",
                    ));
                }
                // No window: the lock goes up now. With one, the companion
                // counts it down (`pause_at`) and the tick stops everyone
                // when it closes.
                if grace == 0 {
                    for user in self.policies.keys().cloned().collect::<Vec<_>>() {
                        self.stop_user(&user, true).await;
                    }
                }
                if !self.exec.dry_run() {
                    self.persist_freeze_state();
                }
                events.push(Event::new(
                    EV_LOCK,
                    SEV_WARN,
                    json!({ "source": "command" }),
                ));
                json!({ "locked": true })
            }
            CMD_UNLOCK => {
                self.device_locked = false;
                save_device_locked(false);
                self.device_lock_grace_until = None;
                // An admin unlock also lifts a confirmed-evasion lockdown.
                self.tamper_lockdown = false;
                // An unlock also disarms carried-over countdowns — and must
                // hit disk immediately, or a power-cut right after would boot
                // back into the lock the parent just lifted.
                self.pending_freeze.clear();
                if !self.exec.dry_run() {
                    self.persist_freeze_state();
                }
                // Resume ends the pause — and only that. Time is given with
                // "Give 15" (`credit_time`), which the console shows; a Resume
                // that quietly handed 30 minutes to whoever a rule was
                // stopping made Pause → Resume a free half hour on a day
                // that was over (and handed it to logins nobody had paused,
                // not even signed in). Someone whose own rules still stop
                // them stays stopped; the lock just says why now.
                //
                // An explicit grant in the payload (`minutes`, or `until:
                // "end_of_day"`, for one `os_username`) is still honoured:
                // that is a parent asking for time by name.
                let targets: Vec<String> = cmd
                    .payload
                    .get("os_username")
                    .and_then(|v| v.as_str())
                    .map(|u| vec![u.to_string()])
                    .unwrap_or_default();
                let minutes = cmd
                    .payload
                    .get("minutes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let end_of_day =
                    cmd.payload.get("until").and_then(|v| v.as_str()) == Some("end_of_day");
                let until = if minutes > 0 {
                    Some(self.trusted_now + chrono::Duration::minutes(minutes.min(24 * 60) as i64))
                } else if end_of_day {
                    Some(self.end_of_day())
                } else {
                    None
                };
                let who: Vec<String> = match until {
                    Some(_) => targets,
                    None => Vec::new(),
                };
                if let Some(until) = until {
                    self.override_users(&who, until);
                }
                // Thaw whoever is free to go now — the pause is lifted and
                // their override (or their own rules) lets them in. Someone a
                // rule still stops (a Resume aimed at another person) stays
                // stopped: thawing them only for the next tick to freeze them
                // again would flash the desktop and say "you're back" when
                // they aren't. The lock just changes its words.
                for user in self.frozen.clone() {
                    let policy = self.policies.get(&user).cloned().unwrap_or_default();
                    if self.rules_now(&user, &policy).is_some() {
                        continue;
                    }
                    self.frozen.remove(&user);
                    self.lock.host().freeze(&user, false, false);
                    self.notify_user(
                        Some(&user),
                        "You're back",
                        "A parent resumed this computer.",
                        false,
                    );
                }
                if !self.exec.dry_run() {
                    self.persist_freeze_state();
                }
                events.push(Event::new(
                    EV_UNLOCK,
                    SEV_INFO,
                    json!({ "source": "command", "override_users": who, "override_until": until }),
                ));
                json!({ "locked": false, "override_users": who, "override_until": until })
            }
            CMD_LOGIN_CODE => {
                let Some((code, os_users)) =
                    crate::logincode::LoginCode::from_command(&cmd.payload, chrono::Utc::now())
                else {
                    return (ack_failed(&cmd.id, "bad login_code payload"), events);
                };
                // Only logins that really exist here; the server named them.
                let os_users: Vec<String> = os_users
                    .into_iter()
                    .filter(|u| crate::sysusers::uid_of(u).is_some())
                    .collect();
                self.login_codes.retain(|(c, _)| c.id != code.id);
                self.login_codes.push((code, os_users.clone()));
                // Publish now, not at the next tick — someone is waiting.
                self.write_status_file();
                // Bring the window up where there's a desktop (no tray on
                // GNOME, and the window may be closed); `ost code` otherwise.
                #[cfg(feature = "gui")]
                if !self.exec.dry_run() {
                    for u in &os_users {
                        crate::logincode::open_app_for(u);
                    }
                }
                json!({ "shown_to": os_users })
            }
            CMD_APPLY_POLICY => match self.client.get_policy().await {
                Ok(bundle) => {
                    self.record_contact();
                    match self.apply_bundle(bundle) {
                        Ok(evs) => events.extend(evs),
                        Err(e) => return (ack_failed(&cmd.id, &e.to_string()), events),
                    }
                    json!({ "policy_version": self.policy_version })
                }
                Err(e) => return (ack_failed(&cmd.id, &e.to_string()), events),
            },
            CMD_SET_TAMPER_LEVEL => {
                let requested = cmd
                    .payload
                    .get("level")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1)
                    .min(u64::from(u8::MAX)) as u8;
                // A fresh request deserves a fresh answer, even if the same
                // cap was already reported from a bundle.
                self.tamper_cap_reported = None;
                let (level, evs, polkit) = self.adopt_tamper_level(requested);
                events.extend(evs);
                if let Err(e) = polkit {
                    return (ack_failed(&cmd.id, &e.to_string()), events);
                }
                tamper_level_ack(&level)
            }
            CMD_CREDIT_TIME => {
                let os_username = cmd
                    .payload
                    .get("os_username")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let minutes = cmd
                    .payload
                    .get("minutes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let request_id = cmd
                    .payload
                    .get("request_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if os_username.is_empty() || minutes == 0 {
                    return (
                        ack_failed(&cmd.id, "credit_time missing os_username/minutes"),
                        events,
                    );
                }
                // A grant filed for an earlier day (the device was offline
                // when the parent approved) belongs to that day, not this one.
                let for_day = cmd
                    .payload
                    .get("day")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<chrono::NaiveDate>().ok());
                if let (Some(d), Some(today)) = (for_day, self.tracker.day()) {
                    if d < today {
                        return (
                            CommandAck {
                                command_id: cmd.id,
                                status: "acked".into(),
                                result: json!({ "credited": false, "stale_day": d }),
                            },
                            events,
                        );
                    }
                }
                // "+N minutes" = N more minutes on today's budget AND an
                // override for N minutes, so it also carries past bedtime or
                // the end of the allowed hours. Idempotent on the command id:
                // a redelivery after a lost ack never credits twice.
                let outcome = self
                    .tracker
                    .grant(&cmd.id, &os_username, minutes, self.trusted_now);
                if !self.exec.dry_run() {
                    self.tracker.save();
                }
                if outcome == screentime::Grant::Duplicate {
                    return (
                        CommandAck {
                            command_id: cmd.id,
                            status: "acked".into(),
                            result: json!({ "credited": true, "duplicate": true,
                                            "os_username": os_username, "minutes": minutes }),
                        },
                        events,
                    );
                }
                // The user's pending requests are now resolved; clear the dedupe
                // cache so a later same-day ask sends a fresh request instead
                // of showing a stale "waiting for a parent".
                self.requested_earn.retain(|(u, _), _| u != &os_username);
                // Tell them — an approval used to be silent. Someone stopped
                // hears "You're back" when the thaw actually happens (the
                // next tick, if the time is enough), never before: the lock
                // must never say they're back while they aren't.
                if !self.frozen.contains(&os_username) {
                    self.notify_user(
                        Some(&os_username),
                        &format!("{minutes} more minutes"),
                        "A parent gave you more time.",
                        false,
                    );
                }
                events.push(earn::earned_event(&os_username, &request_id, minutes));
                json!({ "credited": true, "os_username": os_username, "minutes": minutes })
            }
            CMD_DENY_EARN => {
                let os_username = cmd
                    .payload
                    .get("os_username")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let task_id = cmd
                    .payload
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                // Clear the dedupe so a later lockout can send a fresh request
                // (a denial should never strand "WAITING FOR APPROVAL" all day).
                self.requested_earn.retain(|(u, t), _| {
                    !(u == &os_username && (task_id.is_empty() || t == &task_id))
                });
                self.notify_user(
                    Some(&os_username),
                    "Not this time",
                    "A parent said no to more time for now.",
                    false,
                );
                json!({ "denied": true, "os_username": os_username, "task_id": task_id })
            }
            CMD_PING => {
                // Liveness: prove the client is alive and say what it's doing.
                // The console reads this off the command ack (a round-trip means
                // "it works"); an offline device simply never acks.
                json!({
                    "pong": true,
                    "agent_version": env!("CARGO_PKG_VERSION"),
                    "enforcing": screentime::freezer_usable(),
                    "frozen_users": self.frozen.len(),
                    "active_users": self.active_users.len(),
                    "ts": chrono::Utc::now().to_rfc3339(),
                })
            }
            other => {
                return (
                    ack_failed(&cmd.id, &format!("unknown command '{other}'")),
                    events,
                );
            }
        };
        (
            CommandAck {
                command_id: cmd.id,
                status: "acked".into(),
                result,
            },
            events,
        )
    }
}

/// "You're back": how long, and until when — the verdict's stop, the same
/// moment the warnings announce.
fn back_words(v: &openscreentime_policy::rules::Verdict<chrono::Local>) -> String {
    match (v.allowed, v.minutes_left, v.stop_at) {
        (true, Some(m), Some(at)) if m > 0 => format!(
            "You have {}, until {}.",
            crate::glance::duration(m),
            at.format("%H:%M")
        ),
        _ => "Your screen time is back on.".to_string(),
    }
}

/// The warning vocabulary's name for a screen-time stop reason.
fn stop_reason_of(r: &screentime::LockReason) -> warn::StopReason {
    match r {
        screentime::LockReason::DailyLimit { .. } => warn::StopReason::Limit,
        screentime::LockReason::Bedtime => warn::StopReason::Bedtime,
        screentime::LockReason::OutsideWindow => warn::StopReason::OutsideHours,
    }
}

/// What to do to a user's frozen state this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreezeAction {
    Freeze,
    Unfreeze,
    None,
}

/// Pure decision logic for the enforcement tick (bug fix: a whole-device admin
/// lock, once engaged via the `lock` command, must keep every user frozen
/// regardless of what screen-time enforcement says — it must never be the
/// screen-time verdict alone that decides to unfreeze someone while
/// `device_locked` is true). Extracted so it's testable without the rest of the
/// `Agent` machinery.
/// Whether screen time should be evaluated for a user on this tick.
///
/// Active users are evaluated, obviously. Frozen users are evaluated *even when
/// inactive*: skipping them yielded `None`, which `decide_freeze` reads as
/// "within policy" and unfreezes. Flipping to another session and back then
/// re-armed the full [`FREEZE_GRACE`], handing out ~60 usable seconds per flip.
///
/// A user who is neither active nor frozen is skipped, so nobody is newly
/// frozen — or shown an overlay — merely for appearing in the policy.
fn should_evaluate_screen_time(
    in_grace: bool,
    is_active: bool,
    currently_frozen: bool,
    has_clock_rule: bool,
) -> bool {
    !in_grace && (is_active || currently_frozen || has_clock_rule)
}

fn decide_freeze(
    device_locked: bool,
    screen_time_lock: Option<&screentime::LockReason>,
    currently_frozen: bool,
) -> FreezeAction {
    if device_locked {
        // Admin lock overrides everything: stay (or become) frozen. Screen-time
        // verdicts are irrelevant while the device is locked.
        return if currently_frozen {
            FreezeAction::None
        } else {
            FreezeAction::Freeze
        };
    }
    match (screen_time_lock, currently_frozen) {
        (Some(_), false) => FreezeAction::Freeze,
        (None, true) => FreezeAction::Unfreeze,
        _ => FreezeAction::None,
    }
}

/// What one tick bills: the awake time since the previous tick
/// (CLOCK_MONOTONIC — suspend excluded), capped at `BILL_CAP`. The first tick
/// after start bills nothing (the downtime is unknown).
fn billable_elapsed(last: Option<Instant>, now: Instant) -> Duration {
    last.map(|t| now.saturating_duration_since(t).min(BILL_CAP))
        .unwrap_or(Duration::ZERO)
}

/// When to wake for a stop landing at `at`: just after it (so the budget is
/// really spent by then), if that comes before the next regular tick — never
/// sooner than a quarter second from now, so a stop that didn't land yet
/// can't spin the loop.
fn stop_wake(at: Instant, now: Instant) -> Option<Instant> {
    let wake = (at + STOP_SLACK).max(now + Duration::from_millis(250));
    (wake < now + TICK).then_some(wake)
}

/// Split measured time into the whole seconds billed now and the fraction
/// carried to the next tick.
fn whole_seconds(d: Duration) -> (Duration, Duration) {
    let whole = Duration::from_secs(d.as_secs());
    (whole, d - whole)
}

/// The next heads-up before a stop at `stop` (`WARN_BEFORE_MIN` minutes
/// before it), if one is still ahead of `now`.
fn next_warning<Tz: chrono::TimeZone>(
    stop: chrono::DateTime<Tz>,
    now: chrono::DateTime<Tz>,
) -> Option<chrono::DateTime<Tz>> {
    WARN_BEFORE_MIN
        .iter()
        .map(|m| stop.clone() - chrono::Duration::minutes(*m))
        .find(|w| *w > now)
}

/// The `set_tamper_level` ack result: the level this computer really runs at,
/// and — when that isn't what was asked — that it was capped, and why.
fn tamper_level_ack(level: &tamper::TamperLevel) -> serde_json::Value {
    let mut out = json!({ "tamper_level": level.applied });
    if level.capped() {
        out["requested"] = json!(level.requested);
        out["capped"] = json!(true);
        out["ceiling"] = json!(level.ceiling);
        out["detail"] = json!("level 3 needs --tamper-max on this computer");
    }
    out
}

fn ack_failed(id: &str, msg: &str) -> CommandAck {
    tracing::warn!("command {id} failed: {msg}");
    CommandAck {
        command_id: id.to_string(),
        status: "failed".into(),
        result: json!({ "error": msg }),
    }
}

/// The agent, shared between the enforcement tick (its own task) and the
/// network loops.
type Shared = Arc<tokio::sync::Mutex<Agent>>;

/// Entry point for `run`.
///
/// Two independent loops share the agent:
/// * the **enforcement tick** — accounting, rules, stops, the status file,
///   the watchdog heartbeat — on its own timer, whatever the network does;
/// * the **network loop** — WS bus or HTTP polling, reconnect with backoff,
///   commands, usage reports, event and usage-slice delivery.
///
/// They used to be one: the tick only ran inside the WS/poll loops, so while
/// the server was unreachable a device ticked about once a minute (counting
/// 10–25 % of real use) and the watchdog, seeing a stale heartbeat, kept
/// restarting a perfectly healthy offline agent.
pub async fn run(ctx: Arc<AgentCtx>, cfg: AgentConfig) -> Result<()> {
    ctx.require_root_for_enforcement()?;
    // Removed from its household: never enforce again. Finish taking itself
    // off (the helper stops this unit) and wait for that.
    if crate::retire::marked() {
        tracing::warn!("this computer was removed from its household; not enforcing");
        crate::retire::spawn_helper(&Exec::new(ctx.clone()));
        std::future::pending::<()>().await;
    }
    let mut agent = Agent::new(ctx.clone(), cfg)?;
    tracing::info!(
        dry_run = ctx.dry_run,
        is_root = ctx.is_root,
        tamper_level = agent.tamper_level,
        "openscreentime run loop starting"
    );

    let boot_events = agent.bootstrap().await.unwrap_or_default();
    agent.queue_events(boot_events);

    // The lock: whoever the kernel still has frozen is ours; the graphical
    // lock's socket; and a watch on the VT, so a switch or a login into a
    // stopped session meets the lock at once.
    agent.adopt_frozen();
    let lock_rx = agent
        .lock_rx
        .take()
        .expect("the lock channel is taken once, here");
    if !agent.exec.dry_run() {
        match users::get_user_by_name(lock::LOCK_USER) {
            Some(u) => match lock::socket::bind(&lock::socket::path(), Some(u.primary_group_id())) {
                Ok(l) => {
                    tokio::spawn(lock::socket::serve(
                        l,
                        u.uid(),
                        agent.lock_shared.clone(),
                        agent.lock_tx.clone(),
                    ));
                }
                Err(e) => tracing::warn!("lock socket unavailable: {e}"),
            },
            None => tracing::warn!(
                "no {} user — the graphical lock is off; the text lock is used (run `ost install-service`)",
                lock::LOCK_USER
            ),
        }
        lock::spawn_vt_watch(agent.lock_tx.clone());
    }
    // A lock adopted from the previous run gets a fresh chance to reconnect
    // to the socket bound just now before it counts as hung.
    lock::mark_seen(&agent.lock_shared);
    agent.reconcile_lock().await;

    // Daily self-update (first check ~2 min in). No-op unless enabled and
    // running as the installed /usr/local/bin binary — see update.rs.
    tokio::spawn(crate::update::update_loop(
        agent.cfg.clone(),
        agent.client.clone(),
        agent.exec.clone(),
    ));

    // Answer `ost login` requests from desktop users (they can't read the
    // device token; we mint the voucher for them — see loginbroker.rs).
    tokio::spawn(crate::loginbroker::serve(
        agent.cfg.clone(),
        agent.client.clone(),
        agent.exec.dry_run(),
    ));

    let client = agent.client.clone();
    let agent: Shared = Arc::new(tokio::sync::Mutex::new(agent));
    let tick = tokio::spawn(tick_loop(agent.clone()));
    // A code typed at the lock, or a switch to a stopped session, is answered
    // between ticks and whatever the network is doing.
    tokio::spawn(lock_loop(agent.clone(), lock_rx));

    // The tick never returns; if it dies (a panic), exit so systemd restarts
    // the agent now — a process with a live network loop and no enforcement
    // would otherwise sit there until the watchdog noticed the heartbeat.
    tokio::select! {
        r = tick => anyhow::bail!("the enforcement tick stopped: {r:?}"),
        _ = network_loop(&agent, &client) => unreachable!("the network loop never returns"),
    }
}

/// Reconnect with jittered exponential backoff (1 s → 60 s). A server that
/// answers HTTP but not WS keeps the backoff short: the poll round succeeded.
async fn network_loop(agent: &Shared, client: &ServerClient) {
    let agent = agent.clone();
    let mut backoff_secs = BACKOFF_MIN_SECS;
    loop {
        let ended = match client.connect_ws().await {
            Ok(stream) => {
                tracing::info!("WS bus connected");
                backoff_secs = BACKOFF_MIN_SECS;
                run_ws(&agent, stream).await.err()
            }
            Err(e) if crate::client::is_retired(&e) => Some(e),
            Err(e) => {
                tracing::warn!("WS unavailable ({e}); falling back to heartbeat polling");
                match run_poll(&agent).await {
                    Ok(()) => {
                        backoff_secs = BACKOFF_MIN_SECS;
                        None
                    }
                    Err(e) => Some(e),
                }
            }
        };
        if let Some(e) = ended {
            if retirement_confirmed(client, &e).await {
                agent.lock().await.retire().await;
                // Nothing left to talk about; the helper stops this unit.
                std::future::pending::<()>().await;
            }
            tracing::warn!("server connection ended: {e}");
        }
        let jitter = rand::Rng::gen_range(&mut rand::thread_rng(), 0..=backoff_secs / 2 + 1);
        tokio::time::sleep(Duration::from_secs(backoff_secs + jitter)).await;
        backoff_secs = (backoff_secs * 2).min(BACKOFF_MAX_SECS);
    }
}

/// The server retired this computer — asked once more, so one odd answer
/// can't take the rules off a child's computer. Only `client::Retired`
/// counts (410 `device_retired` from the configured server), never a 401 or
/// a network error.
async fn retirement_confirmed(client: &ServerClient, first: &anyhow::Error) -> bool {
    if !crate::client::is_retired(first) {
        return false;
    }
    match client.get_policy().await {
        Err(e) if crate::client::is_retired(&e) => true,
        other => {
            tracing::warn!(
                "the server said this computer was removed, then not ({:?}); keeping the rules",
                other.err()
            );
            false
        }
    }
}

/// The lock's events: codes and asks from a lock UI, VT changes.
async fn lock_loop(agent: Shared, mut rx: mpsc::Receiver<LockEvent>) {
    while let Some(ev) = rx.recv().await {
        agent.lock().await.on_lock_event(ev).await;
    }
}

/// The enforcement tick, on its own timer. `Delay` (not the default `Burst`):
/// after a stall the next tick simply runs late — and bills the measured
/// elapsed time (capped), never a burst of replayed fixed credits.
async fn tick_loop(agent: Shared) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // A stop due before the next tick gets a tick of its own, right as
        // it lands: the lock comes at the minute the warnings announced.
        let stop = agent.lock().await.next_stop_at;
        match stop.and_then(|at| stop_wake(at, Instant::now())) {
            Some(wake) => {
                tokio::select! {
                    _ = ticker.tick() => {}
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(wake)) => {}
                }
            }
            None => {
                ticker.tick().await;
            }
        }
        let mut a = agent.lock().await;
        let events = a.enforcement_tick().await;
        a.queue_events(events);
    }
}

/// Deliver queued events, in server-sized batches, without holding the agent
/// across the network call. A failed batch goes back to the front.
async fn flush_queued(agent: &Shared) {
    loop {
        let (client, batch) = {
            let mut a = agent.lock().await;
            a.cap_pending_events();
            if a.pending_events.is_empty() {
                return;
            }
            let take = a.pending_events.len().min(EVENT_BATCH_MAX);
            let batch: Vec<Event> = a.pending_events.drain(..take).collect();
            (a.client.clone(), batch)
        };
        if let Err(e) = client.post_events(&batch).await {
            let mut a = agent.lock().await;
            let rest = std::mem::take(&mut a.pending_events);
            a.pending_events = batch;
            a.pending_events.extend(rest);
            a.cap_pending_events();
            // warn, not debug: a stalled audit pipeline is exactly the kind of
            // quiet failure this codebase keeps getting bitten by.
            tracing::warn!(
                "event post failed, {} buffered for retry: {e}",
                a.pending_events.len()
            );
            return;
        }
    }
}

/// Post where-the-time-goes slices (sampled by the tick).
async fn post_slices(agent: &Shared) {
    let (client, batch) = {
        let mut a = agent.lock().await;
        (a.client.clone(), a.attrib.drain(400))
    };
    if batch.is_empty() {
        return;
    }
    if let Err(e) = client.post_usage_slices(&batch).await {
        tracing::debug!("usage post failed, keeping batch: {e}");
        agent.lock().await.attrib.requeue(batch);
    }
}

/// WS-connected loop: read server frames, push state/usage frames, deliver
/// events. The enforcement tick runs elsewhere (`tick_loop`).
async fn run_ws(agent: &Shared, stream: crate::client::WsStream) -> Result<()> {
    let (mut write, mut read) = stream.split();
    let (out_tx, mut out_rx) = mpsc::channel::<AgentFrame>(256);

    // Writer task: serialize AgentFrames to the socket.
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let txt = match serde_json::to_string(&frame) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if write.send(Message::Text(txt)).await.is_err() {
                break;
            }
        }
    });

    // First thing on a fresh connection: tell the server what is actually true
    // here (lock state, frozen users, gaps) and push the usage we may have
    // accumulated while disconnected (the reply carries the person's day
    // elsewhere and the server's clock).
    let (state, usage) = {
        let mut a = agent.lock().await;
        (a.state_frame_due(true), a.usage_snapshot())
    };
    if let Some(frame) = state {
        let _ = out_tx.send(frame).await;
    }
    if !usage.is_empty() {
        let _ = out_tx.send(AgentFrame::Heartbeat { usage }).await;
    }
    let mut last_hb = Instant::now();
    let mut beat = tokio::time::interval(TICK);
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut beats: u32 = 0;
    loop {
        tokio::select! {
            _ = beat.tick() => {
                // Events go over HTTP, not a WS frame: a frame pushed into a
                // dying socket's channel is gone, while the queue keeps
                // undelivered batches and retries — same guarantee in both modes.
                flush_queued(agent).await;
                beats = beats.wrapping_add(1);
                if beats.is_multiple_of(6) {
                    post_slices(agent).await;
                }
                let (state, usage) = {
                    let mut a = agent.lock().await;
                    // The WS bus has no HTTP heartbeat, so usage rides here
                    // every WS_HEARTBEAT.
                    let usage = (last_hb.elapsed() >= WS_HEARTBEAT).then(|| a.usage_snapshot());
                    (a.state_frame_due(false), usage)
                };
                if let Some(frame) = state {
                    let _ = out_tx.send(frame).await;
                }
                if let Some(usage) = usage {
                    last_hb = Instant::now();
                    if !usage.is_empty() {
                        let _ = out_tx.send(AgentFrame::Heartbeat { usage }).await;
                    }
                }
            }
            msg = read.next() => {
                let Some(msg) = msg else { break; };
                let msg = msg?;
                // Any frame from the server (including a bare Ping) counts as contact.
                agent.lock().await.record_contact();
                match msg {
                    Message::Text(txt) => {
                        if let Err(e) = handle_server_text(agent, &txt, &out_tx).await {
                            tracing::debug!("frame handling error: {e}");
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(p) => { let _ = out_tx.send(AgentFrame::Pong).await; let _ = p; }
                    _ => {}
                }
            }
        }
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

async fn handle_server_text(
    agent: &Shared,
    txt: &str,
    out_tx: &mpsc::Sender<AgentFrame>,
) -> Result<()> {
    let frame: ServerFrame = serde_json::from_str(txt)?;
    match frame {
        ServerFrame::Command { command } => {
            let ack = {
                let mut a = agent.lock().await;
                let (ack, events) = a.handle_command(command).await;
                // A resume, unlock or grant takes the lock down right away.
                a.reconcile_lock().await;
                a.queue_events(events);
                ack
            };
            let _ = out_tx.send(AgentFrame::Ack { ack }).await;
            flush_queued(agent).await;
        }
        ServerFrame::Ping => {
            let _ = out_tx.send(AgentFrame::Pong).await;
        }
        ServerFrame::Usage { server_time, users } => {
            agent.lock().await.apply_person_days(server_time, users);
        }
    }
    Ok(())
}

/// Heartbeat polling fallback (no WS); commands flow via the heartbeat
/// command queue. Runs one `POLL_ROUND`, then returns `Ok` so the caller
/// retries the WS bus; returns `Err` as soon as a heartbeat fails.
async fn run_poll(agent: &Shared) -> Result<()> {
    let (client, secs) = {
        let a = agent.lock().await;
        (a.client.clone(), a.cfg.poll_interval_secs.clamp(5, 30))
    };
    let mut hb = tokio::time::interval(Duration::from_secs(secs));
    hb.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let round_end = Instant::now() + POLL_ROUND;
    let mut beats: u32 = 0;
    loop {
        if Instant::now() >= round_end {
            return Ok(());
        }
        hb.tick().await;
        let users = crate::sysusers::login_users();
        let usage = agent.lock().await.usage_snapshot();
        let resp = match client.heartbeat("online", None, &users, &usage).await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!("heartbeat failed ({e}); will retry");
                return Err(e); // bubble up to reconnect/backoff, retries WS
            }
        };
        let current_version = {
            let mut a = agent.lock().await;
            a.record_contact();
            a.apply_person_days(resp.server_time, resp.usage);
            a.policy_version.clone()
        };
        for cmd in resp.commands {
            let ack = {
                let mut a = agent.lock().await;
                let (ack, events) = a.handle_command(cmd).await;
                a.reconcile_lock().await;
                a.queue_events(events);
                ack
            };
            let _ = client.ack_command(&ack).await;
        }
        // Poll mode has no push channel: a changed policy_version is the
        // signal to re-pull and re-apply.
        if resp.policy_version != current_version {
            match client.get_policy().await {
                Ok(bundle) => {
                    let mut a = agent.lock().await;
                    match a.apply_bundle(bundle) {
                        Ok(evs) => a.queue_events(evs),
                        Err(e) => tracing::warn!("policy re-apply failed: {e}"),
                    }
                }
                Err(e) => tracing::warn!("policy re-pull failed: {e}"),
            }
        }
        flush_queued(agent).await;
        beats = beats.wrapping_add(1);
        if beats.is_multiple_of(4) {
            post_slices(agent).await;
        }
    }
}

#[cfg(test)]
mod tests {
    /// A frozen user must stay frozen when their session goes inactive.
    /// Regression: VT-flipping unfroze them and re-granted the save-your-work
    /// countdown, which is ~60 seconds of screen time per flip, all night.
    #[test]
    fn frozen_users_are_still_evaluated_when_inactive() {
        // frozen + inactive -> still evaluated, so the freeze holds
        assert!(should_evaluate_screen_time(false, false, true, false));
        // a clock rule (bedtime/window) is evaluated even with no local seat
        assert!(should_evaluate_screen_time(false, false, false, true));
        assert!(!should_evaluate_screen_time(false, false, false, false));
        // active -> evaluated as always
        assert!(should_evaluate_screen_time(false, true, false, false));
        // neither active nor frozen -> skipped, never newly frozen while away
        assert!(!should_evaluate_screen_time(false, false, false, false));
        // a parent-granted grace window suspends enforcement outright
        assert!(!should_evaluate_screen_time(true, true, true, false));
    }

    /// Billing is measured awake time, capped: a normal tick bills its ten
    /// seconds, a stalled agent at most BILL_CAP, a fresh start nothing.
    #[test]
    fn ticks_bill_measured_time_capped() {
        let t0 = Instant::now();
        assert_eq!(billable_elapsed(None, t0), Duration::ZERO);
        assert_eq!(
            billable_elapsed(Some(t0), t0 + Duration::from_secs(10)),
            Duration::from_secs(10)
        );
        assert_eq!(
            billable_elapsed(Some(t0), t0 + Duration::from_secs(4 * 3600)),
            BILL_CAP
        );
        // A late tick (the old code credited a fixed 10 s whatever happened).
        assert_eq!(
            billable_elapsed(Some(t0), t0 + Duration::from_secs(37)),
            Duration::from_secs(37)
        );
    }

    #[test]
    fn warnings_come_fifteen_five_and_one_minute_before_the_stop() {
        use chrono::TimeZone;
        let at = |h, m| chrono::Utc.with_ymd_and_hms(2026, 9, 24, h, m, 0).unwrap();
        assert_eq!(next_warning(at(20, 0), at(19, 30)), Some(at(19, 45)));
        assert_eq!(next_warning(at(20, 0), at(19, 50)), Some(at(19, 55)));
        assert_eq!(next_warning(at(20, 0), at(19, 56)), Some(at(19, 59)));
        assert_eq!(next_warning(at(20, 0), at(19, 59)), None);
        // The published schedule is the one the companion announces.
        assert_eq!(WARN_BEFORE_MIN.map(|m| m as u32), crate::warn::THRESHOLDS);
    }

    #[test]
    fn ost_unlock_marker_carries_its_minutes() {
        assert_eq!(
            parse_local_recovery_marker("1790000000 45\n"),
            Some((1790000000, 45))
        );
        // An old marker (timestamp only) still clears the lock, no override.
        assert_eq!(
            parse_local_recovery_marker("1790000000"),
            Some((1790000000, 0))
        );
        assert_eq!(parse_local_recovery_marker("garbage"), None);
    }

    /// A full retry buffer must drain in a bounded number of round-trips.
    /// (The batch-vs-server-cap invariant itself is a `const` assertion up top,
    /// so it fails the build rather than waiting for anyone to run tests.)
    #[test]
    fn a_full_event_buffer_drains_in_bounded_batches() {
        let batches = PENDING_EVENTS_CAP.div_ceil(EVENT_BATCH_MAX);
        assert!(
            (1..=16).contains(&batches),
            "a full buffer needs {batches} posts to drain; that is not bounded work per tick"
        );
    }

    use super::*;
    use crate::enforce::screentime::LockReason;

    fn daily_limit() -> LockReason {
        LockReason::DailyLimit {
            used_min: 60,
            limit_min: 60,
        }
    }

    #[test]
    fn device_lock_freezes_regardless_of_screen_time_verdict() {
        // Bug fix: while an admin `lock` is active, a screen-time verdict that
        // would otherwise unfreeze the user (None = within policy) must NOT
        // unfreeze them, and a not-yet-frozen user must be frozen.
        assert_eq!(
            decide_freeze(true, None, false),
            FreezeAction::Freeze,
            "device_locked must freeze a not-yet-frozen user even with no screen-time reason"
        );
        assert_eq!(
            decide_freeze(true, None, true),
            FreezeAction::None,
            "device_locked must keep an already-frozen user frozen"
        );
        let reason = daily_limit();
        assert_eq!(
            decide_freeze(true, Some(&reason), true),
            FreezeAction::None,
            "device_locked must keep the user frozen even with an active screen-time reason too"
        );
    }

    #[test]
    fn device_unlocked_follows_screen_time_verdict() {
        let reason = daily_limit();
        assert_eq!(
            decide_freeze(false, Some(&reason), false),
            FreezeAction::Freeze
        );
        assert_eq!(decide_freeze(false, None, true), FreezeAction::Unfreeze);
        assert_eq!(decide_freeze(false, None, false), FreezeAction::None);
        assert_eq!(
            decide_freeze(false, Some(&reason), true),
            FreezeAction::None,
            "already frozen + still locked out: no change"
        );
    }

    /// The persisted freeze state must survive a serialize/deserialize cycle
    /// intact, and an absent or garbled file must load as the harmless default
    /// (nothing frozen, no grants spent) — never a panic on the boot path.
    #[test]
    fn freeze_state_round_trips_and_tolerates_garbage() {
        let shown = lock::Shown {
            subject: "vali".into(),
            vt: lock::LOCK_VT,
            return_vt: Some(2),
            mode: lock::Mode::Gui,
            boot_id: "b".into(),
        };
        let st = FreezeState {
            frozen: vec!["vali".to_string()],
            lock: Some(shown.clone()),
            tamper_lockdown: true,
            saved_at: Some(chrono::Utc::now()),
        };
        let json = serde_json::to_string(&st).unwrap();
        let back: FreezeState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.frozen, vec!["vali".to_string()]);
        assert_eq!(back.lock, Some(shown));
        assert!(back.tamper_lockdown);
        // A file from before the lock existed (it carried challenge grants)
        // still loads.
        let old: FreezeState = serde_json::from_str(
            r#"{"frozen":["vali"],"challenge_grants":{"vali":["2026-08-05",2]}}"#,
        )
        .unwrap();
        assert_eq!(old.frozen, vec!["vali".to_string()]);
        assert!(old.lock.is_none());
        assert!(back.saved_at.is_some());

        let garbled: FreezeState = serde_json::from_str("{}").unwrap();
        assert!(garbled.frozen.is_empty());
        assert!(!garbled.tamper_lockdown);
    }

    // ── The lock's lifecycle, against a fake machine ──────────────────────────

    use crate::lock::socket::Request;
    use crate::lock::testing::{session, FakeHost};

    const SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    /// A dry-run agent whose lock and freezes go to a recording fake, with mia
    /// on screen (tty2), logged in and over her 60 minutes.
    fn agent_with_mia() -> (Agent, FakeHost) {
        let ctx = AgentCtx::new(true, false, 1);
        let cfg = AgentConfig {
            server_url: "http://127.0.0.1:9".into(),
            device_id: "d".into(),
            device_token: "t".into(),
            poll_interval_secs: 30,
            tamper_level: 1,
            auto_update: false,
        };
        let mut a = Agent::new(ctx, cfg).unwrap();
        let fake = FakeHost::new(a.lock_shared.clone());
        a.lock = LockScreen::new(Box::new(fake.clone()), a.lock_shared.clone(), None);
        a.parent_state = std::env::temp_dir().join(format!(
            "ost-runner-lock-{}-{}.json",
            std::process::id(),
            rand::random::<u32>()
        ));
        a.tracker = screentime::UsageTracker::new();
        a.tracker
            .roll_to(a.trusted_now.with_timezone(&chrono::Local).date_naive());
        a.frozen.clear();
        a.pending_freeze.clear();
        a.device_locked = false;
        a.tamper_lockdown = false;
        let mut p = Policy::default();
        p.screen_time.enabled = true;
        p.screen_time.daily_limit_minutes = 60;
        a.policies.insert("mia".into(), p);
        a.tracker.add_active("mia", 61 * 60, 1);
        {
            let mut w = fake.w();
            w.vt = 2;
            w.logged_in.insert("mia".into());
            w.sessions = vec![session("2", "mia", 2, true)];
        }
        (a, fake)
    }

    fn pos(log: &[String], what: &str) -> usize {
        log.iter()
            .position(|l| l == what)
            .unwrap_or_else(|| panic!("{what:?} not in {log:?}"))
    }

    fn reason(a: &Agent) -> LockReason {
        a.rules_now("mia", &a.policies["mia"]).unwrap()
    }

    #[tokio::test]
    async fn the_lock_goes_up_before_the_freeze_and_down_after_the_thaw() {
        let (mut a, fake) = agent_with_mia();
        a.prev_active = Some(["mia".to_string()].into_iter().collect());
        a.announced.insert("mia".into(), Instant::now()); // the 1-minute warning went out
        let r = reason(&a);
        let mut ev = Vec::new();
        a.screen_time_lockout("mia", &r, &mut ev).await;
        assert!(a.frozen.contains("mia"));
        assert_eq!(a.lock.subject(), Some("mia"));
        let log = fake.w().log.clone();
        // On screen first (the desktop still alive), frozen after.
        assert!(pos(&log, "switch 13") < pos(&log, "freeze mia"));
        assert_eq!(a.face_for("mia").title, "Time's up for today");

        // Any thaw path (here: time granted, then the tick's Unfreeze) takes
        // the lock down — after the thaw, back to her own session.
        fake.w().log.clear();
        a.tracker.add_earned("mia", 30);
        a.lock.host().freeze("mia", false, false);
        a.frozen.remove("mia");
        a.reconcile_lock().await;
        assert!(a.lock.shown().is_none());
        let log = fake.w().log.clone();
        assert!(pos(&log, "thaw mia") < pos(&log, "switch 2"));
        assert_eq!(fake.w().vt, 2);
    }

    #[tokio::test]
    async fn a_code_typed_at_the_lock_is_checked_here_and_unlocks() {
        let (mut a, fake) = agent_with_mia();
        a.parent_totp_secret = Some(SECRET.into());
        a.prev_active = Some(HashSet::new()); // she just logged in: no grace
        let r = reason(&a);
        let mut ev = Vec::new();
        a.screen_time_lockout("mia", &r, &mut ev).await;
        assert!(a.frozen.contains("mia"));

        // A wrong code: refused, one try fewer, still locked.
        let reply = a
            .on_lock_request(Request::Code {
                code: "000000".into(),
            })
            .await;
        assert!(!reply.result.unwrap().ok);
        let face = reply.face.expect("still locked");
        assert_eq!(face.code, lock::CodeState::Ready { tries_left: 4 });
        assert!(a.frozen.contains("mia"));

        // The code the console shows right now.
        let key = parentcode::base32_decode(SECRET).unwrap();
        let counter = chrono::Utc::now().timestamp() as u64 / parentcode::STEP_SECS;
        let code = parentcode::totp_at(&key, counter);
        fake.w().log.clear();
        let reply = a.on_lock_request(Request::Code { code }).await;
        assert!(reply.result.unwrap().ok);
        assert!(reply.face.is_none(), "the lock is down");
        assert!(!a.frozen.contains("mia"));
        assert!(
            a.tracker.peek_override("mia", chrono::Utc::now()).is_some(),
            "the code wrote the one override"
        );
        let log = fake.w().log.clone();
        assert!(pos(&log, "thaw mia") < pos(&log, "switch 2"));
        // The console hears how.
        assert!(a
            .pending_events
            .iter()
            .any(|e| e.ev_type == parentcode::EV_PARENT_CODE_OK));
    }

    #[tokio::test]
    async fn logged_out_users_are_never_frozen() {
        let (mut a, fake) = agent_with_mia();
        {
            let mut w = fake.w();
            w.logged_in.clear();
            w.sessions.clear();
        }
        let r = reason(&a);
        let mut ev = Vec::new();
        a.screen_time_lockout("mia", &r, &mut ev).await;
        a.stop_user("mia", true).await;
        assert!(!a.frozen.contains("mia"));
        assert!(a.pending_freeze.is_empty());
        assert!(a.lock.shown().is_none());
        assert!(fake.w().log.is_empty(), "nothing done: {:?}", fake.w().log);
        assert!(ev.is_empty(), "no 'time ran out' for someone who wasn't on");
    }

    /// Stop mia at her limit, the lock up in front of her, the lock's
    /// one-minute wait already over.
    async fn stop_mia(a: &mut Agent) {
        a.prev_active = Some(HashSet::new());
        let r = reason(a);
        let mut ev = Vec::new();
        a.screen_time_lockout("mia", &r, &mut ev).await;
        assert!(a.frozen.contains("mia"));
        a.lock_since = Some((
            "mia".into(),
            Instant::now() - Duration::from_secs(lock::SNOOZE_WAIT_SECS + 1),
        ));
    }

    fn kind_of(e: &Event) -> &str {
        e.payload
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or_default()
    }

    fn mia_bundle() -> crate::policy::PolicyBundle {
        let mut p = Policy::default();
        p.screen_time.enabled = true;
        p.screen_time.daily_limit_minutes = 60;
        crate::policy::PolicyBundle {
            policy_version: "7".into(),
            device_tamper_level: 1,
            users: vec![crate::policy::UserPolicy {
                os_username: "mia".into(),
                profile_kind: "kid".into(),
                policy: p,
                self_managed: false,
            }],
            vpn: None,
            parent_code: Some(crate::policy::ParentCode {
                totp_secret: SECRET.into(),
                recovery_codes: Vec::new(),
            }),
        }
    }

    /// A stock Debian desktop has neither dnsmasq nor nftables. The rules
    /// still arrive whole — held, the unlock code set up, screen time
    /// enforced — resolv.conf is never pinned to a resolver that isn't
    /// there, and the computer says so: degraded, once, and the `state`
    /// frame the console reads is no longer "doing what it should".
    #[tokio::test]
    async fn a_computer_without_dnsmasq_or_nftables_keeps_its_rules() {
        if crate::config::is_root() {
            return; // apply_bundle writes its caches under /etc
        }
        let (mut a, _fake) = agent_with_mia();
        a.policies.clear();
        a.exec = Exec::simulated(
            &["nft", "dnsmasq"],
            &[("systemctl is-active dnsmasq", "inactive\n")],
        );
        let evs = a
            .apply_bundle(mia_bundle())
            .expect("never aborts on the network");
        assert!(a.policies.contains_key("mia"));
        assert_eq!(a.parent_totp_secret.as_deref(), Some(SECRET));
        assert!(evs.iter().any(|e| e.ev_type == EV_POLICY_APPLIED));
        let degraded: Vec<&str> = evs
            .iter()
            .filter(|e| e.ev_type == EV_ENFORCEMENT_DEGRADED)
            .map(kind_of)
            .collect();
        assert_eq!(degraded, ["dns_resolver_missing", "firewall_not_installed"]);
        assert!(!evs.iter().any(|e| e.ev_type == EV_TAMPER));
        let st = a.device_state();
        assert!(!st.enforcing);
        assert_eq!(st.gaps, ["dns_resolver_missing", "firewall_not_installed"]);
        assert!(
            !a.exec
                .log()
                .iter()
                .any(|l| l == "write /etc/resolv.conf" || l == "run chattr +i /etc/resolv.conf"),
            "{:?}",
            a.exec.log()
        );

        // The next pull with the same gaps is not news.
        let evs = a.apply_bundle(mia_bundle()).unwrap();
        assert!(!evs.iter().any(|e| e.ev_type == EV_ENFORCEMENT_DEGRADED));

        // Screen time still bites: she is over her 60 minutes.
        stop_mia(&mut a).await;
        assert!(a.frozen.contains("mia"));
    }

    /// nftables there but refusing the ruleset: a firewall gap. The whole
    /// network apply failing (its ruleset can't even be written): a gap of
    /// its own. Never an abort — the rules are held either way.
    #[tokio::test]
    async fn a_network_apply_that_fails_is_a_gap_not_an_abort() {
        if crate::config::is_root() {
            return;
        }
        let (mut a, _fake) = agent_with_mia();
        a.policies.clear();
        let running = [("systemctl is-active dnsmasq", "active\n")];
        a.exec = Exec::simulated(&[], &running).failing(&["nft"]);
        a.apply_bundle(mia_bundle())
            .expect("never aborts on the network");
        assert_eq!(a.standing_gaps, ["firewall_not_applied"]);
        assert!(
            !a.firewall_loaded(),
            "a missing table is the known gap, not a flush"
        );

        a.policies.clear();
        a.exec = Exec::simulated(&[], &running)
            .failing(&["write:/etc/openscreentime/dnsmasq.d/openscreentime.conf"]);
        let evs = a
            .apply_bundle(mia_bundle())
            .expect("never aborts on the network");
        assert!(a.policies.contains_key("mia"));
        assert_eq!(a.standing_gaps, [GAP_NETWORK_APPLY_FAILED]);
        assert!(evs.iter().any(
            |e| e.ev_type == EV_ENFORCEMENT_DEGRADED && kind_of(e) == GAP_NETWORK_APPLY_FAILED
        ));
    }

    /// Removed from its household: the person the rules stopped is thawed,
    /// the lock comes down (back to her session), the network rules go, the
    /// helper that finishes the job outside the sandbox is started — and the
    /// agent never enforces again.
    #[tokio::test]
    async fn a_retired_computer_frees_the_person_it_stopped() {
        let (mut a, fake) = agent_with_mia();
        let pinned = crate::enforce::dns::render_resolv_conf();
        a.exec = Exec::simulated(&[], &[("read /etc/resolv.conf", pinned.as_str())]);
        stop_mia(&mut a).await;
        assert!(a.lock.shown().is_some());
        fake.w().log.clear();

        a.retire().await;
        assert!(a.frozen.is_empty() && a.policies.is_empty());
        assert!(a.lock.shown().is_none());
        let log = fake.w().log.clone();
        assert!(pos(&log, "thaw mia") < pos(&log, "switch 2"), "{log:?}");
        assert_eq!(fake.w().vt, 2);

        let ex = a.exec.log();
        for step in [
            "run nft delete table inet openscreentime",
            "run chattr -i /etc/resolv.conf",
            "write /etc/resolv.conf",
            "remove /etc/polkit-1/rules.d/49-openscreentime.rules",
            "remove /etc/openscreentime/policy_bundle.json",
            "write /var/lib/openscreentime/retired",
            "run systemd-run --quiet --collect --unit=openscreentime-retire \
             /usr/local/bin/openscreentime __retire",
        ] {
            pos(&ex, step);
        }
        assert!(!ex.iter().any(|l| l.contains("chattr +i")));

        // Nothing is enforced any more.
        fake.w().log.clear();
        assert!(a.enforcement_tick().await.is_empty());
        a.reconcile_lock().await;
        assert!(fake.w().log.iter().all(|l| !l.starts_with("freeze")));
    }

    #[tokio::test]
    async fn only_someone_who_set_their_own_limit_can_give_themselves_more() {
        let (mut a, _fake) = agent_with_mia();
        // A child: the lock offers "Ask for more time", never the snooze —
        // and a lock that sends the snooze anyway is refused here.
        a.kinds.insert("mia".into(), "kid".into());
        stop_mia(&mut a).await;
        let face = a.face_for("mia");
        assert_eq!(face.snooze, lock::Snooze::Hidden);
        assert_eq!(face.ask, lock::AskState::Ready);
        let reply = a.on_lock_request(Request::Snooze).await;
        assert!(!reply.result.unwrap().ok, "a child can't snooze");
        assert!(a.frozen.contains("mia"), "still stopped");
        assert!(a.tracker.peek_override("mia", a.trusted_now).is_none());

        // An adult at the limit they set: the snooze, not the ask.
        a.kinds.insert("mia".into(), "adult".into());
        let face = a.face_for("mia");
        assert_eq!(face.ask, lock::AskState::Hidden);
        assert_eq!(face.snooze, lock::Snooze::Ready { left: 2 });
        assert_eq!(
            face.detail,
            "You've used the 1 hour you set for today. Screens come back tomorrow."
        );
        assert!(!face.help.contains("parent") && !face.code_hint.contains("parent"));
        for n in 1..=lock::SNOOZES_PER_DAY {
            if n > 1 {
                // The last 15 minutes ran out: stopped again.
                a.trusted_now += chrono::Duration::minutes(16);
                stop_mia(&mut a).await;
            }
            let reply = a.on_lock_request(Request::Snooze).await;
            assert!(reply.result.unwrap().ok, "snooze {n} is allowed");
            assert!(reply.face.is_none(), "the lock is down");
            assert!(!a.frozen.contains("mia"));
            assert!(a.tracker.peek_override("mia", a.trusted_now).is_some());
            assert_eq!(a.tracker.snoozes("mia"), n);
        }
        let logged = a
            .pending_events
            .iter()
            .filter(|e| e.ev_type == EV_SCREEN_TIME_EARNED)
            .count();
        assert_eq!(logged, 3, "each one is on the record");

        // A fourth: that's today's extra time.
        a.trusted_now += chrono::Duration::minutes(16);
        stop_mia(&mut a).await;
        assert!(matches!(
            a.face_for("mia").snooze,
            lock::Snooze::UsedUp { .. }
        ));
        let reply = a.on_lock_request(Request::Snooze).await;
        assert!(!reply.result.unwrap().ok, "the fourth is refused");
        assert!(a.frozen.contains("mia"));

        // Too soon is refused too, even for an adult.
        let (mut b, _fake) = agent_with_mia();
        b.kinds.insert("mia".into(), "adult".into());
        stop_mia(&mut b).await;
        b.lock_since = Some(("mia".into(), Instant::now()));
        assert!(matches!(
            b.face_for("mia").snooze,
            lock::Snooze::Wait { .. }
        ));
        let reply = b.on_lock_request(Request::Snooze).await;
        assert!(!reply.result.unwrap().ok);
        assert!(b.frozen.contains("mia"));
    }

    #[tokio::test]
    async fn logging_in_to_a_stop_meets_the_lock_at_once() {
        let (mut a, fake) = agent_with_mia();
        a.prev_active = Some(HashSet::new());
        // The VT watcher saw her session come on screen.
        a.on_lock_event(LockEvent::VtChanged).await;
        assert!(a.frozen.contains("mia"));
        assert!(a.pending_freeze.is_empty(), "no grace: nothing to save yet");
        let log = fake.w().log.clone();
        assert!(pos(&log, "switch 13") < pos(&log, "freeze mia"));
    }

    #[tokio::test]
    async fn a_sudden_stop_gets_a_countdown_not_a_takeover() {
        let (mut a, fake) = agent_with_mia();
        a.prev_active = Some(["mia".to_string()].into_iter().collect());
        let r = reason(&a);
        let mut ev = Vec::new();
        a.screen_time_lockout("mia", &r, &mut ev).await;
        assert!(a.pending_freeze.contains_key("mia"));
        assert!(!a.frozen.contains("mia"));
        assert!(a.lock.shown().is_none());
        assert!(fake.w().log.is_empty());
        // The countdown is what the companion counts down from.
        a.update_forecasts(&["mia".to_string()]);
        let f = a.forecasts["mia"];
        assert_eq!(f.0, warn::StopReason::Limit);
        assert!((f.1 - chrono::Local::now()).num_seconds() > 0);
    }

    #[tokio::test]
    async fn a_restart_adopts_the_freeze_and_thaws_strangers() {
        let (mut a, fake) = agent_with_mia();
        {
            let mut w = fake.w();
            w.logged_in.insert("dad".into());
            w.frozen.insert("mia".into(), true);
            w.frozen.insert("dad".into(), true);
        }
        a.adopt_frozen();
        assert!(a.frozen.contains("mia"), "a managed user stays stopped");
        assert!(!a.frozen.contains("dad"));
        assert_eq!(
            fake.w().frozen.get("dad"),
            Some(&false),
            "nobody else stays frozen"
        );
        // With mia on screen and frozen, the lock comes back for her.
        a.reconcile_lock().await;
        assert_eq!(a.lock.subject(), Some("mia"));
    }

    #[tokio::test]
    async fn a_resume_for_someone_else_leaves_a_rule_stop_in_place() {
        let (mut a, fake) = agent_with_mia(); // over her 60 minutes
        let cmd = |t: &str, payload| Command {
            id: "c1".into(),
            cmd_type: t.into(),
            payload,
        };
        let _ = a.handle_command(cmd(CMD_LOCK, json!({}))).await;
        assert_eq!(a.face_for("mia").title, "Paused by a parent");
        let _ = a
            .handle_command(cmd(CMD_UNLOCK, json!({ "os_username": "sib" })))
            .await;
        a.reconcile_lock().await;
        // Still stopped by her limit: no thaw, no "You're back"; the lock
        // just says why now.
        assert!(a.frozen.contains("mia"));
        assert_eq!(a.lock.subject(), Some("mia"));
        assert_eq!(a.face_for("mia").title, "Time's up for today");
        assert!(!fake.w().log.contains(&"thaw mia".to_string()));
    }

    /// Acceptance, step 5: after the unlock code Mia's window said "0 min
    /// left" in red — it read the spent budget — while the rules gave her 30
    /// minutes. The status publishes the override's time as time left, and
    /// the console hears the override in the state frame.
    #[tokio::test]
    async fn status_after_the_unlock_code_shows_the_override_minutes() {
        let (mut a, _fake) = agent_with_mia(); // 61 of 60 minutes used
        a.parent_totp_secret = Some(SECRET.into());
        a.prev_active = Some(HashSet::new());
        let r = reason(&a);
        let mut ev = Vec::new();
        a.screen_time_lockout("mia", &r, &mut ev).await;
        let key = parentcode::base32_decode(SECRET).unwrap();
        let counter = chrono::Utc::now().timestamp() as u64 / parentcode::STEP_SECS;
        let code = parentcode::totp_at(&key, counter);
        let reply = a.on_lock_request(Request::Code { code }).await;
        assert!(reply.result.unwrap().ok);

        let p = a.policies["mia"].clone();
        let s = a.user_status("mia", &p);
        assert_eq!(s["allowed"], true);
        assert_eq!(s["remaining_minutes"], -1, "the budget is spent…");
        assert_eq!(s["minutes_left"], 30, "…and the code gives 30");
        assert!(s["override_until"].is_string());
        assert_eq!(s["stop_at"], s["override_until"]);
        // What every surface reads from it.
        let clock: crate::glance::Clock = serde_json::from_value(s.clone()).unwrap();
        match clock.left(chrono::Local::now()) {
            crate::glance::Left::Minutes {
                minutes,
                unlocked_until: Some(_),
            } => assert!((29..=30).contains(&minutes)),
            other => panic!("{other:?}"),
        }
        // The console counts from the same override.
        let st = a.device_state();
        assert!(st.overrides.contains_key("mia"));
    }

    /// Acceptance, step 6a: "Give 15" on a day already 5 minutes over said
    /// "You have 10 minutes left today" (the budget) while the warnings said
    /// 15, ending 00:27 — and the lock came at 00:27. One number: the stop.
    #[test]
    fn a_grant_on_an_overused_day_says_the_stop_it_will_keep() {
        let (mut a, _fake) = agent_with_mia();
        a.tracker = screentime::UsageTracker::new();
        a.tracker
            .roll_to(a.trusted_now.with_timezone(&chrono::Local).date_naive());
        let mut p = Policy::default();
        p.screen_time.enabled = true;
        p.screen_time.daily_limit_minutes = 5;
        a.policies.insert("mia".into(), p.clone());
        a.tracker.add_active("mia", 10 * 60, 1);
        a.tracker.grant("g1", "mia", 15, a.trusted_now);
        assert_eq!(a.tracker.remaining_minutes("mia", &p), Some(10));
        let v = a.stop_verdict("mia", &p);
        assert_eq!(v.minutes_left, Some(15));
        let words = back_words(&v);
        assert!(words.starts_with("You have 15 min, until "), "{words}");
        let s = a.user_status("mia", &p);
        assert_eq!(s["minutes_left"], 15);
        assert_eq!(s["stop_at"], s["override_until"]);
    }

    /// Acceptance, step 6a: Mia pressed "Ask" at the lock and the console
    /// read "Read for 20 min" (the first earn task); after Give 15 her ask
    /// still said "waiting". The ask is a plain ask, and a grant answers it.
    #[tokio::test]
    async fn a_plain_ask_is_filed_as_one_and_a_grant_answers_it() {
        let (mut a, _fake) = agent_with_mia();
        let mut p = a.policies["mia"].clone();
        p.gamification.earn_time.enabled = true;
        p.gamification.earn_time.tasks = vec![crate::policy::EarnTask {
            id: "reading".into(),
            label: "Read for 20 min".into(),
            reward_minutes: 15,
        }];
        a.policies.insert("mia".into(), p.clone());
        let offer = a.earn_offer_for("mia");
        assert_eq!(
            (offer.id.as_str(), offer.label.as_str()),
            ("ask", "Asked for more time")
        );
        // The agent filed it (the server call is the network's business).
        a.requested_earn.insert(
            ("mia".into(), offer.id.clone()),
            chrono::Local::now().date_naive(),
        );
        assert_eq!(a.user_status("mia", &p)["ask_pending"], true);
        let (ack, _) = a
            .handle_command(Command {
                id: "g1".into(),
                cmd_type: CMD_CREDIT_TIME.into(),
                payload: json!({ "os_username": "mia", "minutes": 15, "request_id": null }),
            })
            .await;
        assert_eq!(ack.result["credited"], true);
        assert_eq!(
            a.user_status("mia", &p)["ask_pending"],
            false,
            "answered: the window offers Ask again"
        );
    }

    #[test]
    fn billing_carries_the_fraction_of_a_second() {
        let mut carry = Duration::ZERO;
        let mut billed = 0;
        for _ in 0..30 {
            let (whole, c) = whole_seconds(Duration::from_millis(10_040) + carry);
            billed += whole.as_secs();
            carry = c;
        }
        assert_eq!(billed, 301, "30 ticks of 10.04 s bill 301 s, not 300");
        // A stop just ahead of the next tick gets its own; one far off doesn't.
        let now = Instant::now();
        let wake = stop_wake(now + Duration::from_secs(3), now).unwrap();
        assert_eq!(wake, now + Duration::from_secs(3) + STOP_SLACK);
        assert!(stop_wake(now + Duration::from_secs(30), now).is_none());
        assert!(stop_wake(now, now).unwrap() >= now + Duration::from_millis(250));
    }

    #[tokio::test]
    async fn resume_from_the_console_takes_the_lock_down() {
        // Within her limit: Resume ends the pause, she's thawed at once and
        // the lock comes down.
        let (mut a, fake) = agent_with_mia();
        a.tracker = screentime::UsageTracker::new();
        a.tracker
            .roll_to(a.trusted_now.with_timezone(&chrono::Local).date_naive());
        a.tracker.add_active("mia", 20 * 60, 1);
        a.prev_active = Some(HashSet::new());
        let cmd = |t: &str| Command {
            id: "c1".into(),
            cmd_type: t.into(),
            payload: json!({}),
        };
        let (_ack, _ev) = a.handle_command(cmd(CMD_LOCK)).await;
        assert!(a.frozen.contains("mia"));
        assert_eq!(a.face_for("mia").title, "Paused by a parent");
        let (ack, _ev) = a.handle_command(cmd(CMD_UNLOCK)).await;
        a.reconcile_lock().await;
        assert!(a.lock.shown().is_none());
        let log = fake.w().log.clone();
        assert!(pos(&log, "thaw mia") < pos(&log, "switch 2"));
        assert_eq!(ack.result["override_users"], json!([]), "nothing given");
    }

    /// Acceptance, step 6b/7a: Resume on a computer whose child's time was
    /// up handed her 30 free minutes (and 30 more to a login nobody had
    /// paused, not even signed in). Resume ends a pause and nothing else.
    #[tokio::test]
    async fn resume_on_a_time_up_day_gives_no_time() {
        let (mut a, _fake) = agent_with_mia(); // 61 of her 60 minutes used
        let mut dad = Policy::default();
        dad.screen_time.enabled = true;
        dad.screen_time.bedtime = Some(openscreentime_policy::Bedtime {
            start: "00:00".into(),
            end: "23:59".into(),
        });
        a.policies.insert("philip".into(), dad);
        a.prev_active = Some(HashSet::new());
        let cmd = |t: &str| Command {
            id: "c1".into(),
            cmd_type: t.into(),
            payload: json!({}),
        };
        let _ = a.handle_command(cmd(CMD_LOCK)).await;
        let (ack, ev) = a.handle_command(cmd(CMD_UNLOCK)).await;
        a.reconcile_lock().await;
        assert_eq!(ack.result["locked"], json!(false), "the pause is over");
        assert_eq!(ack.result["override_users"], json!([]));
        assert!(ev.iter().all(|e| e.payload["override_users"] == json!([])));
        for u in ["mia", "philip"] {
            assert!(
                a.tracker.peek_override(u, a.trusted_now).is_none(),
                "{u} was given time"
            );
        }
        // Mia is still at her limit: stopped, and the lock says why.
        assert!(!a.stop_verdict("mia", &a.policies["mia"].clone()).allowed);
        assert!(a.frozen.contains("mia"));
        assert_eq!(a.face_for("mia").title, "Time's up for today");
    }

    fn tamper_agent(tamper_max: bool, cfg_level: u8) -> Agent {
        let ctx = AgentCtx::new(true, tamper_max, 1);
        let cfg = AgentConfig {
            server_url: "http://127.0.0.1:9".into(),
            device_id: "d".into(),
            device_token: "t".into(),
            poll_interval_secs: 30,
            tamper_level: cfg_level,
            auto_update: false,
        };
        Agent::new(ctx, cfg).unwrap()
    }

    fn capped_events(evs: &[Event]) -> usize {
        evs.iter()
            .filter(|e| e.payload["kind"] == "tamper_level_capped")
            .count()
    }

    #[tokio::test]
    async fn set_tamper_level_3_without_the_flag_is_capped_and_says_so() {
        // agent.toml can't smuggle level 3 in either.
        let mut a = tamper_agent(false, 3);
        assert_eq!(a.tamper_level, 1);
        assert_eq!(capped_events(&a.pending_events), 1);
        let (ack, evs) = a
            .handle_command(Command {
                id: "c1".into(),
                cmd_type: CMD_SET_TAMPER_LEVEL.into(),
                payload: json!({ "level": 3 }),
            })
            .await;
        assert_eq!(ack.status, "acked");
        assert_eq!(a.tamper_level, 1);
        assert_eq!(ack.result["tamper_level"], 1);
        assert_eq!(ack.result["requested"], 3);
        assert_eq!(ack.result["capped"], true);
        assert_eq!(capped_events(&evs), 1);
        // No level-3 hardening happened, so no level-3 guidance either.
        assert!(!evs.iter().any(|e| e.payload["kind"] == "boot_guidance"));

        // The same capped request riding every policy bundle is said once.
        let (_, evs, _) = a.adopt_tamper_level(3);
        assert_eq!(capped_events(&evs), 0);
        assert_eq!(a.tamper_level, 1);
    }

    #[tokio::test]
    async fn set_tamper_level_3_with_the_flag_is_applied() {
        let mut a = tamper_agent(true, 1);
        assert_eq!(a.tamper_level, 3, "--tamper-max starts at 3");
        let lower = |level: u8| Command {
            id: "c1".into(),
            cmd_type: CMD_SET_TAMPER_LEVEL.into(),
            payload: json!({ "level": level }),
        };
        let (ack, _) = a.handle_command(lower(1)).await;
        assert_eq!(
            (a.tamper_level, &ack.result),
            (1, &json!({ "tamper_level": 1 }))
        );
        let (ack, evs) = a.handle_command(lower(3)).await;
        assert_eq!(a.tamper_level, 3);
        assert_eq!(ack.result, json!({ "tamper_level": 3 }));
        assert_eq!(capped_events(&evs), 0);
        assert!(evs.iter().any(|e| e.payload["kind"] == "boot_guidance"));
    }
}

/// Focus hours: a self-managed person's own blocked sites join the host's
/// blocks while their window holds, and leave when it ends.
#[cfg(test)]
mod focus_tests {
    use super::*;
    use crate::policy::{Focus, Window};
    use chrono::TimeZone;

    fn agent() -> Agent {
        let ctx = AgentCtx::new(true, false, 1);
        let cfg = AgentConfig {
            server_url: "http://127.0.0.1:9".into(),
            device_id: "d".into(),
            device_token: "t".into(),
            poll_interval_secs: 30,
            tamper_level: 1,
            auto_update: false,
        };
        Agent::new(ctx, cfg).unwrap()
    }

    /// 2026-09-21 is a Monday.
    fn at_local(h: u32, m: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::Local
            .with_ymd_and_hms(2026, 9, 21, h, m, 0)
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn focused(sites: &[&str], hours: Option<(&str, &str)>) -> Policy {
        Policy {
            focus: Focus {
                sites: sites.iter().map(|s| s.to_string()).collect(),
                hours: hours.map(|(s, e)| Window {
                    days: vec![1, 2, 3, 4, 5],
                    start: s.into(),
                    end: e.into(),
                }),
            },
            ..Default::default()
        }
    }

    #[test]
    fn own_sites_are_blocked_inside_focus_hours_only() {
        let mut a = agent();
        a.policies.insert(
            "jonas".into(),
            focused(&["reddit.com"], Some(("09:00", "12:00"))),
        );
        let mut kid = Policy::default();
        kid.blocks.custom_domains = vec!["example.org".into()];
        a.policies.insert("mia".into(), kid);

        a.trusted_now = at_local(10, 0);
        let on = a.effective_network_policy();
        assert_eq!(
            on.blocks.custom_domains,
            vec!["example.org".to_string(), "reddit.com".to_string()]
        );
        assert!(
            on.lockdown.force_dns,
            "a block brings the anti-bypass posture"
        );
        assert_eq!(a.focus_now(), vec!["jonas".to_string()]);

        a.trusted_now = at_local(12, 0);
        let off = a.effective_network_policy();
        assert_eq!(off.blocks.custom_domains, vec!["example.org".to_string()]);
        assert!(a.focus_now().is_empty());
    }

    #[test]
    fn no_hours_means_all_day_and_no_sites_means_nothing() {
        let mut a = agent();
        a.policies
            .insert("jonas".into(), focused(&["youtube.com"], None));
        a.trusted_now = at_local(3, 0);
        let p = a.effective_network_policy();
        assert_eq!(p.blocks.custom_domains, vec!["youtube.com".to_string()]);
        assert!(a.wants_force_dns());

        let mut a = agent();
        a.policies
            .insert("jonas".into(), focused(&[], Some(("09:00", "12:00"))));
        a.trusted_now = at_local(10, 0);
        assert!(a.effective_network_policy().blocks.is_empty());
        assert!(!a.wants_force_dns());
    }

    #[test]
    fn the_network_is_reapplied_when_focus_begins_and_ends_once() {
        let mut a = agent();
        a.policies.insert(
            "jonas".into(),
            focused(&["reddit.com"], Some(("09:00", "12:00"))),
        );
        a.trusted_now = at_local(8, 59);
        assert!(!a.focus_flipped(), "nothing blocking yet, nothing applied");
        a.trusted_now = at_local(9, 0);
        assert!(a.focus_flipped(), "focus began");
        assert!(!a.focus_flipped(), "…once");
        a.trusted_now = at_local(11, 30);
        assert!(!a.focus_flipped());
        a.trusted_now = at_local(12, 0);
        assert!(a.focus_flipped(), "focus ended");
        // A failed re-apply is retried.
        a.focus_applied = None;
        assert!(a.focus_flipped());
    }
}
