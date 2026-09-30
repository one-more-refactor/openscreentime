//! `tray` subcommand — the per-user companion (feature `tray`).
//!
//! Runs AS THE DESKTOP USER (not root): it only reads the status snapshot the
//! root agent writes to `/run/openscreentime/` every tick, and talks to the
//! session bus (StatusNotifierItem via `ksni` where a host exists, desktop
//! notifications via `notify-rust` everywhere — GNOME included).
//!
//! It is the one channel for warnings: 15, 5 and 1 minute before any stop
//! (the daily limit, bedtime, the end of allowed hours, a parent's scheduled
//! pause), the last minute as one critical notification updated in place, so
//! the lock never arrives unannounced. It also delivers what the agent
//! publishes ("You're back — 15 more minutes"). Started on every desktop login
//! by an XDG autostart entry and by the systemd user unit; it keeps a single
//! instance. Everything else fires on state *transitions* only.
//!
//! Where a tray exists, its icon is the ring (brand/tray-*.svg), drawn with
//! the real share of the day used (`mark::tray_argb`). The first time it runs
//! for someone it opens the app window, which shows the first-run cards.

use crate::glance::Left;
use crate::parent;
use crate::warn::{self, StopReason, WarnState};
use anyhow::Result;
use serde::Deserialize;
use std::sync::mpsc;
use std::time::Duration;

/// Shared, device-wide snapshot (lock/connection/remote-shell). World-readable
/// but carries NO per-user activity — that lives in the per-user file below.
fn status_path() -> String {
    crate::paths::run_str("status.json")
}
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// How often parent mode polls the server for pending requests + alerts.
const PARENT_POLL: Duration = Duration::from_secs(15);

/// An approve/deny the parent triggered from the tray menu, handed to the
/// worker thread (which owns the HTTP client) to carry out.
enum ParentAction {
    Approve(String),
    Deny(String),
}

// ---------------------------------------------------------------------------
// Status snapshot (schema mirrors runner::write_status_file)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct Status {
    #[serde(default)]
    connection: String,
    #[serde(default)]
    device_locked: bool,
    #[serde(default)]
    offline_hard_lockdown: bool,
    #[serde(default)]
    tamper_lockdown: bool,
    #[serde(default)]
    users: Vec<UserStatus>,
    /// Normal (non-blocking) notifications published by the agent for the tray
    /// to deliver. Consumed by monotonic `id` so each shows exactly once.
    #[serde(default)]
    notifications: Vec<TrayNotification>,
    /// Sign-in / confirm codes for this user (logincode.rs): one desktop
    /// notification each, shared with the app window so it rings once.
    #[serde(default)]
    login_codes: Vec<crate::logincode::LoginCode>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct TrayNotification {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    urgency: String,
    /// Target user, or `None`/absent = device-wide.
    #[serde(default)]
    user: Option<String>,
    /// `"back"`: the welcome after a stop ("You're back — 15 more minutes").
    #[serde(default)]
    kind: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct UserStatus {
    name: String,
    #[serde(default)]
    used_minutes: u64,
    /// The verdict: time left, the next stop and why, a parent's override.
    #[serde(flatten)]
    clock: crate::glance::Clock,
    /// Countdown to an imminent session freeze, if one is pending.
    #[serde(default)]
    freeze_in_secs: Option<u64>,
    /// Sets their own limits: nobody to ask.
    #[serde(default)]
    self_managed: bool,
    /// May ask a parent for more (absent from older agents: yes).
    #[serde(default = "yes")]
    can_ask: bool,
}

fn yes() -> bool {
    true
}

impl Status {
    fn user<'a>(&'a self, name: &str) -> Option<&'a UserStatus> {
        self.users.iter().find(|u| u.name == name)
    }
}

impl UserStatus {
    /// Time left: the verdict, the same number the app window shows.
    fn left(&self) -> Left {
        self.clock.left(chrono::Local::now())
    }

    /// The share of today's time used (the ring), if something counts down.
    fn frac(&self) -> Option<f32> {
        let left = match self.left() {
            Left::Minutes { minutes, .. } => minutes,
            Left::Stopped => 0,
            Left::NoLimit => return None,
        };
        let total = self.used_minutes as f32 + left.max(0) as f32;
        Some(if total > 0.0 {
            (self.used_minutes as f32 / total).clamp(0.0, 1.0)
        } else {
            1.0
        })
    }
}

/// Read this user's status: the private per-user file if present (managed user),
/// otherwise the shared device-wide snapshot (non-managed user still sees
/// lock/connection/remote-shell state).
fn read_status(username: &str) -> Option<Status> {
    let per_user = crate::paths::run_str(&format!("status.{username}.json"));
    let raw = std::fs::read_to_string(&per_user)
        .or_else(|_| std::fs::read_to_string(status_path()))
        .ok()?;
    serde_json::from_str(&raw).ok()
}

