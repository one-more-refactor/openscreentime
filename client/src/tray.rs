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
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct UserStatus {
    name: String,
    #[serde(default)]
    used_minutes: u64,
    /// `None` = no daily limit configured.
    #[serde(default)]
    remaining_minutes: Option<i64>,
    #[serde(default)]
    frozen: bool,
    /// Countdown to an imminent session freeze, if one is pending.
    #[serde(default)]
    freeze_in_secs: Option<u64>,
    /// The next stop (RFC 3339) and why — what the warnings count down to.
    #[serde(default)]
    stop_at: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

impl Status {
    fn user<'a>(&'a self, name: &str) -> Option<&'a UserStatus> {
        self.users.iter().find(|u| u.name == name)
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

    /// "TIME LEFT: NN MIN" / "NO LIMIT" / "PAUSED" — the headline for the
    /// current user, or a device-level line when we are not a managed user.
    fn time_line(&self) -> String {
        match self.me() {
            Some(u) if u.frozen => "PAUSED".to_string(),
            Some(u) => match u.remaining_minutes {
                Some(m) => format!("TIME LEFT: {} MIN", m.max(0)),
                None => "NO LIMIT".to_string(),
            },
            None => "DEVICE MANAGED".to_string(),
        }
    }

    fn connection_line(&self) -> &'static str {
        match self.status.as_ref().map(|s| s.connection.as_str()) {
            Some("online") => "ONLINE",
            Some("offline_fail_closed") => "OFFLINE — LOCKED",
            Some(_) => "OFFLINE",
            None => "AGENT NOT RUNNING",
        }
    }

    /// Anything that means "restricted right now" for this user/device.
    fn restricted(&self) -> bool {
        let Some(s) = &self.status else { return false };
        s.connection == "offline_fail_closed"
            || s.device_locked
            || s.offline_hard_lockdown
            || s.tamper_lockdown
            || self.me().is_some_and(|u| u.frozen)
    }
}

impl ksni::Tray for OpenScreenTimeTray {
    fn id(&self) -> String {
        "openscreentime".into()
    }

    fn title(&self) -> String {
        "OPENSCREENTIME".into()
    }

