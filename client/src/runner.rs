//! The `run` subcommand: connect the WS bus (falling back to heartbeat polling),
//! pull per-user policy, apply enforcement continuously, dispatch commands, and
//! stream events. This is the orchestrator that ties every module together.

use crate::client::ServerClient;
use crate::config::{AgentConfig, AgentCtx};
use crate::enforce::{self, screentime};
use crate::lockout::{self, LockSpec};
use crate::policy::Policy;
use crate::protocol::*;
use crate::util::Exec;
use crate::{earn, tamper};
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// How often the enforcement tick runs (screen-time accounting granularity).
const TICK: Duration = Duration::from_secs(10);

/// Save-your-work countdown between "the lock decision fired" and the actual
/// cgroup freeze. A freeze with zero warning looks exactly like a kernel hang
/// and can eat unsaved work — never again. Admin locks stay immediate.
const FREEZE_GRACE: Duration = Duration::from_secs(60);

/// Minutes granted when a parent PIN arrives via the headless file-drop
/// override (`/run/openscreentime/unlock_pin.<user>`), matching the GUI's PIN grant.
const PIN_OVERRIDE_GRANT_MIN: u32 = 30;
/// Max self-serve challenge (math) unlock grants honored per user per day, so
/// the trivial challenge can't be re-solved indefinitely to defeat screen time.
const CHALLENGE_GRANTS_PER_DAY: u32 = 3;

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

/// Where the rest of the reboot-surviving enforcement state lives. The freeze
/// set, the save-your-work countdowns and the daily challenge-unlock counter
/// used to be memory-only, so holding the power button was a complete reset:
/// a fresh 60-second grace and three more math unlocks per boot, repeatable
/// all night using nothing but features built for the child. `device_locked`
/// was persisted for exactly this reason; these were missed.
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
    /// user → (date, count) of self-serve challenge unlocks already honored.
    #[serde(default)]
    challenge_grants: HashMap<String, (chrono::NaiveDate, u32)>,
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
    /// so the headless auto-request doesn't spam the server more than once a day
    /// (CONTRACT-PROD.md §4 — the server also dedupes, this just avoids the noise).
    requested_earn: HashMap<(String, String), chrono::NaiveDate>,
    /// (os_username) → (date, count) of self-serve challenge unlock grants
    /// honored today, capped at [`CHALLENGE_GRANTS_PER_DAY`].
    challenge_grants: HashMap<String, (chrono::NaiveDate, u32)>,
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
    /// Confirmation gate that separates a real, sustained evasion attempt from a
    /// transient blip before escalating to `tamper_lockdown`.
    tamper_monitor: tamper::TamperMonitor,
    /// Verified-unlock grace windows (user → expiry). Fed by overlay grants and
    /// the parent-PIN file override; while active, the user is treated as
    /// within policy (screen-time AND admin lock — the parent always wins).
    unlock_until: HashMap<String, Instant>,
    /// Pre-lockout warnings already shown today: (user, kind) → local date.
    warned: HashMap<(String, String), chrono::NaiveDate>,
    /// Armed save-your-work countdowns (user → freeze deadline).
    pending_freeze: HashMap<String, Instant>,
    /// Users whose freeze was carried over from before a restart (their
    /// [`Self::pending_freeze`] entry is pre-expired). The lockout overlay from
    /// the previous run died with it, so when the resumed freeze lands the
    /// overlay must be presented again — an unexplained frozen session is
    /// indistinguishable from a hang. Never populated during normal operation.
    resumed_frozen: HashSet<String>,
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

/// The per-user on-demand earn-request marker. The kid's tray drops a file here
/// (in its own `/run/user/<uid>`, which only that user and root can touch); the
/// root agent consumes it. Returns `None` if the username has no uid.
fn ondemand_earn_marker(user: &str) -> Option<std::path::PathBuf> {
    let uid = crate::sysusers::uid_of(user)?;
    Some(std::path::PathBuf::from(format!(
        "/run/user/{uid}/openscreentime/earn_request"
    )))
}