/// When this user's status file was last written (the private one if there
/// is one, else the shared one).
fn status_written(username: &str) -> Option<std::time::SystemTime> {
    let per_user = crate::paths::run_str(&format!("status.{username}.json"));
    std::fs::metadata(&per_user)
        .or_else(|_| std::fs::metadata(status_path()))
        .and_then(|m| m.modified())
        .ok()
}

/// Wait until the agent writes this user's status again, or `at_most`. It
/// writes it the moment it has something to say ("You're back" as the lock
/// comes down), so a notification goes out within a fifth of a second of
/// it, not up to a whole poll later — and a companion thawed with the
/// person's apps doesn't sleep through the news of its own thaw.
fn wait_for_status(username: &str, at_most: Duration) {
    let since = status_written(username);
    let start = std::time::Instant::now();
    while start.elapsed() < at_most {
        std::thread::sleep(STATUS_WATCH);
        if status_written(username) != since {
            return;
        }
    }
}

/// How often the companion looks for a fresh status file between polls.
const STATUS_WATCH: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------------------
// Tray model
// ---------------------------------------------------------------------------

struct OpenScreenTimeTray {
    /// Desktop user we render for; matched against `status.users[]`.
    username: String,
    /// `None` when the status file is missing/unreadable (agent not running).
    status: Option<Status>,
    /// Parent-mode: pending time requests (kept current by the worker thread).
    /// Empty unless this machine is paired (`openscreentime pair`).
    pending: Vec<parent::api::PendingReq>,
    /// `Some` in parent mode — menu actions send approve/deny here for the
    /// worker to execute against the server.
    action_tx: Option<mpsc::Sender<ParentAction>>,
}

impl OpenScreenTimeTray {
    fn me(&self) -> Option<&UserStatus> {
        self.status.as_ref().and_then(|s| s.user(&self.username))
    }

    /// The headline for the current user (sentence case, docs/BRAND-CLIENT.md
    /// §4.1), or a device-level line when we are not a managed user.
    fn time_line(&self) -> String {
        let Some(s) = &self.status else {
            return "OpenScreenTime isn't running".to_string();
        };
        if s.device_locked {
            return "Paused by a parent".to_string();
        }
        match self.me().map(UserStatus::left) {
            Some(Left::Stopped) => "Time's up for today".to_string(),
            Some(Left::Minutes {
                unlocked_until: Some(t),
                ..
            }) => format!("Unlocked until {}", t.format("%H:%M")),
            Some(Left::Minutes { minutes: 1, .. }) => "1 minute left".to_string(),
            Some(Left::Minutes { minutes, .. }) => format!("{minutes} minutes left"),
            Some(Left::NoLimit) => "No limit today".to_string(),
            None => "This computer is managed".to_string(),
        }
    }

    fn connection_line(&self) -> &'static str {
        match self.status.as_ref().map(|s| s.connection.as_str()) {
            Some("online") => "Connected",
            Some(_) => "Offline — it catches up when it's back",
            None => "Not running",
        }
    }

    /// What the ring in the panel shows.
    fn tray_state(&self) -> crate::mark::TrayState {
        use crate::mark::TrayState;
        let Some(s) = &self.status else {
            return TrayState::Idle;
        };
        if s.device_locked {
            return TrayState::Paused;
        }
        if s.tamper_lockdown || s.offline_hard_lockdown {
            return TrayState::Stopped;
        }
        match self.me() {
            Some(u) => match (u.left(), u.frac()) {
                (Left::Stopped, _) => TrayState::Stopped,
                (Left::Minutes { minutes, .. }, Some(frac)) if minutes <= 15 => {
                    TrayState::Low { frac }
                }
                (Left::Minutes { .. }, Some(frac)) => TrayState::Ok { frac },
                _ => TrayState::Idle,
            },
            None => TrayState::Idle,
        }
    }
}

impl ksni::Tray for OpenScreenTimeTray {
    fn id(&self) -> String {
        "openscreentime".into()
    }

    fn title(&self) -> String {
        "OpenScreenTime".into()
    }