    fn icon_name(&self) -> String {
        // Themed freedesktop icon names — no bundled assets.
        let name = match &self.status {
            None => "security-medium",
            Some(_) if self.restricted() => "security-low",
            Some(s) if s.connection == "online" => "security-high",
            Some(_) => "security-medium",
        };
        name.into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "OPENSCREENTIME".into(),
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
                label: format!("CONNECTION: {}", self.connection_line()),
                enabled: false,
                ..Default::default()
            }
            .into(),
        ];
        // Parent mode: pending time requests, each with approve/deny.
        if self.action_tx.is_some() && !self.pending.is_empty() {
            items.push(MenuItem::Separator);
            items.push(
                StandardItem {
                    label: format!("{} TIME REQUEST(S)", self.pending.len()),
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
                        label: format!("{} · +{} MIN · {}", r.who(), r.minutes, r.task_label),
                        submenu: vec![
                            StandardItem {
                                label: format!("APPROVE +{} MIN", r.minutes),
                                activate: Box::new(move |t: &mut Self| {
                                    if let Some(tx) = &t.action_tx {
                                        let _ = tx.send(ParentAction::Approve(approve_id.clone()));
                                    }
                                }),
                                ..Default::default()
                            }
                            .into(),
                            StandardItem {
                                label: "DENY".into(),
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

        // The managed user can ask for more time straight from the tray. Shown
        // whenever this user is managed (has a status entry).
        if self.me().is_some() {
            items.push(MenuItem::Separator);
            items.push(
                StandardItem {
                    label: "REQUEST MORE TIME".into(),
                    activate: Box::new(|_: &mut Self| request_more_time()),
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "ABOUT OPENSCREENTIME".into(),
                activate: Box::new(|_: &mut Self| {
                    notify(
                        "OPENSCREENTIME",
                        "This device is managed. Screen time and network filtering are active.",
                        false,
                    );
                }),
                ..Default::default()
            }
            .into(),
        );
        items
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
        notify("Couldn't send", "Try again in a moment", false);
        return;
    }
    if let Err(e) = std::fs::write(dir.join("earn_request"), b"1") {
        tracing::debug!("could not write earn-request marker: {e}");
        notify("Couldn't send", "Try again in a moment", false);
        return;
    }
    notify(
        "REQUEST SENT",
        "Asked for more time — waiting for a parent",
        false,
    );
}

fn notify(summary: &str, body: &str, critical: bool) {
    let mut n = notify_rust::Notification::new();
    n.appname("OpenScreenTime")
        .summary(summary)
        .body(body)
        .icon("security-medium");
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
                "Offline too long",
                "This computer stays on its last rules until it reconnects.",
                true,
            );
        } else if next.connection == "online" {
            notify("Back online", "Connection to the server restored", false);
        }
    }
    match (prev.offline_hard_lockdown, next.offline_hard_lockdown) {
        (false, true) => notify("Lockdown active", "The device is in offline lockdown", true),
        (true, false) => notify("Lockdown lifted", "Normal use has resumed", false),
        _ => {}
    }
    match (prev.tamper_lockdown, next.tamper_lockdown) {
        (false, true) => notify(
            "OpenScreenTime was changed",
            "Ask a parent — their unlock code opens it.",
            true,
        ),
        (true, false) => notify("Tamper lock lifted", "Normal use has resumed", false),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Warnings before a stop
// ---------------------------------------------------------------------------

fn parse_at(s: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&chrono::Local))
}

/// The stop coming up for this user, from what the snapshot publishes: an
/// armed save-your-work countdown, else the rules' next stop (`stop_at` +
/// `reason`, a parent's scheduled pause included), else — from an agent that
/// predates those fields — the daily limit's minutes. `None` while they are
/// stopped, or unmanaged.
fn next_stop(
    status: &Status,
    username: &str,
    now: chrono::DateTime<chrono::Local>,
) -> Option<(StopReason, i64, chrono::DateTime<chrono::Local>)> {
    let me = status.user(username)?;
    if me.frozen {
        return None;
    }
    let reason = me
        .reason
        .as_deref()
        .and_then(warn::parse_reason)
        .unwrap_or(StopReason::Limit);
    let (reason, at) = if let Some(s) = me.freeze_in_secs {
        (reason, now + chrono::Duration::seconds(s as i64))
    } else if let Some(at) = me.stop_at.as_deref().and_then(parse_at) {
        (reason, at)
    } else if me.reason.is_none() {
        let m = me.remaining_minutes.filter(|m| *m > 0)?;
        (StopReason::Limit, now + chrono::Duration::minutes(m))
    } else {
        return None;
    };
    Some((reason, (at - now).num_seconds(), at))
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
    fn observe(&mut self, stop: Option<(StopReason, i64, chrono::DateTime<chrono::Local>)>) {
        let Some((reason, secs, at)) = stop.filter(|(_, s, _)| *s > 0) else {
            self.state.clear();
            if let Some(h) = self.last_minute.take() {
                h.close();
            }
            return;
        };
        let due = self.state.observe(reason, secs);
        let w = warn::words(reason, secs, Some(at));
        if let Some(h) = self.last_minute.as_mut() {
            if w.title != self.shown_title {
                h.summary(&w.title).body(&w.body);
                let _ = h.update();
                self.shown_title = w.title;
            }
            return;
        }
        if due.is_none() {
            return;
        }
        let ask = reason != StopReason::Paused;
        match show_warning(&w, ask) {
            Some(h) if w.critical => {
                self.shown_title = w.title;
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
        "open" => {
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe)
                    .arg("app")
                    .stdin(std::process::Stdio::null())
                    .spawn();
            }
        }
        _ => {}
    }
}

fn show_warning(w: &warn::Words, ask: bool) -> Option<notify_rust::NotificationHandle> {
    let mut n = notify_rust::Notification::new();
    n.appname("OpenScreenTime")
        .summary(&w.title)
        .body(&w.body)
        .icon("openscreentime")
        .hint(notify_rust::Hint::DesktopEntry("openscreentime".into()));
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
                            if approve { "APPROVED" } else { "DENIED" },
                            "Time request updated",
                            false,
                        ),
                        Err(e) => {
                            tracing::warn!("parent decide failed: {e}");
                            notify(
                                "COULDN'T UPDATE",
                                "Check the connection and try again",
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
                                    &format!("{} WANTS +{} MIN", r.who().to_uppercase(), r.minutes),
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
                                notify(&format!("ALERT · {}", a.etype.to_uppercase()), msg, true);
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

/// Blocking loop: spawn the ksni DBus service, then poll the status file
/// every 5s, pushing updates into the tray via the service handle.
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

    // First-run intro (skippable child-facing cards), shown once. Only on a
    // gui+tray build — the intro window needs the gui presenter.
    #[cfg(feature = "gui")]
    maybe_show_intro();

    let mut warner = Warner::default();
    loop {
        if let Some(n) = &prev {
            warner.observe(next_stop(n, &username, chrono::Local::now()));
        }
        std::thread::sleep(POLL_INTERVAL);
        let next = read_status(&username);
        if let (Some(p), Some(n)) = (&prev, &next) {
            if p != n {
                notify_transitions(p, n);
            }
        }
        if let Some(n) = &next {
            // A stop that's gone (a thaw) closes its last-minute notice
            // before "You're back" arrives.
            warner.observe(next_stop(n, &username, chrono::Local::now()));
            last_notif_id = deliver_notifications(&username, n, last_notif_id);
            let now = chrono::Utc::now();
            for c in n.login_codes.iter().filter(|c| c.is_live(now)) {
                if crate::logincode::first_sighting(c) {
                    crate::logincode::notify(c);
                }
            }
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

/// Show the first-run intro once, as a detached subprocess so it never blocks
/// the tray. No-op if it's already been seen.
#[cfg(feature = "gui")]
fn maybe_show_intro() {
    if crate::intro::already_seen() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    match std::process::Command::new(exe)
        .arg("__intro")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => {} // the subprocess marks itself seen when it closes
        Err(e) => tracing::debug!("could not spawn first-run intro: {e}"),
    }
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
        }
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
        let s = status(&format!(
            r#"{{"users":[{{"name":"mia","remaining_minutes":40,"stop_at":"{at}","reason":"bedtime"}}]}}"#
        ));
        let (r, secs, _) = next_stop(&s, "mia", now).unwrap();
        assert_eq!(r, StopReason::Bedtime);
        assert!((299..=300).contains(&secs));
        // Only the old field: the daily limit's minutes.
        let s = status(r#"{"users":[{"name":"mia","remaining_minutes":12}]}"#);
        assert_eq!(next_stop(&s, "mia", now).unwrap().0, StopReason::Limit);
        // A parent's pause with a window is published as the next stop.
        let s = status(&format!(
            r#"{{"users":[{{"name":"mia","remaining_minutes":12,"stop_at":"{at}","reason":"paused"}}]}}"#
        ));
        assert_eq!(next_stop(&s, "mia", now).unwrap().0, StopReason::Paused);
        // An armed countdown wins over the published stop.
        let s = status(&format!(
            r#"{{"users":[{{"name":"mia","stop_at":"{at}","reason":"limit","freeze_in_secs":50}}]}}"#
        ));
        let (_, secs, _) = next_stop(&s, "mia", now).unwrap();
        assert!((49..=50).contains(&secs));
        // Stopped, or not managed here: nothing to warn about.
        let s = status(r#"{"users":[{"name":"mia","remaining_minutes":0,"frozen":true}]}"#);
        assert!(next_stop(&s, "mia", now).is_none());
        assert!(next_stop(&s, "dad", now).is_none());
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