/// Increment a per-user daily counter (resetting it on a new day) and report
/// whether this use is within `cap`. Pure, so the challenge-grant cap is
/// unit-testable without constructing an `Agent`.
fn allow_daily(
    map: &mut HashMap<String, (chrono::NaiveDate, u32)>,
    user: &str,
    today: chrono::NaiveDate,
    cap: u32,
) -> bool {
    let entry = map.entry(user.to_string()).or_insert((today, 0));
    if entry.0 != today {
        *entry = (today, 0);
    }
    if entry.1 >= cap {
        return false;
    }
    entry.1 += 1;
    true
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
        let resumed_frozen: HashSet<String> = carried.frozen.iter().cloned().collect();
        // Pre-expired countdowns: the grace was already granted before the
        // restart. If the user is still outside policy on their first active
        // tick the freeze lands immediately; if they are back within policy
        // (a reboot the next morning) the entry is simply disarmed.
        let pending_freeze: HashMap<String, Instant> = carried
            .frozen
            .iter()
            .map(|u| (u.clone(), Instant::now()))
            .collect();
        Ok(Agent {
            tamper_level: cfg
                .tamper_level
                .max(if ctx.tamper_max >= 3 { 3 } else { 1 }),
            ctx,
            cfg,
            client,
            exec,
            policies: HashMap::new(),
            vpn: None,
            // Reboot-surviving: reload the day's usage so a restart can't reset it.
            tracker: screentime::UsageTracker::load(),
            frozen: HashSet::new(),
            // Reboot-surviving: a parent's lock must outlast a power-cycle.
            device_locked: load_device_locked(),
            policy_version: String::new(),
            expected_wall: None,
            requested_earn: HashMap::new(),
            challenge_grants: carried.challenge_grants,
            last_contact: Instant::now(),
            contact_state: ContactState::Online,
            offline_grace: offline_grace_from_env(),
            last_contact_wall: load_last_contact_wall(),
            last_contact_saved: Instant::now(),
            offline_hard_lockdown: false,
            tamper_lockdown: carried.tamper_lockdown,
            tamper_monitor: tamper::TamperMonitor::new(),
            unlock_until: HashMap::new(),
            warned: HashMap::new(),
            pending_freeze,
            resumed_frozen,
            pending_events,
            notifications: VecDeque::new(),
            notif_seq: 0,
        })
    }

    /// Publish a normal (non-blocking) desktop notification for the tray to
    /// deliver. `user = None` means device-wide. Also emits the headless
    /// `wall`/log fallback so a machine with no tray isn't left silent.
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
        lockout::notify(&self.exec, "notification", &format!("{title} — {body}"));
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

    /// The device-wide offline hard-lockdown threshold: the strictest (smallest
    /// non-zero) `lockdown.offline_lockdown_days` across all managed users.
    /// 0 = feature off.
    fn offline_lockdown_days(&self) -> u32 {
        self.policies
            .values()
            .map(|p| p.lockdown.offline_lockdown_days)
            .filter(|d| *d > 0)
            .min()
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
        let engaged =
            days > 0 && (chrono::Utc::now() - self.last_contact_wall).num_days() >= i64::from(days);
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
        for up in bundle.users {
            self.policies.insert(up.os_username, up.policy);
        }
        self.vpn = bundle.vpn;
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

    /// Merge all users' network policies into the tightest host-global ruleset:
    /// intersection of allowed ports, union of DNS allowlists only if every active
    /// policy allows the name — the skeleton takes the *first* user's policy or the
    /// default, and documents per-user network isolation as future work.
    fn effective_network_policy(&self) -> Policy {
        // Prefer a non-wildcard, screen-time-enabled (i.e. "managed") policy so the
        // host DNS/firewall reflect the strictest present. Fall back to default.
        self.policies
            .values()
            .min_by_key(|p| {
                let allow_all = p.dns.allows_everything();
                let ports = p.firewall.allow_outbound_ports.len();
                (allow_all as usize, ports)
            })
            .cloned()
            .unwrap_or_default()
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

        // Screen-time: account active seat users, evaluate, freeze/unfreeze.
        let active = screentime::active_seat_users(&self.exec);
        for user in &active {
            self.tracker
                .add_active(user, TICK.as_secs() as u32, self.ctx.time_accel);
        }
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

            // 1) Consume verified unlocks FIRST — every tick, every user,
            // frozen or not. (The old code only consulted the override on the
            // freeze-transition tick, so once a user was frozen a parent
            // standing at the machine could never get them out.) Two sources:
            //   * an overlay grant (GUI already verified PIN/challenge), and
            //   * the headless parent-PIN file drop (verified here).
            let granted: Option<(u32, &str)> =
                if let Some((mins, kind)) = lockout::take_unlock_grant(&user) {
                    // A self-serve challenge (math) grant is capped per day so it
                    // can't be re-solved indefinitely to defeat screen time; a
                    // parent-PIN grant is never capped.
                    if kind == "challenge"
                        && !allow_daily(
                            &mut self.challenge_grants,
                            &user,
                            chrono::Local::now().date_naive(),
                            CHALLENGE_GRANTS_PER_DAY,
                        )
                    {
                        tracing::info!("challenge unlock for {user} ignored — daily cap reached");
                        None
                    } else {
                        Some((mins, "lockout-screen unlock"))
                    }
                } else {
                    let spec = LockSpec::from_lockout(
                        &Default::default(),
                        "",
                        "",
                        &user,
                        policy.parent_pin_hash.clone(),
                    );
                    lockout::check_and_consume_pin_override(&self.exec, &spec)
                        .then_some((PIN_OVERRIDE_GRANT_MIN, "parent PIN"))
                };
            if let Some((mins, source)) = granted {
                self.unlock_until.insert(
                    user.clone(),
                    Instant::now() + Duration::from_secs(u64::from(mins) * 60),
                );
                // A parent standing at the machine with the PIN has handled the
                // situation — clear a confirmed-evasion lockdown so the device
                // isn't stuck locked after they've dealt with it.
                if self.tamper_lockdown {
                    self.tamper_lockdown = false;
                    tracing::info!("tamper lockdown cleared by parent PIN at the device");
                }
                self.pending_freeze.remove(&user);
                if currently_frozen {
                    if let Err(e) = screentime::freeze_user(&self.exec, &user, false, false) {
                        tracing::warn!("unfreeze {user} (verified unlock) failed: {e}");
                    }
                    self.frozen.remove(&user);
                }
                events.push(tamper::tamper_event(
                    "parent_pin_override",
                    SEV_INFO,
                    &format!("{user} was unlocked for {mins} min via {source}"),
                ));
                continue;
            }

            // 2) An active grace window suspends enforcement for this user —
            // including a whole-device admin lock (the parent always wins).
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
            let lock = if should_evaluate_screen_time(in_grace, is_active, currently_frozen) {
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
                self.resumed_frozen.remove(&user);
            }

            // 3) Pre-lockout warnings — the teen must never be surprised by a
            // freeze. Fires while still within policy.
            if is_active && !currently_frozen && !self.device_locked && lock.is_none() && !in_grace
            {
                self.maybe_warn(&user, &policy);
            }

            let effective_device_locked =
                (self.device_locked || self.offline_hard_lockdown || self.tamper_lockdown)
                    && !in_grace;
            match decide_freeze(effective_device_locked, lock.as_ref(), currently_frozen) {
                FreezeAction::Freeze => {
                    if effective_device_locked {
                        // A whole-device lock (admin command, the offline
                        // hard-lockdown escalation, or a confirmed evasion
                        // attempt) overrides screen-time and is immediate (and
                        // may hard-fall-back to session termination — it's an
                        // explicit parent action / tamper response).
                        let (headline, detail) = if self.device_locked {
                            ("LOCKED", "THIS DEVICE IS LOCKED BY AN ADMIN")
                        } else if self.tamper_lockdown {
                            (
                                "TAMPERING DETECTED",
                                "OPENSCREENTIME WAS TAMPERED WITH — ASK A PARENT (PIN UNLOCKS)",
                            )
                        } else {
                            (
                                "OFFLINE TOO LONG",
                                "NO SERVER CONTACT FOR DAYS — ASK A PARENT (PIN UNLOCKS)",
                            )
                        };
                        let spec = LockSpec::from_lockout(
                            &Default::default(),
                            headline,
                            detail,
                            &user,
                            policy.parent_pin_hash.clone(),
                        );
                        lockout::present(&self.exec, &spec);
                        if let Err(e) = screentime::freeze_user(&self.exec, &user, true, true) {
                            tracing::warn!("freeze {user} failed: {e}");
                        }
                        self.frozen.insert(user.clone());
                    } else if let Some(reason) = &lock {
                        self.screen_time_lockout(&user, &policy, reason, &mut events)
                            .await;
                    }
                }
                FreezeAction::Unfreeze => {
                    // Policy now allows (and no admin lock is active): unfreeze.
                    if let Err(e) = screentime::freeze_user(&self.exec, &user, false, false) {
                        tracing::warn!("unfreeze {user} failed: {e}");
                    }
                    self.frozen.remove(&user);
                    tracing::info!("{user} unlocked (within policy again)");
                }
                FreezeAction::None => {}
            }
        }

        // On-demand "request more time" markers dropped by users' trays.
        self.check_ondemand_earn().await;

        // Persist the freeze/grant state every tick, like the usage ledger
        // above — a power-cycle at any moment must resume, not reset.
        if !self.exec.dry_run() {
            self.persist_freeze_state();
        }
        self.write_status_file();
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
            challenge_grants: self.challenge_grants.clone(),
            tamper_lockdown: self.tamper_lockdown,
            saved_at: Some(chrono::Utc::now()),
        });
    }

    /// Screen-time lockout with a save-your-work grace: the first tick with a
    /// lock reason presents the overlay (earn offer, nudges, event) and arms a
    /// `FREEZE_GRACE` countdown; the freeze itself only lands once the
    /// countdown expires. Never terminates the session (soft freeze only).
    async fn screen_time_lockout(
        &mut self,
        user: &str,
        policy: &Policy,
        reason: &screentime::LockReason,
        events: &mut Vec<Event>,
    ) {
        match self.pending_freeze.get(user) {
            None => {
                // Arm the countdown + present everything ONCE.
                let mut spec = LockSpec::from_lockout(
                    &policy.gamification.lockout,
                    &reason.headline(),
                    &reason.detail(),
                    user,
                    policy.parent_pin_hash.clone(),
                );
                // Offer an earn-time task as the primary action when the user
                // ran out of daily minutes (Duolingo-style: earn your way
                // back). Headless build has no interactive task picker, so the
                // first offer is auto-requested and the copy reflects that
                // it's already in flight.
                if matches!(reason, screentime::LockReason::DailyLimit { .. }) {
                    if let Some(offer) = earn::earn_offers(&policy.gamification).into_iter().next()
                    {
                        spec.action =
                            self.auto_request_earn(user, &offer)
                                .await
                                .unwrap_or_else(|| {
                                    format!("Earn {} min — {}", offer.reward_minutes, offer.label)
                                });
                    }
                }
                // The full-screen overlay now shows a live save-your-work
                // countdown itself (no more static "PAUSES IN 60 SECONDS" text).
                spec.countdown_secs = Some(FREEZE_GRACE.as_secs() as u32);
                lockout::present(&self.exec, &spec);
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
                            "freeze_grace_secs": FREEZE_GRACE.as_secs(),
                        }),
                    )
                    .for_user(user),
                );
                self.pending_freeze
                    .insert(user.to_string(), Instant::now() + FREEZE_GRACE);
            }
            Some(deadline) if *deadline <= Instant::now() => {
                self.pending_freeze.remove(user);
                // A freeze resuming from before a restart has no overlay on
                // screen (the presenter died with the previous run) — put it
                // back up, without a countdown, so the frozen session explains
                // itself. Normal freezes were presented when the countdown was
                // armed and must NOT be presented again (the GUI presenter is a
                // detached subprocess; re-presenting would stack a second one).
                if self.resumed_frozen.remove(user) {
                    let spec = LockSpec::from_lockout(
                        &policy.gamification.lockout,
                        &reason.headline(),
                        &reason.detail(),
                        user,
                        policy.parent_pin_hash.clone(),
                    );
                    lockout::present(&self.exec, &spec);
                }
                if let Err(e) = screentime::freeze_user(&self.exec, user, true, false) {
                    tracing::warn!("freeze {user} failed: {e}");
                }
                self.frozen.insert(user.to_string());
            }
            Some(_) => {} // countdown still running
        }
    }

    /// Pre-lockout wind-down: 10-minute and 2-minute warnings plus a bedtime
    /// heads-up 15 minutes out, each at most once per user per day.
    ///
    /// These deliberately emit no server event. Telling a parent "we warned
    /// them at 10 minutes" is feed noise; the moment that actually matters —
    /// the stop itself — already emits `screen_time_exceeded`.
    fn maybe_warn(&mut self, user: &str, policy: &Policy) {
        let today = chrono::Local::now().date_naive();
        let fire = |warned: &mut HashMap<(String, String), chrono::NaiveDate>,
                    exec: &Exec,
                    kind: &str,
                    copy: String| {
            let key = (user.to_string(), kind.to_string());
            if warned.get(&key) == Some(&today) {
                return;
            }
            warned.insert(key, today);
            lockout::notify(exec, kind, &copy);
        };

        if let Some(rem) = self.tracker.remaining_minutes(user, policy) {
            // Check the tighter threshold first so a user who logs in with
            // 2 minutes left gets the urgent copy, not the relaxed one.
            let warn = if rem > 0 && rem <= 2 {
                Some((
                    "time_2min",
                    format!("{rem} min left — wrap up and save your work now."),
                ))
            } else if rem > 2 && rem <= 10 {
                Some((
                    "time_10min",
                    format!("{rem} min left today — a good time to finish up."),
                ))
            } else {
                None
            };
            if let Some((kind, copy)) = warn {
                fire(&mut self.warned, &self.exec, kind, copy);
            }
        }

        if let Some(bt) = &policy.screen_time.bedtime {
            if let Some(mins) = screentime::minutes_until_bedtime(bt, chrono::Local::now().time()) {
                if (1..=15).contains(&mins) {
                    fire(
                        &mut self.warned,
                        &self.exec,
                        "bedtime_soon",
                        format!("Bedtime in {mins} min — time to wind down."),
                    );
                }
            }
        }
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
            }]);
            view["notifications"] = json!(notifs);
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
            if !path.exists() {
                continue;
            }
            let _ = std::fs::remove_file(&path); // single-use
            let policy = self.policies.get(&user).cloned().unwrap_or_default();
            // Use the first configured earn offer, or a plain "more time" ask.
            let offer = earn::earn_offers(&policy.gamification)
                .into_iter()
                .next()
                .unwrap_or_else(|| earn::EarnOffer {
                    id: "more_time".into(),
                    label: "More screen time".into(),
                    reward_minutes: 15,
                });
            if let Some(copy) = self.auto_request_earn(&user, &offer).await {
                self.notify_user(Some(&user), "Request sent", &copy, false);
            }
        }
    }

    /// Auto-request an earn-time offer once per (user, task) per day (the server
    /// also dedupes by returning the existing pending row, but we avoid spamming
    /// it every tick). Returns the presenter copy to show, if a request was sent
    /// or already pending today.
    async fn auto_request_earn(&mut self, user: &str, offer: &earn::EarnOffer) -> Option<String> {
        let today = chrono::Local::now().date_naive();
        let key = (user.to_string(), offer.id.clone());
        if self.requested_earn.get(&key) == Some(&today) {
            return Some("Request sent — waiting for approval.".to_string());
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
                Some("REQUEST SENT — WAITING FOR APPROVAL".to_string())
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
                for user in self.policies.keys().cloned().collect::<Vec<_>>() {
                    let pin_hash = self
                        .policies
                        .get(&user)
                        .and_then(|p| p.parent_pin_hash.clone());
                    let spec = LockSpec::from_lockout(
                        &Default::default(),
                        "LOCKED",
                        "THIS DEVICE IS LOCKED BY AN ADMIN",
                        &user,
                        pin_hash,
                    );
                    lockout::present(&self.exec, &spec);
                    let _ = screentime::freeze_user(&self.exec, &user, true, true);
                    self.frozen.insert(user);
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
                // An admin unlock also lifts a confirmed-evasion lockdown.
                self.tamper_lockdown = false;
                for user in self.frozen.drain().collect::<Vec<_>>() {
                    let _ = screentime::freeze_user(&self.exec, &user, false, false);
                }
                // An unlock also disarms carried-over countdowns — and must
                // hit disk immediately, or a power-cut right after would boot
                // back into the lock the parent just lifted.
                self.pending_freeze.clear();
                self.resumed_frozen.clear();
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
                // cache so a later same-day lockout sends a fresh request instead
                // of showing a stale "REQUEST SENT — WAITING FOR APPROVAL".
                self.requested_earn.retain(|(u, _), _| u != &os_username);
                // Tell the kid — an approval used to be silent to them.
                self.notify_user(
                    Some(&os_username),
                    "TIME GRANTED",
                    &format!("+{minutes} MIN — YOU'RE BACK"),
                    false,
                );
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
                    "REQUEST NOT APPROVED",
                    "MAYBE LATER — ASK A PARENT",
                    false,
                );
                json!({ "denied": true, "os_username": os_username, "task_id": task_id })
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
fn should_evaluate_screen_time(in_grace: bool, is_active: bool, currently_frozen: bool) -> bool {
    !in_grace && (is_active || currently_frozen)
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

    // Daily self-update (first check ~2 min in). No-op unless enabled and
    // running as the installed /usr/local/bin binary — see update.rs.
    tokio::spawn(crate::update::update_loop(
        agent.cfg.clone(),
        agent.client.clone(),
        agent.exec.clone(),
    ));

    loop {
        match agent.client.connect_ws().await {
            Ok(stream) => {
                tracing::info!("WS bus connected");
                if let Err(e) = run_ws(&mut agent, stream).await {
                    tracing::warn!("WS loop ended: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("WS unavailable ({e}); falling back to heartbeat polling");
                if let Err(e) = run_poll(&mut agent).await {
                    tracing::warn!("poll loop ended: {e}");
                }
            }
        }
        // Reconnect/backoff.
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// WS-connected event loop: read server frames, run the enforcement tick, and
/// drain agent→server frames (events, acks) through a writer task.
async fn run_ws(agent: &mut Agent, stream: crate::client::WsStream) -> Result<()> {
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
                // The WS bus has no HTTP heartbeat, so push usage here — otherwise
                // screen_time_ledger only ever updates in the degraded poll path.
                let usage = agent.usage_snapshot();
                if !usage.is_empty() {
                    let _ = out_tx.send(AgentFrame::Heartbeat { usage }).await;
                }
            }
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
/// command queue.
async fn run_poll(agent: &mut Agent) -> Result<()> {
    let interval = Duration::from_secs(agent.cfg.poll_interval_secs.max(5));
    let mut ticker = tokio::time::interval(TICK);
    let mut hb = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let events = agent.enforcement_tick().await;
                agent.flush_events(events).await;
            }
            _ = hb.tick() => {
                let users = crate::sysusers::login_users();
                let usage = agent.usage_snapshot();
                match agent.client.heartbeat("online", None, &users, &usage).await {
                    Ok(resp) => {
                        agent.record_contact();
                        for cmd in resp.commands {
                            let (ack, events) = agent.handle_command(cmd).await;
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
        assert!(should_evaluate_screen_time(false, false, true));
        // active -> evaluated as always
        assert!(should_evaluate_screen_time(false, true, false));
        // neither active nor frozen -> skipped, never newly frozen while away
        assert!(!should_evaluate_screen_time(false, false, false));
        // a parent-granted grace window suspends enforcement outright
        assert!(!should_evaluate_screen_time(true, true, true));
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
    fn challenge_grants_are_capped_per_day_then_reset() {
        let mut map = HashMap::new();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        // First CHALLENGE_GRANTS_PER_DAY are honored, the next is not.
        for _ in 0..CHALLENGE_GRANTS_PER_DAY {
            assert!(allow_daily(
                &mut map,
                "kid",
                today,
                CHALLENGE_GRANTS_PER_DAY
            ));
        }
        assert!(!allow_daily(
            &mut map,
            "kid",
            today,
            CHALLENGE_GRANTS_PER_DAY
        ));
        // A different user has an independent budget.
        assert!(allow_daily(
            &mut map,
            "sib",
            today,
            CHALLENGE_GRANTS_PER_DAY
        ));
        // A new day resets the counter.
        let tomorrow = today.succ_opt().unwrap();
        assert!(allow_daily(
            &mut map,
            "kid",
            tomorrow,
            CHALLENGE_GRANTS_PER_DAY
        ));
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
        let mut grants = HashMap::new();
        grants.insert(
            "vali".to_string(),
            (chrono::NaiveDate::from_ymd_opt(2026, 8, 5).unwrap(), 2u32),
        );
        let st = FreezeState {
            frozen: vec!["vali".to_string()],
            challenge_grants: grants,
            tamper_lockdown: true,
            saved_at: Some(chrono::Utc::now()),
        };
        let json = serde_json::to_string(&st).unwrap();
        let back: FreezeState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.frozen, vec!["vali".to_string()]);
        assert_eq!(back.challenge_grants.get("vali").map(|g| g.1), Some(2));
        assert!(back.tamper_lockdown);
        assert!(back.saved_at.is_some());

        let garbled: FreezeState = serde_json::from_str("{}").unwrap();
        assert!(garbled.frozen.is_empty());
        assert!(!garbled.tamper_lockdown);
    }
}