    fn icon_name(&self) -> String {
        // Empty: the host draws our pixmap (the ring, with today's share).
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let state = self.tray_state();
        [16, 22, 24, 32, 48]
            .into_iter()
            .map(|px| ksni::Icon {
                width: px as i32,
                height: px as i32,
                data: crate::mark::tray_argb(px, state),
            })
            .collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "OpenScreenTime".into(),
            description: format!("{} · {}", self.time_line(), self.connection_line()),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let mut items: Vec<ksni::MenuItem<Self>> = vec![
            StandardItem {
                label: self.time_line(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: self.connection_line().into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
        ];
        // Parent mode: pending time requests, each with its two answers.
        if self.action_tx.is_some() && !self.pending.is_empty() {
            items.push(MenuItem::Separator);
            let n = self.pending.len();
            items.push(
                StandardItem {
                    label: if n == 1 {
                        "1 time request".to_string()
                    } else {
                        format!("{n} time requests")
                    },
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
            for r in &self.pending {
                let approve_id = r.id.clone();
                let deny_id = r.id.clone();
                items.push(
                    SubMenu {
                        label: format!(
                            "{} · {} more minutes · {}",
                            r.who(),
                            r.minutes,
                            r.task_label
                        ),
                        submenu: vec![
                            StandardItem {
                                label: format!("Give {} more minutes", r.minutes),
                                activate: Box::new(move |t: &mut Self| {
                                    if let Some(tx) = &t.action_tx {
                                        let _ = tx.send(ParentAction::Approve(approve_id.clone()));
                                    }
                                }),
                                ..Default::default()
                            }
                            .into(),
                            StandardItem {
                                label: "Not now".into(),
                                activate: Box::new(move |t: &mut Self| {
                                    if let Some(tx) = &t.action_tx {
                                        let _ = tx.send(ParentAction::Deny(deny_id.clone()));
                                    }
                                }),
                                ..Default::default()
                            }
                            .into(),
                        ],
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        // The one verb, for someone who has a parent to ask.
        if self.me().is_some_and(|u| u.can_ask && !u.self_managed) {
            items.push(MenuItem::Separator);
            items.push(
                StandardItem {
                    label: "Ask for more time".into(),
                    activate: Box::new(|_: &mut Self| request_more_time()),
                    ..Default::default()
                }
                .into(),
            );
        }

        // The window says the rest (and the honest footer).
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Open OpenScreenTime".into(),
                activate: Box::new(|_: &mut Self| open_app()),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

/// Open the app window (it brings an open one forward instead).
fn open_app() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .arg("app")
            .stdin(std::process::Stdio::null())
            .spawn();
    }
}

// ---------------------------------------------------------------------------
// Notifications (transitions only)
// ---------------------------------------------------------------------------

/// Drop an on-demand "request more time" marker in this user's own runtime dir
/// for the root agent to pick up and turn into an earn-request. Writing here is
/// the only channel the unprivileged tray has to the root agent — and it's
/// spoof-proof, since `/run/user/<uid>` is the user's own 0700 directory.
fn request_more_time() {
    let uid = users::get_current_uid();
    let dir = std::path::PathBuf::from(format!("/run/user/{uid}/openscreentime"));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!("could not create runtime dir for earn request: {e}");
        notify("Couldn't send", "Try again in a moment.", false);
        return;
    }
    if let Err(e) = std::fs::write(dir.join("earn_request"), b"1") {
        tracing::debug!("could not write earn-request marker: {e}");
        notify("Couldn't send", "Try again in a moment.", false);
        return;
    }
    notify(
        "Asked for more time",
        "Waiting for a parent to answer.",
        false,
    );
}

fn notify(summary: &str, body: &str, critical: bool) {
    let mut n = notify_rust::Notification::new();
    n.appname("OpenScreenTime")
        .summary(summary)
        .body(body)
        .icon("openscreentime")
        .hint(notify_rust::Hint::DesktopEntry("openscreentime".into()));
    if critical {
        n.urgency(notify_rust::Urgency::Critical);
    }
    if let Err(e) = n.show() {
        tracing::debug!("notification failed: {e}");
    }
}

/// Diff two consecutive snapshots and fire notifications for the device-level
/// transitions we care about. Both sides must be present: on startup (or while
/// the agent is down) we stay silent instead of "catching up" on stale state.
/// Warnings before a stop are the [`Warner`]'s; "You're back" comes from the
/// agent (only it knows a thaw really happened).
fn notify_transitions(prev: &Status, next: &Status) {
    if prev.connection != next.connection {
        if next.connection == "offline_fail_closed" {
            notify(
                "Offline for a while",
                "It keeps today's rules and catches up when it's back.",
                false,
            );
        } else if next.connection == "online" && prev.connection == "offline_fail_closed" {
            notify("Back online", "Connected to home again.", false);
        }
    }
    match (prev.offline_hard_lockdown, next.offline_hard_lockdown) {
        (false, true) => notify(
            "Offline for too long",
            "This computer stops until it reaches home again. A parent's unlock code opens it.",
            true,
        ),
        (true, false) => notify("Back to normal", "This computer reached home again.", false),
        _ => {}
    }
    match (prev.tamper_lockdown, next.tamper_lockdown) {
        (false, true) => notify(
            "OpenScreenTime was changed",
            "Ask a parent — their unlock code opens it.",
            true,
        ),
        (true, false) => notify("Back to normal", "A parent checked this computer.", false),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Warnings before a stop
// ---------------------------------------------------------------------------

/// What's coming for this user, as the warnings see it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Upcoming {
    /// No stop ahead (or already stopped, or not managed here).
    Nothing,
    /// A daily-limit stop while nobody is using the screen: it slides later
    /// every tick (idle time isn't billed), so there is no true time to
    /// announce yet — keep what was said, say the rest once they're back.
    Hold,
    /// The stop, the seconds until it, and when it lands.
    Stop(StopReason, i64, chrono::DateTime<chrono::Local>),
}

/// The stop coming up for this user, from what the snapshot publishes: an
/// armed save-your-work countdown, else the rules' next stop (`stop_at` +
/// `reason`, a parent's scheduled pause included), else — from an agent that
/// predates those fields — the daily limit's minutes.
fn next_stop(status: &Status, username: &str, now: chrono::DateTime<chrono::Local>) -> Upcoming {
    let Some(me) = status.user(username) else {
        return Upcoming::Nothing;
    };
    if me.clock.frozen {
        return Upcoming::Nothing;
    }
    let reason = me
        .clock
        .reason
        .as_deref()
        .and_then(warn::parse_reason)
        .unwrap_or(StopReason::Limit);
    let at = if let Some(s) = me.freeze_in_secs {
        now + chrono::Duration::seconds(s as i64)
    } else if me.clock.stop_at.is_some() {
        match me.clock.stop_moment() {
            Some(at) => at,
            None => return Upcoming::Hold,
        }
    } else if me.clock.reason.is_none() {
        match me.clock.remaining_minutes.filter(|m| *m > 0) {
            Some(m) => now + chrono::Duration::minutes(m),
            None => return Upcoming::Nothing,
        }
    } else {
        return Upcoming::Nothing;
    };
    Upcoming::Stop(reason, (at - now).num_seconds(), at)
}

/// Shows the 15/5/1-minute warnings: the last minute as ONE critical
/// notification, kept current in place and closed once the stop is gone.
#[derive(Default)]
struct Warner {
    state: WarnState,
    last_minute: Option<notify_rust::NotificationHandle>,
    shown_title: String,
}

impl Warner {
    /// `frac`: the share of today's time used (the ring on the notice), if
    /// there is a limit; `can_ask`: whether "Ask for more time" is offered.
    fn observe(&mut self, stop: Upcoming, frac: Option<f32>, can_ask: bool) {
        let (reason, secs, at) = match stop {
            Upcoming::Hold => return,
            Upcoming::Stop(r, s, at) if s > 0 => (r, s, at),
            _ => {
                self.state.clear();
                if let Some(h) = self.last_minute.take() {
                    h.close();
                }
                return;
            }
        };
        let due = self.state.observe_stop(reason, secs, at.timestamp());
        let w = warn::words(reason, secs, Some(at));
        if let Some(h) = self.last_minute.as_mut() {
            let shown = format!("{}\n{}", w.title, w.body);
            if shown != self.shown_title {
                h.summary(&w.title).body(&w.body);
                let _ = h.update();
                self.shown_title = shown;
            }
            return;
        }
        if due.is_none() {
            return;
        }
        let ask = can_ask && reason != StopReason::Paused;
        match show_warning(&w, ask, frac) {
            Some(h) if w.critical => {
                self.shown_title = format!("{}\n{}", w.title, w.body);
                self.last_minute = Some(h);
            }
            Some(h) => {
                std::thread::spawn(move || h.wait_for_action(on_action));
            }
            None => {}
        }
    }
}

fn on_action(action: &str) {
    match action {
        "ask" => request_more_time(),
        "open" => open_app(),
        _ => {}
    }
}

/// Board 05b's amber ring, at the share of today used, as an image file the
/// notification server can load (SVG, in the person's own runtime dir).
fn warning_image(frac: Option<f32>) -> Option<String> {
    use crate::mark::{ring_svg, Ring, TRACK_CARD, WARN};
    let ring = match frac {
        Some(f) => Ring::Fill {
            frac: f.max(0.02),
            color: WARN,
        },
        None => Ring::Full { color: WARN },
    };
    let dir = std::path::PathBuf::from(format!(
        "/run/user/{}/openscreentime",
        users::get_current_uid()
    ));
    std::fs::create_dir_all(&dir).ok()?;
    // One file per percent: a server that caches by path still shows the new one.
    let pct = frac.map_or(100, |f| (f * 100.0).round() as u32);
    let path = dir.join(format!("warn-ring-{pct}.svg"));
    std::fs::write(&path, ring_svg(96.0, ring, TRACK_CARD)).ok()?;
    Some(path.to_string_lossy().into_owned())
}

fn show_warning(
    w: &warn::Words,
    ask: bool,
    frac: Option<f32>,
) -> Option<notify_rust::NotificationHandle> {
    let mut n = notify_rust::Notification::new();
    n.appname("OpenScreenTime")
        .summary(&w.title)
        .body(&w.body)
        .icon("openscreentime")
        .hint(notify_rust::Hint::DesktopEntry("openscreentime".into()));
    if let Some(img) = warning_image(frac) {
        n.hint(notify_rust::Hint::ImagePath(img));
    }
    if ask {
        n.action("ask", "Ask for more time");
    }
    n.action("open", "Open OpenScreenTime");
    if w.critical {
        n.urgency(notify_rust::Urgency::Critical)
            .timeout(notify_rust::Timeout::Never);
    }
    match n.show() {
        Ok(h) => {
            if w.critical {
                // Actions on the one we keep (to update and close) are heard
                // by id, from a thread of their own.
                let id = h.id();
                std::thread::spawn(move || {
                    let _ = notify_rust::handle_action(id, |r| {
                        if let notify_rust::ActionResponse::Custom(a) = r {
                            on_action(a);
                        }
                    });
                });
            }
            Some(h)
        }
        Err(e) => {
            tracing::debug!("warning notification failed: {e}");
            None
        }
    }
}

/// One companion per person: the autostart entry and the user unit may both
/// start one. Held for the life of the process.
fn single_instance() -> Option<std::fs::File> {
    use std::os::fd::AsRawFd;
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", users::get_current_uid()));
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(format!("{dir}/openscreentime-companion.lock"))
        .ok()?;
    // SAFETY: flock on an fd we own.
    let held = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    held.then_some(f)
}

/// Pure selection: the notifications this user hasn't seen yet (id above the
/// high-water mark, targeted at them or device-wide), plus the new high-water
/// mark. Split out from delivery so the logic is testable without a session bus.
fn select_notifications<'a>(
    username: &str,
    notifs: &'a [TrayNotification],
    last_id: u64,
) -> (Vec<&'a TrayNotification>, u64) {
    let mut high = last_id;
    let mut show = Vec::new();
    for n in notifs {
        high = high.max(n.id);
        let for_me = n.user.as_deref().is_none_or(|u| u == username);
        if n.id > last_id && for_me {
            show.push(n);
        }
    }
    (show, high)
}

/// Deliver any agent-published notifications this user hasn't seen yet and
/// return the new high-water mark. On the very first read we prime the mark to
/// the newest id instead of replaying the backlog.
fn deliver_notifications(username: &str, status: &Status, last_id: u64) -> u64 {
    let (show, high) = select_notifications(username, &status.notifications, last_id);
    for n in show {
        notify(&n.title, &n.body, n.urgency == "critical");
    }
    high
}

/// Is a welcome back ("You're back — 15 more minutes") among the
/// notifications this user is about to be shown? Then the warning the
/// welcome already said stays quiet (`WarnState::heard_back`): a "15 minutes
/// left" two seconds after it took its place on screen (acceptance round 4).
fn welcomes_back(username: &str, notifs: &[TrayNotification], last_id: u64) -> bool {
    select_notifications(username, notifs, last_id)
        .0
        .iter()
        .any(|n| n.kind == "back")
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Parent-mode worker: owns the HTTP client and a tokio runtime, polls the
/// server for pending requests + alerts every `PARENT_POLL`, notifies on new
/// ones, keeps the tray's pending list current, and carries out approve/deny
/// actions the menu sends. Runs on its own thread so it never blocks the ksni
/// service or the status poll.
fn spawn_parent_worker(
    cfg: parent::ParentConfig,
    handle: ksni::Handle<OpenScreenTimeTray>,
    rx: mpsc::Receiver<ParentAction>,
) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("parent worker: could not start runtime: {e}");
                return;
            }
        };
        let client = reqwest::Client::new();
        let mut seen_pending: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen_alerts: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Skip notifications on the first pass so a companion starting up doesn't
        // announce the entire existing backlog.
        let mut primed = false;

        loop {
            // Act on a queued approve/deny immediately; otherwise wake to poll.
            match rx.recv_timeout(PARENT_POLL) {
                Ok(action) => {
                    let (id, approve) = match action {
                        ParentAction::Approve(id) => (id, true),
                        ParentAction::Deny(id) => (id, false),
                    };
                    match rt.block_on(parent::api::decide(&client, &cfg, &id, approve)) {
                        Ok(()) => notify(
                            if approve { "Given" } else { "Not now" },
                            if approve {
                                "The extra time is on its way."
                            } else {
                                "They'll hear it was a no for now."
                            },
                            false,
                        ),
                        Err(e) => {
                            tracing::warn!("parent decide failed: {e}");
                            notify(
                                "Couldn't update",
                                "Check the connection and try again.",
                                false,
                            );
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            match rt.block_on(parent::api::pending(&client, &cfg)) {
                Ok(pending) => {
                    if primed {
                        for r in &pending {
                            if !seen_pending.contains(&r.id) {
                                notify(
                                    &format!("{} asked for {} more minutes", r.who(), r.minutes),
                                    &format!("{} · {}", r.task_label, r.device_name),
                                    false,
                                );
                            }
                        }
                    }
                    seen_pending = pending.iter().map(|r| r.id.clone()).collect();
                    handle.update(move |t: &mut OpenScreenTimeTray| t.pending = pending.clone());
                }
                Err(e) => tracing::debug!("parent pending poll failed: {e}"),
            }

            match rt.block_on(parent::api::alerts(&client, &cfg)) {
                Ok(alerts) => {
                    if primed {
                        for a in &alerts {
                            if a.severity == "critical" && !seen_alerts.contains(&a.id) {
                                let msg = a
                                    .payload
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or(&a.etype);
                                notify(
                                    &format!("Alert · {}", a.etype.replace('_', " ")),
                                    msg,
                                    true,
                                );
                            }
                        }
                    }
                    seen_alerts = alerts.iter().map(|a| a.id.clone()).collect();
                }
                Err(e) => tracing::debug!("parent alerts poll failed: {e}"),
            }
            primed = true;
        }
    });
}

/// Blocking loop: spawn the ksni DBus service, then read the status file
/// whenever the agent rewrites it (at least every 5 s), pushing updates into
/// the tray via the service handle.
pub fn run() -> Result<()> {
    let username = std::env::var("USER")
        .ok()
        .or_else(current_username)
        .ok_or_else(|| {
            anyhow::anyhow!("cannot determine the current user ($USER unset and no uid entry)")
        })?;
    let Some(_instance) = single_instance() else {
        tracing::info!("the companion is already running for {username}");
        return Ok(());
    };
    tracing::info!(
        "companion starting for {username} (reading {})",
        status_path()
    );

    let mut prev = read_status(&username);
    if prev.is_none() {
        tracing::warn!(
            "{} not readable yet — is openscreentime running?",
            status_path()
        );
    }

    // High-water mark for the notification queue: prime to whatever is already
    // present so a tray starting up mid-day doesn't replay the backlog.
    let mut last_notif_id = prev
        .as_ref()
        .and_then(|s| s.notifications.iter().map(|n| n.id).max())
        .unwrap_or(0);

    // Parent mode is enabled iff this machine has been paired.
    let parent_cfg = parent::ParentConfig::load();
    let (tray_tx, worker_rx) = match parent_cfg {
        Some(_) => {
            let (tx, rx) = mpsc::channel::<ParentAction>();
            (Some(tx), Some(rx))
        }
        None => (None, None),
    };

    let service = ksni::TrayService::new(OpenScreenTimeTray {
        username: username.clone(),
        status: prev.clone(),
        pending: Vec::new(),
        action_tx: tray_tx,
    });
    let handle = service.handle();
    service.spawn();

    if let (Some(cfg), Some(rx)) = (parent_cfg, worker_rx) {
        tracing::info!("parent mode enabled (paired with {})", cfg.server_url);
        spawn_parent_worker(cfg, handle.clone(), rx);
    }

    // First run: open the app window once — it shows the first-run cards.
    // Only on a gui+tray build (the window needs the gui presenter).
    #[cfg(feature = "gui")]
    if !crate::app::intro_seen() {
        open_app();
    }

    let mut warner = Warner::default();
    let ring_of = |s: &Status| {
        let me = s.user(&username);
        (
            me.and_then(UserStatus::frac),
            me.is_some_and(|u| u.can_ask && !u.self_managed),
        )
    };
    loop {
        if let Some(n) = &prev {
            let (frac, ask) = ring_of(n);
            warner.observe(next_stop(n, &username, chrono::Local::now()), frac, ask);
        }
        wait_for_status(&username, POLL_INTERVAL);
        let next = read_status(&username);
        if let (Some(p), Some(n)) = (&prev, &next) {
            if p != n {
                notify_transitions(p, n);
            }
        }
        if let Some(n) = &next {
            if welcomes_back(&username, &n.notifications, last_notif_id) {
                warner.state.heard_back(chrono::Utc::now().timestamp());
            }
            // A stop that's gone (a thaw) closes its last-minute notice
            // before "You're back" arrives.
            let (frac, ask) = ring_of(n);
            warner.observe(next_stop(n, &username, chrono::Local::now()), frac, ask);
            last_notif_id = deliver_notifications(&username, n, last_notif_id);
            let now = chrono::Utc::now();
            let live: Vec<_> = n
                .login_codes
                .iter()
                .filter(|c| c.is_live(now))
                .cloned()
                .collect();
            crate::logincode::close_stale(&live);
            for c in &live {
                if crate::logincode::first_sighting(c) {
                    crate::logincode::notify(c);
                }
            }
        } else {
            // The agent is gone (stopped, or taken off this computer): a
            // code it published is no longer anyone's to type.
            crate::logincode::close_stale(&[]);
        }
        if prev != next {
            let for_tray = next.clone();
            handle.update(move |t| t.status = for_tray);
        }
        prev = next;
    }
}

fn current_username() -> Option<String> {
    users::get_current_username().map(|s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notif(id: u64, user: Option<&str>) -> TrayNotification {
        TrayNotification {
            id,
            title: "T".into(),
            body: "B".into(),
            urgency: "normal".into(),
            user: user.map(str::to_string),
            kind: String::new(),
        }
    }

    /// Acceptance round 4, step 6: after "Give 15" the welcome went to the
    /// tray and "15 minutes left" was the first thing on screen. The agent
    /// marks the welcome; the companion sees it — only a new one, only its
    /// own — and holds the warning it already said.
    #[test]
    fn a_new_welcome_back_quiets_the_warning_it_already_said() {
        let s = status(
            r#"{"notifications":[
                {"id":7,"title":"You're back — 15 more minutes","body":"A parent gave you more time.","user":"mia","kind":"back"},
                {"id":8,"title":"Not this time","body":"…","user":"mia"}
            ]}"#,
        );
        assert!(welcomes_back("mia", &s.notifications, 6));
        assert!(!welcomes_back("mia", &s.notifications, 7), "already shown");
        assert!(!welcomes_back("dad", &s.notifications, 6), "not his");
        // An agent from before the mark: no field, nothing held back.
        let old = status(r#"{"notifications":[{"id":9,"title":"You're back","user":"mia"}]}"#);
        assert!(!welcomes_back("mia", &old.notifications, 0));

        // What the companion then does with the stop 15 minutes out.
        let mut w = WarnState::default();
        let now = chrono::Utc::now().timestamp();
        w.heard_back(now);
        assert_eq!(w.observe_stop(StopReason::Limit, 898, now + 898), None);
        assert_eq!(w.observe_stop(StopReason::Limit, 299, now + 898), Some(5));
    }

    #[test]
    fn shows_only_new_targeted_or_broadcast() {
        let notifs = vec![
            notif(1, Some("kid")),   // already seen
            notif(2, Some("kid")),   // new, mine
            notif(3, Some("other")), // new, not mine
            notif(4, None),          // new, broadcast
        ];
        let (show, high) = select_notifications("kid", &notifs, 1);
        let ids: Vec<u64> = show.iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![2, 4]);
        assert_eq!(high, 4);
    }

    fn status(json: &str) -> Status {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn the_next_stop_comes_from_whatever_the_agent_publishes() {
        let now = chrono::Local::now();
        let at = (now + chrono::Duration::minutes(5)).to_rfc3339();
        // stop_at + reason (bedtime)
        let stop = |s: &Status| match next_stop(s, "mia", now) {
            Upcoming::Stop(r, secs, _) => (r, secs),
            other => panic!("expected a stop, got {other:?}"),
        };
        let s = status(&format!(
            r#"{{"users":[{{"name":"mia","remaining_minutes":40,"stop_at":"{at}","reason":"bedtime"}}]}}"#
        ));
        let (r, secs) = stop(&s);
        assert_eq!(r, StopReason::Bedtime);
        assert!((299..=300).contains(&secs));
        // Only the old field: the daily limit's minutes.
        let s = status(r#"{"users":[{"name":"mia","remaining_minutes":12}]}"#);
        assert_eq!(stop(&s).0, StopReason::Limit);
        // A parent's pause with a window is published as the next stop.
        let s = status(&format!(
            r#"{{"users":[{{"name":"mia","remaining_minutes":12,"stop_at":"{at}","reason":"paused"}}]}}"#
        ));
        assert_eq!(stop(&s).0, StopReason::Paused);
        // An armed countdown wins over the published stop.
        let s = status(&format!(
            r#"{{"users":[{{"name":"mia","stop_at":"{at}","reason":"limit","freeze_in_secs":50}}]}}"#
        ));
        assert!((49..=50).contains(&stop(&s).1));
        // The limit while the minutes are being used: a real moment.
        let s = status(&format!(
            r#"{{"users":[{{"name":"mia","stop_at":"{at}","reason":"limit","counting":true}}]}}"#
        ));
        assert_eq!(stop(&s).0, StopReason::Limit);
        // Stopped, or not managed here: nothing to warn about.
        let s = status(r#"{"users":[{"name":"mia","remaining_minutes":0,"frozen":true}]}"#);
        assert_eq!(next_stop(&s, "mia", now), Upcoming::Nothing);
        assert_eq!(next_stop(&s, "dad", now), Upcoming::Nothing);
    }

    /// Acceptance, step 4: at login, before anyone touched anything, the
    /// warning said "ends at 23:37" — the forecast of someone using the
    /// screen from that second — and the stop came at 23:38:51. An idle
    /// person's limit stop is not a moment yet: nothing is announced until
    /// the minutes are really being used.
    #[test]
    fn an_idle_limit_is_not_announced_with_a_time_it_wont_keep() {
        let now = chrono::Local::now();
        let at = (now + chrono::Duration::minutes(5)).to_rfc3339();
        let s = status(&format!(
            r#"{{"users":[{{"name":"mia","allowed":true,"remaining_minutes":5,"minutes_left":5,
                "stop_at":"{at}","reason":"limit","counting":false}}]}}"#
        ));
        assert_eq!(next_stop(&s, "mia", now), Upcoming::Hold);
    }

    fn tray_for(json: &str) -> OpenScreenTimeTray {
        OpenScreenTimeTray {
            username: "mia".into(),
            status: Some(status(json)),
            pending: Vec::new(),
            action_tx: None,
        }
    }

    #[test]
    fn the_tray_ring_shows_the_share_of_the_day() {
        use crate::mark::TrayState;
        let t = tray_for(
            r#"{"connection":"online","users":[{"name":"mia","used_minutes":48,"remaining_minutes":27}]}"#,
        );
        match t.tray_state() {
            TrayState::Ok { frac } => assert!((frac - 0.64).abs() < 0.001),
            other => panic!("{other:?}"),
        }
        assert_eq!(t.time_line(), "27 minutes left");
        assert_eq!(t.connection_line(), "Connected");
        let t = tray_for(r#"{"users":[{"name":"mia","used_minutes":80,"remaining_minutes":10}]}"#);
        assert!(matches!(t.tray_state(), TrayState::Low { .. }));
        let t = tray_for(
            r#"{"users":[{"name":"mia","used_minutes":90,"remaining_minutes":0,"frozen":true}]}"#,
        );
        assert_eq!(t.tray_state(), TrayState::Stopped);
        assert_eq!(t.time_line(), "Time's up for today");
        let t =
            tray_for(r#"{"device_locked":true,"users":[{"name":"mia","remaining_minutes":20}]}"#);
        assert_eq!(t.tray_state(), TrayState::Paused);
        assert_eq!(t.time_line(), "Paused by a parent");
        let t = tray_for(r#"{"users":[{"name":"mia","used_minutes":5}]}"#);
        assert_eq!(t.tray_state(), TrayState::Idle);
        assert_eq!(t.time_line(), "No limit today");
        for t in [t.time_line(), t.connection_line().to_string()] {
            assert_ne!(t, t.to_uppercase(), "no shouting");
        }
        // Unlocked with a code after time's up (acceptance step 5): the
        // override's time, never "time's up".
        let until = (chrono::Local::now() + chrono::Duration::minutes(29)).to_rfc3339();
        let t = tray_for(&format!(
            r#"{{"users":[{{"name":"mia","used_minutes":6,"remaining_minutes":-1,"allowed":true,
                "reason":"limit","minutes_left":29,"stop_at":"{until}","override_until":"{until}",
                "counting":true}}]}}"#
        ));
        assert!(
            t.time_line().starts_with("Unlocked until "),
            "{}",
            t.time_line()
        );
        assert!(matches!(t.tray_state(), TrayState::Ok { .. }));
    }

    #[test]
    fn priming_to_newest_suppresses_backlog() {
        let notifs = vec![notif(1, None), notif(2, None), notif(3, None)];
        // Prime as the run loop does: last_id = max present.
        let last = notifs.iter().map(|n| n.id).max().unwrap();
        let (show, high) = select_notifications("kid", &notifs, last);
        assert!(show.is_empty());
        assert_eq!(high, 3);
    }
}
