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

fn degraded_events(gaps: &[enforce::Gap]) -> Vec<Event> {
    gaps.iter()
        .map(|gap| {
            Event::new(
                EV_ENFORCEMENT_DEGRADED,
                SEV_CRITICAL,
                json!({ "kind": gap.kind(), "detail": gap.explain() }),
            )
        })
        .collect()
}

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

fn read_local_recovery_marker() -> Option<u64> {
    std::fs::read_to_string(local_recovery_marker_path())
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Called by `ost unlock` (a separate process): clear the persisted lock so a
/// reboot doesn't reload it, and leave a marker the live agent picks up.
pub fn record_local_recovery() {
    save_device_locked(false);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = local_recovery_marker_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, now.to_string()) {
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
    /// The device's unlock-code secret from the last bundle.
    parent_totp_secret: Option<String>,
    /// Unused one-time recovery codes from the last bundle.
    parent_recovery: Vec<crate::policy::RecoveryCode>,
    /// (user, app) → date an `app_blocked` event was already emitted.
    app_reported: HashMap<(String, String), chrono::NaiveDate>,
    /// Standing enforcement gap kinds from the last network apply.
    standing_gaps: Vec<String>,
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
    /// Effective tamper level (max of device policy and --tamper-max).
    tamper_level: u8,
    policy_version: String,
    /// Expected wall-clock at the next tick (clock-skew / time-tamper detection).
    expected_wall: Option<chrono::DateTime<chrono::Utc>>,
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
    clock_ahead_reported: bool,
    no_credential_reported: bool,
    /// Confirmation gate that separates a real, sustained evasion attempt from a
    /// transient blip before escalating to `tamper_lockdown`.
    tamper_monitor: tamper::TamperMonitor,
    /// Verified-unlock grace windows (user → expiry). Fed by a code typed at
    /// the lock; while active, the user is treated as within policy
    /// (screen-time AND admin lock — the parent always wins).
    unlock_until: HashMap<String, Instant>,
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
    /// The next stop per user, as published in the status snapshot.
    forecasts: HashMap<String, warn::Forecast>,
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
    /// Web sign-in requests waiting for a human at this machine to answer
    /// (CONTRACT-0.6 client-first login). Published per target user in the
    /// status snapshot; answered via a marker file in `/run/user/<uid>`.
    pending_logins: Vec<PendingLogin>,
    /// Where-the-time-goes sampler (apps by /proc, sites by dnsmasq log).
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
}

/// One outstanding "approve this web sign-in?" prompt.
#[derive(Debug, Clone)]
struct PendingLogin {
    /// The server's `login_requests.id` (opaque here).
    id: String,
    /// Display name of the person signing in — what the prompt shows.
    username: String,
    /// The OS logins on this device that belong to that person; only their
    /// sessions see the prompt, and only their decision files are honored.
    os_users: Vec<String>,
    /// Three 4-digit codes to show the human (number-matching); the browser
    /// shows the one real code and the human taps the match. The device is not
    /// told which is real — the server decides on the tapped value.
    codes: Vec<String>,
    /// The number the human tapped, once read from the decision file — held in
    /// MEMORY so a transient POST failure is retried from here, never
    /// re-written back into the child-owned runtime dir (that write followed a
    /// child-planted symlink → root file write; the retry-via-file is gone).
    /// `Some("")` = "not me" (deny); `Some(code)` = tapped that number.
    decision: Option<String>,
    expires: chrono::DateTime<chrono::Utc>,
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
            tamper_level: cfg
                .tamper_level
                .max(if ctx.tamper_max >= 3 { 3 } else { 1 }),
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
            expected_wall: None,
            requested_earn: HashMap::new(),
            last_contact: Instant::now(),
            contact_state: ContactState::Online,
            offline_grace: offline_grace_from_env(),
            last_contact_wall: load_last_contact_wall(),
            last_contact_saved: Instant::now(),
            offline_hard_lockdown: false,
            tamper_lockdown: carried.tamper_lockdown,
            last_local_recovery: read_local_recovery_marker(),
            device_lock_grace_until: None,
            local_net_up: true,
            clock_ahead_reported: false,
            no_credential_reported: false,
            tamper_monitor: tamper::TamperMonitor::new(),
            unlock_until: HashMap::new(),
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
            pending_logins: Vec::new(),
            attrib: crate::attrib::Attrib::new(),
            attrib_ticks: 0,
            probe_reported: HashMap::new(),
            dns_relaxed: false,
            dns_unreach_ticks: 0,
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

    /// Deliver `fresh` events plus any earlier failures. On error the batch is
    /// kept for the next attempt (see `pending_events`) instead of dropped.
    async fn flush_events(&mut self, fresh: Vec<Event>) {
        self.pending_events.extend(fresh);
        if self.pending_events.is_empty() {
            return;
        }
        if self.pending_events.len() > PENDING_EVENTS_CAP {
            let excess = self.pending_events.len() - PENDING_EVENTS_CAP;
            self.pending_events.drain(..excess);
        }
        // Post in server-sized batches, dropping each only once it lands.
        //
        // Posting the whole buffer in one request was a trap: the server rejects
        // any batch over MAX_EVENTS (100) with a 400, which `error_for_status`
        // turns into an error, so the batch was kept — and a buffer that has
        // once exceeded 100 can never shrink again. Roughly 17 minutes offline
        // is enough to cross it, because the offline re-assert emits a degraded
        // event per standing gap every 10s tick. After that every event post
        // fails forever while heartbeats keep succeeding, so the device looks
        // healthy and the entire tamper/audit trail is silently discarded —
        // which is precisely the data this buffer exists to protect.
        while !self.pending_events.is_empty() {
            let take = self.pending_events.len().min(EVENT_BATCH_MAX);
            let batch: Vec<Event> = self.pending_events[..take].to_vec();
            match self.client.post_events(&batch).await {
                Ok(()) => {
                    self.pending_events.drain(..take);
                }
                Err(e) => {
                    // warn, not debug: a stalled audit pipeline is exactly the
                    // kind of quiet failure this codebase keeps getting bitten by.
                    tracing::warn!(
                        "event post failed, {} buffered for retry: {e}",
                        self.pending_events.len()
                    );
                    return;
                }
            }
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
            let effective = self.effective_network_policy();
            let server_host = crate::client::server_host(&self.cfg.server_url);
            match enforce::apply_network_policy(
                self.ctx.clone(),
                &self.exec,
                server_host.as_deref(),
                &effective,
                &enforce::vpn::VpnState::Sync(self.vpn.as_ref()),
            ) {
                Ok((gaps, report)) => {
                    events.extend(degraded_events(&gaps));
                    events.extend(vpn_report_event(report));
                }
                Err(e) => tracing::warn!("offline fail-closed policy re-assert failed: {e}"),
            }
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
        if bundle.device_tamper_level > self.tamper_level && self.ctx.tamper_max >= 3 {
            self.tamper_level = bundle.device_tamper_level;
        } else if bundle.device_tamper_level > self.tamper_level {
            self.tamper_level = bundle.device_tamper_level.min(3);
        }
        self.policies.clear();
        self.kinds.clear();
        for up in bundle.users {
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
        // DNS/nftables are host-global: apply the most restrictive effective policy.
        let effective = self.effective_network_policy();
        let server_host = crate::client::server_host(&self.cfg.server_url);
        let (gaps, vpn_report) = enforce::apply_network_policy(
            self.ctx.clone(),
            &self.exec,
            server_host.as_deref(),
            &effective,
            &enforce::vpn::VpnState::Sync(self.vpn.as_ref()),
        )?;
        // Best-effort cache so `ost unlock` can work without a live
        // agent process or server connection (parent PIN + recovery teardown).
        crate::policy::save_cache(&effective);
        // …and the whole bundle, so a reboot while the server is unreachable
        // re-enforces the last known policy instead of coming up wide open.
        // Cached only after a successful apply, and cached verbatim — rebuilding
        // it from `self.policies` would silently drop `profile_kind`.
        crate::policy::save_bundle_cache(&cacheable);
        tracing::info!(
            "policy v{} applied for {} user(s)",
            self.policy_version,
            self.policies.len()
        );
        self.standing_gaps = gaps.iter().map(|g| g.kind().to_string()).collect();
        // "Applied" is reported alongside, not instead of, the gaps: the policy
        // really was written, it just isn't all being enforced.
        let mut events = vec![Event::new(
            EV_POLICY_APPLIED,
            SEV_INFO,
            json!({
                "policy_version": self.policy_version,
                "users": self.policies.len(),
                "dns_gaps": gaps.len(),
            }),
        )];
        events.extend(degraded_events(&gaps));
        events.extend(vpn_report_event(vpn_report));
        Ok(events)
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
        for p in self.policies.values() {
            blocks.apps.extend(p.blocks.apps.iter().cloned());
            blocks
                .categories
                .extend(p.blocks.categories.iter().cloned());
            blocks
                .custom_domains
                .extend(p.blocks.custom_domains.iter().cloned());
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
        self.policies
            .values()
            .any(|p| !p.blocks.is_empty() || !p.dns.blocklist.is_empty())
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
        self.policies
            .keys()
            .map(|u| crate::client::UsageReport {
                os_username: u.clone(),
                used_minutes_today: self.tracker.used_minutes(u),
            })
            .collect()
    }

    async fn enforcement_tick(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        tamper::touch_heartbeat(&self.exec);

        // Clock-skew / time-tamper detection: the tick fires on a monotonic timer, so
        // wall-clock should advance ~TICK each tick. A large deviation means someone
        // moved the system clock (a classic screen-time evasion). We compare against the
        // wall-clock we expected this tick to land on, then arm the next expectation.
        let now = chrono::Utc::now();
        if let Some(expected) = self.expected_wall.take() {
            if let Some(ev) = tamper::clock_skew_event(expected, now) {
                events.push(ev);
            }
        }
        self.expected_wall = Some(now + chrono::Duration::from_std(TICK).unwrap_or_default());

        // Keep DNS filtering from bricking a captive-portal / public-DNS-
        // blocking network; re-apply the (relaxed or restored) policy on a flip.
        let (dns_events, dns_flipped) = self.update_dns_reachability();
        events.extend(dns_events);
        if dns_flipped && !self.exec.dry_run() {
            let effective = self.effective_network_policy();
            let server_host = crate::client::server_host(&self.cfg.server_url);
            if let Ok((gaps, report)) = enforce::apply_network_policy(
                self.ctx.clone(),
                &self.exec,
                server_host.as_deref(),
                &effective,
                &enforce::vpn::VpnState::Sync(self.vpn.as_ref()),
            ) {
                events.extend(degraded_events(&gaps));
                events.extend(vpn_report_event(report));
            }
        }

        // Tamper re-assertion (resolv.conf / nft drift, NM disconnect).
        events.extend(tamper::reassert_all(&self.exec));

        // reassert_all flags a missing nft table (critical event) but can't
        // rebuild it — it has no policy. Repair it here with the effective
        // policy so a flush/delete can't leave the device with NO firewall
        // (fail-open) until the next full policy apply.
        // `Some(true)` only — if the probe itself couldn't run (`None`),
        // applying a ruleset through the same broken spawn path won't work
        // either; the reassert above already reported it, retry next tick.
        if enforce::firewall::table_missing(&self.exec) == Some(true) && !self.exec.dry_run() {
            let effective = self.effective_network_policy();
            let server_host = crate::client::server_host(&self.cfg.server_url);
            match enforce::apply_network_policy(
                self.ctx.clone(),
                &self.exec,
                server_host.as_deref(),
                &effective,
                &enforce::vpn::VpnState::Sync(self.vpn.as_ref()),
            ) {
                Ok((gaps, report)) => {
                    tracing::info!("nft table was missing — re-applied firewall");
                    events.extend(degraded_events(&gaps));
                    events.extend(vpn_report_event(report));
                }
                Err(e) => tracing::warn!("firewall repair after drift failed: {e}"),
            }
        }

        // Fail-closed offline grace: alert + aggressively re-assert last-known
        // policy once we've gone too long without hearing from the server.
        events.extend(self.offline_grace_check());
        // …and the days-scale escalation on top of it (policy-configurable).
        // `ost unlock` ran in another process: honor its recovery marker once.
        if let Some(ts) = read_local_recovery_marker() {
            if self.last_local_recovery != Some(ts) {
                self.last_local_recovery = Some(ts);
                events.extend(self.local_recovery("ost unlock"));
            }
        }
        events.extend(self.offline_hard_lockdown_check());
        if let Some(ev) = tamper::nm_guard_probe(&self.exec) {
            events.push(ev);
        }

        // Confirm sustained evasion (vs. a transient blip) and escalate to a
        // whole-device lockdown. We feed the monitor the tamper-signal kinds
        // seen this tick; a kind that crosses its confirmation threshold is a
        // real attempt (the "check it's real, not a packet drop" gate).
        let kinds: Vec<&str> = events
            .iter()
            .filter(|e| e.ev_type == EV_TAMPER)
            .filter_map(|e| e.payload.get("kind").and_then(|k| k.as_str()))
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

        // Forward clock-jump defense: while on a network, the accounting day
        // may not run ahead of "last server-confirmed day + 1". Genuinely
        // offline (no network) → no ceiling, an honest week away still rolls.
        let ceiling = self.local_net_up.then(|| {
            self.last_contact_wall
                .with_timezone(&chrono::Local)
                .date_naive()
                + chrono::Days::new(1)
        });
        self.tracker.set_day_ceiling(ceiling);
        if self.tracker.clock_ahead_of_ceiling() {
            if !self.clock_ahead_reported {
                self.clock_ahead_reported = true;
                events.push(tamper::tamper_event(
                    "clock_ahead_of_server",
                    SEV_WARN,
                    "the clock is more than a day ahead of the last server-confirmed time — \
                     the daily budget will not reset until the server is reached",
                ));
            }
        } else {
            self.clock_ahead_reported = false;
        }

        // Screen-time: account active seat users, evaluate, freeze/unfreeze.
        let active = screentime::active_seat_users(&self.exec);
        self.active_users = active.clone();
        for user in &active {
            // A frozen user is NOT spending screen time: their processes are
            // suspended at the lock screen, but logind still reports the seat
            // "active", so counting them here burned budget while locked —
            // silently eating an earn-time grant so the freeze could never
            // lift ("granted, but still locked"). Skip them, exactly as the
            // attribution sampler below already does.
            if self.frozen.contains(user) {
                continue;
            }
            self.tracker
                .add_active(user, TICK.as_secs() as u32, self.ctx.time_accel);
        }

        // Where the time goes (CONTRACT-0.6): sample running catalog apps for
        // the active, unfrozen users, and tail the resolver's query log.
        {
            let uids: std::collections::HashMap<String, u32> = active
                .iter()
                .filter(|u| !self.frozen.contains(*u))
                .filter_map(|u| crate::sysusers::uid_of(u).map(|id| (u.clone(), id)))
                .collect();
            self.attrib.sample_apps(&uids, TICK.as_secs() as i64);
            self.attrib.ingest_dns_log();
            self.attrib_ticks += 1;
            if self.attrib_ticks >= 6 {
                self.attrib_ticks = 0;
                let batch = self.attrib.drain(400);
                if !batch.is_empty() {
                    match self.client.post_usage_slices(&batch).await {
                        Ok(()) => {}
                        Err(e) => {
                            tracing::debug!("usage post failed, keeping batch: {e}");
                            self.attrib.requeue(batch);
                        }
                    }
                }
                // The lock must never lie (CONTRACT-0.6 §4): probe that the
                // stops we believe in are real, on the same 1-minute cadence.
                events.extend(self.probe_enforcement());
            }
        }
        // Blocked apps with a native client: deny their processes (CONTRACT-0.4 §7).
        events.extend(enforce::apps::deny(
            &self.exec,
            &self.policies,
            &mut self.app_reported,
        ));
        // Persist the ledger every tick so a restart resumes today's usage
        // instead of granting a fresh budget (best-effort; skipped in dry-run).
        if !self.exec.dry_run() {
            self.tracker.save();
        }
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

            // An active grace window (a code typed at the lock) suspends
            // enforcement for this user — including a whole-device admin lock
            // (the parent always wins).
            let in_grace = self
                .unlock_until
                .get(&user)
                .is_some_and(|t| *t > Instant::now());
            if !in_grace {
                self.unlock_until.remove(&user);
            }

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
                screentime::evaluate(&policy, &self.tracker, &user)
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

            // A whole-device lock (admin command with its save-your-work window
            // closed, the offline hard-lockdown, a confirmed evasion attempt).
            let effective_device_locked = self.device_lock_effective() && !in_grace;
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
                    let body = match self.tracker.remaining_minutes(&user, &policy) {
                        Some(m) if m > 0 => format!("You have {m} minutes left today."),
                        _ => "Your screen time is back on.".to_string(),
                    };
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

        // Web sign-in decisions dropped by users' trays (client-first login).
        self.check_login_decisions().await;

        // The lock follows the freeze set: up in front of whoever is stopped
        // and on screen, down (after the thaw above) once they are not.
        self.reconcile_lock().await;
        self.prev_active = Some(active.iter().cloned().collect());

        // Persist the freeze/grant state every tick, like the usage ledger
        // above — a power-cycle at any moment must resume, not reset.
        if !self.exec.dry_run() {
            self.persist_freeze_state();
        }
        self.write_status_file();
        events
    }

    /// Collect the sign-in decisions users' trays dropped in their own
    /// `/run/user/<uid>/openscreentime` (the same spoof-proof channel as the
    /// earn marker: only that user and root can write there), report them to
    /// the server, and expire prompts nobody answered.
    async fn check_login_decisions(&mut self) {
        if self.pending_logins.is_empty() {
            return;
        }
        let now = chrono::Utc::now();
        let pending = self.pending_logins.clone();
        let mut done: Vec<String> = Vec::new();
        for p in &pending {
            if p.expires < now {
                done.push(p.id.clone());
                continue;
            }
            // The request id is interpolated into a filesystem path, so it must
            // be an opaque token — never a traversal or a weird name.
            if !p
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            {
                done.push(p.id.clone());
                continue;
            }
            for u in &p.os_users {
                let Some(uid) = crate::sysusers::uid_of(u) else {
                    continue;
                };
                // The verdict lives in memory once read. If we already have it
                // (a prior tick read the file but the POST failed), retry the
                // POST — we do NOT touch the child-owned file again.
                let tapped = match &p.decision {
                    Some(d) => d.clone(),
                    None => {
                        let path = std::path::PathBuf::from(format!(
                            "/run/user/{uid}/openscreentime/login_decision_{}",
                            p.id
                        ));
                        // O_NOFOLLOW: the runtime dir is owned by the child, so
                        // a symlink there must never be followed by this root
                        // read. A symlink or missing file simply means "no
                        // answer yet". The file is deleted (also O_NOFOLLOW via
                        // remove_file, which does not follow the final
                        // component) the moment we have the verdict.
                        use std::os::unix::fs::OpenOptionsExt;
                        let Ok(f) = std::fs::OpenOptions::new()
                            .read(true)
                            .custom_flags(libc::O_NOFOLLOW)
                            .open(&path)
                        else {
                            continue;
                        };
                        use std::io::Read;
                        let mut raw = String::new();
                        if f.take(64).read_to_string(&mut raw).is_err() {
                            continue;
                        }
                        let _ = std::fs::remove_file(&path);
                        // The tapped number, or "deny"/empty for "not me". Only
                        // digits are kept; the server matches it to the real code.
                        let d: String = raw.trim().chars().filter(|c| c.is_ascii_digit()).collect();
                        // Record in memory so a POST failure retries from here.
                        if let Some(m) = self.pending_logins.iter_mut().find(|x| x.id == p.id) {
                            m.decision = Some(d.clone());
                        }
                        d
                    }
                };
                // The agent never judges: it forwards the tapped code and the
                // server decides approve vs deny by matching it.
                let code_opt = if tapped.is_empty() {
                    None
                } else {
                    Some(tapped.as_str())
                };
                match self.client.post_login_decision(&p.id, code_opt, u).await {
                    Ok(()) => {
                        self.notify_user(
                            Some(u),
                            "Answered",
                            "Your answer was sent. If it was you, the web session opens.",
                            false,
                        );
                        done.push(p.id.clone());
                    }
                    Err(e) => {
                        // Transient: the verdict is safe in memory; next tick
                        // re-POSTs it. Nothing is written to disk.
                        tracing::warn!("login decision for {} failed, will retry: {e}", p.id);
                    }
                }
                break;
            }
        }
        if !done.is_empty() {
            self.pending_logins.retain(|p| !done.contains(&p.id));
            self.write_status_file();
        }
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

    /// The next stop per user — published in the status snapshot for the
    /// companion's 15/5/1-minute warnings, and written to the terminals of
    /// anyone here with no desktop (their only way to hear it).
    fn update_forecasts(&mut self, active: &[String]) {
        let now = chrono::Local::now();
        let accel = i64::from(self.ctx.time_accel.max(1));
        let pause = self
            .device_lock_grace_until
            .filter(|t| self.device_locked && *t > Instant::now())
            .map(|t| warn::Forecast {
                reason: warn::StopReason::Pause,
                at: now
                    + chrono::Duration::from_std(t.saturating_duration_since(Instant::now()))
                        .unwrap_or_default(),
            });
        let until = |t: Instant| {
            now + chrono::Duration::from_std(t.saturating_duration_since(Instant::now()))
                .unwrap_or_default()
        };
        let mut next: HashMap<String, warn::Forecast> = HashMap::new();
        for (u, p) in &self.policies {
            if self.frozen.contains(u) {
                continue;
            }
            let rule = screentime::evaluate(p, &self.tracker, u).map(|r| stop_reason_of(&r));
            let f = if let Some(deadline) = self.pending_freeze.get(u) {
                // The save-your-work countdown of a stop that already tripped.
                Some(warn::Forecast {
                    reason: rule.unwrap_or(warn::StopReason::Limit),
                    at: until(*deadline),
                })
            } else if let (Some(t), Some(reason)) = (
                self.unlock_until.get(u).filter(|t| **t > Instant::now()),
                rule,
            ) {
                // A code bought time; the stop comes back when it runs out.
                Some(warn::Forecast {
                    reason,
                    at: until(*t),
                })
            } else {
                let limit_left = self
                    .tracker
                    .remaining_minutes(u, p)
                    .map(|m| chrono::Duration::seconds(m * 60 / accel));
                warn::forecast(p, limit_left, now)
            };
            if let Some(f) = [f, pause].into_iter().flatten().min_by_key(|f| f.at) {
                next.insert(u.clone(), f);
            }
        }
        for (u, f) in &next {
            if (f.at - now).num_seconds() <= 90 {
                self.announced.insert(u.clone(), Instant::now());
            }
        }
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
                let st = self.tty_warn.entry(u.clone()).or_default();
                match next.get(&u) {
                    Some(f) => {
                        let secs = (f.at - now).num_seconds();
                        if st.observe(f.reason, secs).is_some() {
                            let w = warn::words(f.reason, secs, Some(f.at));
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

    /// Everything the lock says to `user` right now.
    fn face_for(&self, user: &str) -> Face {
        use lock::{AskState, CodeState, Look};
        let policy = self.policies.get(user).cloned().unwrap_or_default();
        let verifier = self
            .parent_keys(&policy)
            .verifier()
            .with_state_path(self.parent_state.clone());
        let code = lock::code_state(&verifier);
        let now = chrono::Local::now();
        let (look, title, detail, can_ask) = if self.tamper_lockdown {
            (
                Look::Wall,
                "Stopped until a parent checks this computer".to_string(),
                "OpenScreenTime was changed without a parent's code.".to_string(),
                false,
            )
        } else if self.offline_hard_lockdown {
            (
                Look::Wall,
                "Stopped until this computer reaches the family server".to_string(),
                "It hasn't been in touch for days.".to_string(),
                false,
            )
        } else if self.device_locked {
            (
                Look::Paused,
                "Paused by a parent".to_string(),
                String::new(),
                false,
            )
        } else {
            match screentime::evaluate(&policy, &self.tracker, user) {
                Some(screentime::LockReason::DailyLimit {
                    used_min,
                    limit_min,
                }) => (
                    Look::Wall,
                    "Time's up for today".to_string(),
                    if used_min > limit_min {
                        format!("All {limit_min} minutes are used.")
                    } else {
                        format!("{used_min} of {limit_min} minutes used.")
                    },
                    true,
                ),
                Some(screentime::LockReason::Bedtime) => (
                    Look::Night,
                    match &policy.screen_time.bedtime {
                        Some(bt) => format!("Bedtime until {}", bt.end.trim()),
                        None => "Bedtime".to_string(),
                    },
                    "Screens are off until morning.".to_string(),
                    true,
                ),
                Some(screentime::LockReason::OutsideWindow) => (
                    Look::Night,
                    match warn::next_allowed(&policy.screen_time.schedule, now) {
                        Some(t) => format!("Outside allowed hours until {t}"),
                        None => "Outside allowed hours".to_string(),
                    },
                    "Screens are off at this time of day.".to_string(),
                    true,
                ),
                None => (
                    Look::Wall,
                    "This computer is stopped for now".to_string(),
                    String::new(),
                    true,
                ),
            }
        };
        let today = now.date_naive();
        let asked = self
            .requested_earn
            .iter()
            .any(|((u, _), d)| u == user && *d == today);
        let ask = match (can_ask, asked) {
            (false, _) => AskState::Hidden,
            (true, true) => AskState::Sent,
            (true, false) => AskState::Ready,
        };
        let help = if code == CodeState::Unavailable {
            lock::HELP_NO_CODE
        } else {
            lock::HELP
        };
        Face {
            look,
            title,
            detail,
            who: user.to_string(),
            code,
            ask,
            help: help.to_string(),
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
                _ => "That code didn't work.".to_string(),
            });
        }
        let minutes = lock::UNLOCK_MINUTES;
        self.unlock_until.insert(
            user.to_string(),
            Instant::now() + Duration::from_secs(u64::from(minutes) * 60),
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
            let in_grace = self
                .unlock_until
                .get(&u)
                .is_some_and(|t| *t > Instant::now());
            if self.policies.contains_key(&u)
                && !self.frozen.contains(&u)
                && !self.pending_freeze.contains_key(&u)
                && !in_grace
            {
                let policy = self.policies.get(&u).cloned().unwrap_or_default();
                if self.device_lock_effective() {
                    self.stop_user(&u, true).await;
                } else if let Some(reason) = screentime::evaluate(&policy, &self.tracker, &u) {
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

    /// The earn offer a "more time" ask files: the first configured task, or
    /// a plain ask.
    fn earn_offer_for(&self, user: &str) -> earn::EarnOffer {
        let policy = self.policies.get(user).cloned().unwrap_or_default();
        earn::earn_offers(&policy.gamification)
            .into_iter()
            .next()
            .unwrap_or_else(|| earn::EarnOffer {
                id: "more_time".into(),
                label: "More screen time".into(),
                reward_minutes: 15,
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
            // A parent's pause with a save-your-work window: when it lands.
            "pause_at": self.device_lock_grace_until
                .filter(|t| self.device_locked && *t > Instant::now())
                .map(|t| (chrono::Local::now()
                    + chrono::Duration::from_std(t.saturating_duration_since(Instant::now()))
                        .unwrap_or_default())
                    .to_rfc3339()),
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
            view["users"] = json!([{
                "name": u,
                "used_minutes": self.tracker.used_minutes(u),
                "remaining_minutes": self.tracker.remaining_minutes(u, p),
                "frozen": self.frozen.contains(u),
                "freeze_in_secs": self.pending_freeze.get(u).map(|d|
                    d.saturating_duration_since(Instant::now()).as_secs()),
                // The next stop, for the companion's 15/5/1-minute warnings.
                "stop_at": self.forecasts.get(u).map(|f| f.at.to_rfc3339()),
                "reason": self.forecasts.get(u).map(|f| f.reason.as_str()),
                "minutes_left": self.forecasts.get(u).map(|f|
                    ((f.at - chrono::Local::now()).num_seconds().max(0) + 59) / 60),
            }]);
            view["notifications"] = json!(notifs);
            // Sign-in prompts addressed to this user's sessions — the tray
            // renders these as actionable notifications and answers via a
            // decision file (client-first login, CONTRACT-0.6).
            view["login_requests"] = json!(self
                .pending_logins
                .iter()
                .filter(|l| l.os_users.iter().any(|x| x == u))
                .map(|l| json!({
                    "id": l.id,
                    "username": l.username,
                    "codes": l.codes,
                    "expires_at": l.expires.to_rfc3339(),
                }))
                .collect::<Vec<_>>());
            write_private_status(dir, u, uid, &view.to_string());
        }
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
                // Resume lifts the pause. Someone whose own rules still stop
                // them (time's up, bedtime) stays stopped — thawing them only
                // for the next tick to freeze them again would flash their
                // desktop and tell them "you're back" when they aren't. The
                // lock just changes its words.
                for user in self.frozen.clone() {
                    let policy = self.policies.get(&user).cloned().unwrap_or_default();
                    let in_grace = self
                        .unlock_until
                        .get(&user)
                        .is_some_and(|t| *t > Instant::now());
                    if !in_grace && screentime::evaluate(&policy, &self.tracker, &user).is_some() {
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
                // An unlock also disarms carried-over countdowns — and must
                // hit disk immediately, or a power-cut right after would boot
                // back into the lock the parent just lifted.
                self.pending_freeze.clear();
                if !self.exec.dry_run() {
                    self.persist_freeze_state();
                }
                events.push(Event::new(
                    EV_UNLOCK,
                    SEV_INFO,
                    json!({ "source": "command" }),
                ));
                json!({ "locked": false })
            }
            CMD_LOGIN_APPROVE => {
                let request_id = cmd
                    .payload
                    .get("request_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let username = cmd
                    .payload
                    .get("username")
                    .and_then(|v| v.as_str())
                    .unwrap_or("someone")
                    .to_string();
                let os_users: Vec<String> = cmd
                    .payload
                    .get("os_users")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let secs = cmd
                    .payload
                    .get("expires_in_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(120);
                let codes: Vec<String> = cmd
                    .payload
                    .get("codes")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if request_id.is_empty() || os_users.is_empty() || codes.is_empty() {
                    return (ack_failed(&cmd.id, "bad login_approve payload"), events);
                }
                for u in &os_users {
                    self.notify_user(
                        Some(u),
                        "Sign-in request",
                        &format!(
                            "{username} is signing in on the web. If it's you, tap the \
                             number shown in your browser — otherwise tap Not me."
                        ),
                        false,
                    );
                }
                self.pending_logins.retain(|p| p.id != request_id);
                self.pending_logins.push(PendingLogin {
                    id: request_id,
                    username,
                    os_users: os_users.clone(),
                    codes,
                    decision: None,
                    expires: chrono::Utc::now() + chrono::Duration::seconds(secs as i64),
                });
                // Snappy: the tray polls the snapshot every 5 s — publish now
                // rather than waiting for the next tick.
                self.write_status_file();
                json!({ "prompted": os_users })
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
                let level = cmd
                    .payload
                    .get("level")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1) as u8;
                let level = level.min(3);
                if level >= 3 && self.ctx.tamper_max < 3 {
                    tracing::warn!("server asked for level 3 but --tamper-max not set; capping at active ceiling");
                }
                self.tamper_level = level.min(if self.ctx.tamper_max >= 3 { 3 } else { level });
                if let Err(e) = tamper::install_polkit(&self.exec, self.tamper_level) {
                    return (ack_failed(&cmd.id, &e.to_string()), events);
                }
                if self.tamper_level >= 3 {
                    let _ = tamper::apply_level3_tty_lockdown(&self.exec);
                    events.push(tamper::level3_boot_guidance_event());
                }
                json!({ "tamper_level": self.tamper_level })
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
                self.tracker.add_earned(&os_username, minutes);
                if !self.exec.dry_run() {
                    self.tracker.save();
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

/// The warning vocabulary's name for a screen-time stop reason.
fn stop_reason_of(r: &screentime::LockReason) -> warn::StopReason {
    match r {
        screentime::LockReason::DailyLimit { .. } => warn::StopReason::Limit,
        screentime::LockReason::Bedtime => warn::StopReason::Bedtime,
        screentime::LockReason::OutsideWindow => warn::StopReason::Window,
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

fn ack_failed(id: &str, msg: &str) -> CommandAck {
    tracing::warn!("command {id} failed: {msg}");
    CommandAck {
        command_id: id.to_string(),
        status: "failed".into(),
        result: json!({ "error": msg }),
    }
}

/// Entry point for `run`.
pub async fn run(ctx: Arc<AgentCtx>, cfg: AgentConfig) -> Result<()> {
    ctx.require_root_for_enforcement()?;
    let mut agent = Agent::new(ctx.clone(), cfg)?;
    tracing::info!(
        dry_run = ctx.dry_run,
        is_root = ctx.is_root,
        tamper_level = agent.tamper_level,
        "openscreentime run loop starting"
    );

    let boot_events = agent.bootstrap().await.unwrap_or_default();
    agent.flush_events(boot_events).await;

    // The lock: whoever the kernel still has frozen is ours; the graphical
    // lock's socket; and a watch on the VT, so a switch or a login into a
    // stopped session meets the lock at once.
    agent.adopt_frozen();
    let mut lock_rx = agent
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

    // Reconnect with jittered exponential backoff (1 s → 60 s). A server that
    // answers HTTP but not WS keeps the backoff short: the poll round succeeded.
    let mut backoff_secs = BACKOFF_MIN_SECS;
    loop {
        match agent.client.connect_ws().await {
            Ok(stream) => {
                tracing::info!("WS bus connected");
                backoff_secs = BACKOFF_MIN_SECS;
                if let Err(e) = run_ws(&mut agent, stream, &mut lock_rx).await {
                    tracing::warn!("WS loop ended: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("WS unavailable ({e}); falling back to heartbeat polling");
                match run_poll(&mut agent, &mut lock_rx).await {
                    Ok(()) => backoff_secs = BACKOFF_MIN_SECS,
                    Err(e) => tracing::warn!("poll loop ended: {e}"),
                }
            }
        }
        let jitter = rand::Rng::gen_range(&mut rand::thread_rng(), 0..=backoff_secs / 2 + 1);
        // Waiting to reconnect must not leave a code typed at the lock unanswered.
        let wait = tokio::time::sleep(Duration::from_secs(backoff_secs + jitter));
        tokio::pin!(wait);
        loop {
            tokio::select! {
                _ = &mut wait => break,
                Some(ev) = lock_rx.recv() => agent.on_lock_event(ev).await,
            }
        }
        backoff_secs = (backoff_secs * 2).min(BACKOFF_MAX_SECS);
    }
}

/// WS-connected event loop: read server frames, run the enforcement tick, and
/// drain agent→server frames (events, acks) through a writer task.
async fn run_ws(
    agent: &mut Agent,
    stream: crate::client::WsStream,
    lock_rx: &mut mpsc::Receiver<LockEvent>,
) -> Result<()> {
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
    // accumulated while disconnected.
    if let Some(frame) = agent.state_frame_due(true) {
        let _ = out_tx.send(frame).await;
    }
    let usage = agent.usage_snapshot();
    if !usage.is_empty() {
        let _ = out_tx.send(AgentFrame::Heartbeat { usage }).await;
    }
    let mut last_hb = Instant::now();

    let mut ticker = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                // Events go over HTTP (`flush_events`), not a WS frame: a frame
                // pushed into a dying socket's channel is gone, while the flush
                // buffer keeps undelivered batches and retries next tick — the
                // same guarantee in both WS and poll mode.
                let events = agent.enforcement_tick().await;
                agent.flush_events(events).await;
                // Honest state: on change, and at least every STATE_AT_LEAST.
                if let Some(frame) = agent.state_frame_due(false) {
                    let _ = out_tx.send(frame).await;
                }
                // The WS bus has no HTTP heartbeat, so push usage here every
                // WS_HEARTBEAT — otherwise screen_time_ledger only ever updates
                // in the degraded poll path.
                if last_hb.elapsed() >= WS_HEARTBEAT {
                    last_hb = Instant::now();
                    let usage = agent.usage_snapshot();
                    if !usage.is_empty() {
                        let _ = out_tx.send(AgentFrame::Heartbeat { usage }).await;
                    }
                }
            }
            Some(ev) = lock_rx.recv() => agent.on_lock_event(ev).await,
            msg = read.next() => {
                let Some(msg) = msg else { break; };
                let msg = msg?;
                // Any frame from the server (including a bare Ping) counts as contact.
                agent.record_contact();
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
    agent: &mut Agent,
    txt: &str,
    out_tx: &mpsc::Sender<AgentFrame>,
) -> Result<()> {
    let frame: ServerFrame = serde_json::from_str(txt)?;
    match frame {
        ServerFrame::Command { command } => {
            let (ack, events) = agent.handle_command(command).await;
            // A resume, unlock or grant takes the lock down right away.
            agent.reconcile_lock().await;
            agent.flush_events(events).await;
            let _ = out_tx.send(AgentFrame::Ack { ack }).await;
        }
        ServerFrame::Ping => {
            let _ = out_tx.send(AgentFrame::Pong).await;
        }
    }
    Ok(())
}

/// Heartbeat polling fallback (no WS); commands flow via the heartbeat
/// command queue. Runs one `POLL_ROUND`, then returns `Ok` so the caller
/// retries the WS bus; returns `Err` as soon as a heartbeat fails.
async fn run_poll(agent: &mut Agent, lock_rx: &mut mpsc::Receiver<LockEvent>) -> Result<()> {
    let interval = Duration::from_secs(agent.cfg.poll_interval_secs.clamp(5, 30));
    let mut ticker = tokio::time::interval(TICK);
    let mut hb = tokio::time::interval(interval);
    let round_end = Instant::now() + POLL_ROUND;
    loop {
        if Instant::now() >= round_end {
            return Ok(());
        }
        tokio::select! {
            _ = ticker.tick() => {
                let events = agent.enforcement_tick().await;
                agent.flush_events(events).await;
            }
            Some(ev) = lock_rx.recv() => agent.on_lock_event(ev).await,
            _ = hb.tick() => {
                let users = crate::sysusers::login_users();
                let usage = agent.usage_snapshot();
                match agent.client.heartbeat("online", None, &users, &usage).await {
                    Ok(resp) => {
                        agent.record_contact();
                        for cmd in resp.commands {
                            let (ack, events) = agent.handle_command(cmd).await;
                            agent.reconcile_lock().await;
                            agent.flush_events(events).await;
                            let _ = agent.client.ack_command(&ack).await;
                        }
                        // Poll mode has no push channel: a changed policy_version
                        // is the signal to re-pull and re-apply.
                        if resp.policy_version != agent.policy_version {
                            match agent.client.get_policy().await {
                                Ok(bundle) => match agent.apply_bundle(bundle) {
                                    Ok(evs) => {
                                        agent.flush_events(evs).await;
                                    }
                                    Err(e) => tracing::warn!("policy re-apply failed: {e}"),
                                },
                                Err(e) => tracing::warn!("policy re-pull failed: {e}"),
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("heartbeat failed ({e}); will retry");
                        return Err(e); // bubble up to reconnect/backoff, retries WS
                    }
                }
            }
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
        screentime::evaluate(&a.policies["mia"], &a.tracker, "mia").unwrap()
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
        assert!(a.unlock_until.contains_key("mia"));
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
        assert_eq!(f.reason, warn::StopReason::Limit);
        assert!((f.at - chrono::Local::now()).num_seconds() > 0);
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
    async fn resume_keeps_someone_whose_own_rules_still_stop_them() {
        let (mut a, fake) = agent_with_mia(); // over her 60 minutes
        let cmd = |t: &str| Command {
            id: "c1".into(),
            cmd_type: t.into(),
            payload: json!({}),
        };
        let _ = a.handle_command(cmd(CMD_LOCK)).await;
        assert_eq!(a.face_for("mia").title, "Paused by a parent");
        let _ = a.handle_command(cmd(CMD_UNLOCK)).await;
        a.reconcile_lock().await;
        // Still stopped by her limit: no thaw, no "You're back"; the lock
        // just says why now.
        assert!(a.frozen.contains("mia"));
        assert_eq!(a.lock.subject(), Some("mia"));
        assert_eq!(a.face_for("mia").title, "Time's up for today");
        assert!(!fake.w().log.contains(&"thaw mia".to_string()));
    }

    #[tokio::test]
    async fn resume_from_the_console_takes_the_lock_down() {
        let (mut a, fake) = agent_with_mia();
        a.tracker.add_earned("mia", 30); // within her rules
        a.prev_active = Some(HashSet::new());
        let cmd = |t: &str| Command {
            id: "c1".into(),
            cmd_type: t.into(),
            payload: json!({}),
        };
        let (_ack, _ev) = a.handle_command(cmd(CMD_LOCK)).await;
        assert!(a.frozen.contains("mia"));
        assert_eq!(a.face_for("mia").title, "Paused by a parent");
        let (_ack, _ev) = a.handle_command(cmd(CMD_UNLOCK)).await;
        a.reconcile_lock().await;
        assert!(a.lock.shown().is_none());
        let log = fake.w().log.clone();
        assert!(pos(&log, "thaw mia") < pos(&log, "switch 2"));
    }
}
