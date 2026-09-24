//! The lock: its own session, on its own VT.
//!
//! Why it exists. A screen-time stop freezes the person's whole
//! `user-<uid>.slice` with the cgroup freezer. On every modern desktop that
//! slice contains the compositor, so anything drawn *inside* the session — the
//! old root-run overlay — kept a dead picture on screen and never got a key.
//! Codes could not be typed, VTs could not be switched. So the lock no longer
//! lives in the session it stops:
//!
//! * **Graphical lock**: `openscreentime-lock@<vt>.service` runs the kiosk
//!   compositor `cage` (without `-s`: the keyboard cannot switch VTs) as the
//!   unprivileged system user `ost-lock`, in its own logind session on a
//!   dedicated VT, hosting the egui lock UI (`ost __lockscreen`). A separate
//!   unit, so an agent restart (watchdog, self-update) never takes it down.
//! * **Text lock** (no cage, a headless build, or cage didn't come up in a
//!   few seconds): the agent itself draws a plain text lock on that same VT
//!   and locks VT switching (`VT_LOCKSWITCH`, what `vlock -a` does).
//!
//! Both ask the agent to check a typed code — the graphical one over a
//! root-owned socket that only `ost-lock` may use ([`socket`]), the text one
//! in-process — and the agent verifies it with the same offline
//! [`crate::parentcode::Verifier`] as everything else. Nothing on the lock side
//! holds a secret.
//!
//! Order of operations, which is the whole trick:
//! * **lock**: start the lock → switch to its VT *while the person's compositor
//!   is still alive* (it hands over the display and input cleanly) → only then
//!   freeze the slice, compositor included;
//! * **unlock**: thaw first → switch back to the person's session → stop the lock.
//!
//! The agent owns the lock's lifetime: [`LockScreen`] is driven from the runner
//! after every tick, command and lock request, so every path that thaws someone
//! also takes the lock down.

#[cfg(feature = "gui")]
pub mod screen;
pub mod socket;
pub mod text;
pub mod vt;

use crate::parentcode::Verifier;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The lock's VT. High enough to stay clear of display managers (tty1),
/// user sessions and logind's autovt gettys (tty1–6), and journald's
/// traditional console (tty12).
pub const LOCK_VT: u32 = 13;
/// The unprivileged system user the graphical lock runs as. No shell, no home,
/// no secrets: all it can do is draw and ask the agent.
pub const LOCK_USER: &str = "ost-lock";
/// Minutes a code typed at the lock buys.
pub const UNLOCK_MINUTES: u32 = 30;
/// Installed by `install-service` on a GUI build.
pub const UNIT_TEMPLATE_PATH: &str = "/etc/systemd/system/openscreentime-lock@.service";
pub const PAM_PATH: &str = "/etc/pam.d/openscreentime-lock";
pub const UNIT_TEMPLATE: &str = include_str!("../../systemd/openscreentime-lock@.service");
pub const PAM_BODY: &str = "# Managed by openscreentime — the lock screen's logind session.\n\
# It runs as ost-lock (no password, no shell, no secrets), so there is nothing\n\
# to authenticate; pam_systemd registers the seat session cage needs.\n\
account  required  pam_permit.so\n\
session  optional  pam_systemd.so\n";

/// How long the graphical lock gets to say hello before the text lock takes over.
const GUI_TIMEOUT: Duration = Duration::from_secs(8);
/// How long the in-process text lock gets to draw.
const TEXT_TIMEOUT: Duration = Duration::from_secs(3);
/// A graphical lock UI polls every second or two; this long without a word
/// means it hung, and the text lock takes over.
const UI_STALE: Duration = Duration::from_secs(20);

pub fn unit_name(vt: u32) -> String {
    format!("openscreentime-lock@{vt}.service")
}

// ── What the lock says ────────────────────────────────────────────────────────

/// Which ring the lock draws (DESIGN-CLIENT.md §1/§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    /// Time's up, or a whole-device stop: full red ring, padlock.
    Wall,
    /// Bedtime / outside allowed hours: full ink ring, moon.
    Night,
    /// A parent paused it: dashed ring, pause bars.
    Paused,
}

/// Can a code be typed, and how many tries are left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CodeState {
    /// No unlock code on this computer yet: say so, offer only "ask".
    Unavailable,
    Ready {
        tries_left: u32,
    },
    /// Too many wrong codes; try again in this many seconds.
    Wait {
        secs: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskState {
    Hidden,
    Ready,
    Sent,
}

/// Everything a lock UI shows. Built by the runner, published to both locks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Face {
    pub look: Look,
    /// One plain sentence: "Time's up for today", "Bedtime until 07:00".
    pub title: String,
    /// A quieter second line, possibly empty.
    #[serde(default)]
    pub detail: String,
    /// Whose screen this is.
    #[serde(default)]
    pub who: String,
    pub code: CodeState,
    pub ask: AskState,
    pub help: String,
}

impl Face {
    /// What a lock shows before it has heard from the agent (just started, or
    /// the agent is restarting). True, if vague.
    pub fn waiting() -> Face {
        Face {
            look: Look::Wall,
            title: "This computer is stopped for now".into(),
            detail: String::new(),
            who: String::new(),
            code: CodeState::Unavailable,
            ask: AskState::Hidden,
            help: HELP.into(),
        }
    }
}

pub const HELP: &str = "A parent can unlock this from their console, or read you the unlock code.";
pub const HELP_NO_CODE: &str =
    "There's no unlock code on this computer yet — a parent can unlock it from their console.";

/// The verifier inputs for this device (moved here from the old overlay).
#[derive(Debug, Clone, Default)]
pub struct ParentKeys {
    /// Argon2 PHC hash of a profile-level backup code.
    pub pin_hash: Option<String>,
    /// Base32 TOTP secret behind the unlock code the console shows.
    pub totp_secret: Option<String>,
    /// Unused one-time recovery codes `{id, mac}`.
    pub recovery: Vec<crate::policy::RecoveryCode>,
}

impl ParentKeys {
    pub fn verifier(&self) -> Verifier {
        Verifier::new(self.totp_secret.clone(), self.pin_hash.clone())
            .with_recovery(self.recovery.clone())
    }
}

/// The code field's state for a verifier: tries left, or how long to wait.
pub fn code_state(v: &Verifier) -> CodeState {
    if !v.configured() {
        return CodeState::Unavailable;
    }
    match v.tries() {
        crate::parentcode::Tries::Left(n) => CodeState::Ready { tries_left: n },
        crate::parentcode::Tries::Wait(s) => CodeState::Wait { secs: s },
    }
}

// ── Shared state between the runner, the socket and the text lock ─────────────

#[derive(Debug, Default)]
pub struct Shared {
    /// The face to show; `None` while no lock is up.
    pub face: Option<Face>,
    /// Last time a lock UI spoke to the agent (the graphical lock's socket
    /// requests, or the text lock drawing). How the agent knows a lock is
    /// actually on screen before it freezes anyone.
    pub ui_seen: Option<Instant>,
}

pub type SharedRef = Arc<Mutex<Shared>>;

pub fn shared() -> SharedRef {
    Arc::new(Mutex::new(Shared::default()))
}

fn with_shared<T>(s: &SharedRef, f: impl FnOnce(&mut Shared) -> T) -> T {
    let mut g = s.lock().unwrap_or_else(|p| p.into_inner());
    f(&mut g)
}

pub fn mark_seen(s: &SharedRef) {
    with_shared(s, |sh| sh.ui_seen = Some(Instant::now()));
}

pub fn current_face(s: &SharedRef) -> Option<Face> {
    with_shared(s, |sh| sh.face.clone())
}

/// What wakes the runner between ticks.
pub enum LockEvent {
    /// A lock UI asked for something only the runner can answer.
    Request(socket::Pending),
    /// The VT on screen changed (someone logged in, or switched sessions).
    VtChanged,
}

pub type LockTx = mpsc::Sender<LockEvent>;

/// Watch the active VT and wake the runner when it changes, so a person who
/// switches (or logs in) to a stopped session meets the lock in a moment, not
/// a frozen desktop for up to a tick.
pub fn spawn_vt_watch(tx: LockTx) {
    std::thread::spawn(move || {
        let mut last = vt::active();
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let now = vt::active();
            if now != last {
                last = now;
                if tx.blocking_send(LockEvent::VtChanged).is_err() {
                    return;
                }
            }
        }
    });
}

// ── Sessions (logind) ────────────────────────────────────────────────────────

/// One logind session, as far as the lock cares.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Session {
    pub id: String,
    pub user: String,
    pub seat: String,
    pub vt: Option<u32>,
    pub active: bool,
    pub graphical: bool,
    pub tty: String,
    pub class: String,
    pub state: String,
}

/// Parse `loginctl show-session A B … -p …` (blank-line separated blocks).
pub fn parse_sessions(out: &str) -> Vec<Session> {
    out.split("\n\n")
        .filter_map(|block| {
            let mut s = Session::default();
            for line in block.lines() {
                let Some((k, v)) = line.split_once('=') else {
                    continue;
                };
                let v = v.trim();
                match k {
                    "Id" => s.id = v.into(),
                    "Name" => s.user = v.into(),
                    "Seat" => s.seat = v.into(),
                    "VTNr" => s.vt = v.parse().ok().filter(|n| *n > 0),
                    "Active" => s.active = v == "yes",
                    "Type" => s.graphical = matches!(v, "wayland" | "x11" | "mir"),
                    "TTY" => s.tty = v.into(),
                    "Class" => s.class = v.into(),
                    "State" => s.state = v.into(),
                    _ => {}
                }
            }
            (!s.id.is_empty()).then_some(s)
        })
        .collect()
}

/// The person whose session is on the screen right now (the active seat0
/// session), never the lock's own session.
pub fn on_screen_user(sessions: &[Session]) -> Option<String> {
    sessions
        .iter()
        .find(|s| {
            s.seat == "seat0"
                && s.active
                && s.state != "closing"
                && s.class.starts_with("user")
                && s.user != LOCK_USER
                && !s.user.is_empty()
        })
        .map(|s| s.user.clone())
}

/// The VT of `user`'s seat0 session (graphical first) — where to go back to.
pub fn session_vt(sessions: &[Session], user: &str) -> Option<u32> {
    let mut mine: Vec<&Session> = sessions
        .iter()
        .filter(|s| s.user == user && s.seat == "seat0" && s.state != "closing" && s.vt.is_some())
        .collect();
    mine.sort_by_key(|s| !s.graphical);
    mine.first().and_then(|s| s.vt)
}

pub fn has_graphical_session(sessions: &[Session], user: &str) -> bool {
    sessions
        .iter()
        .any(|s| s.user == user && s.graphical && s.state != "closing")
}

/// `user`'s terminals (`pts/3`, `tty2`) — where a person with no desktop hears
/// from us. Never anyone else's.
pub fn ttys_of(sessions: &[Session], user: &str) -> Vec<String> {
    let mut t: Vec<String> = sessions
        .iter()
        .filter(|s| s.user == user && s.state != "closing" && !s.tty.is_empty())
        .map(|s| s.tty.clone())
        .filter(|t| {
            (t.starts_with("pts/") || t.starts_with("tty"))
                && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '/')
        })
        .collect();
    t.sort();
    t.dedup();
    t
}

fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

// ── The host: everything the lock does to the machine ─────────────────────────

/// The machine-facing operations of the lock and freeze, behind one seam so
/// the lifecycle can be tested without a machine.
pub trait Host: Send + Sync {
    fn freeze(&self, user: &str, on: bool, hard: bool);
    fn is_frozen(&self, user: &str) -> Option<bool>;
    /// Has a login (a user slice) right now. Logged-out people are never frozen.
    fn logged_in(&self, user: &str) -> bool;
    /// Every human account on the machine.
    fn login_users(&self) -> Vec<String>;
    fn sessions(&self) -> Vec<Session>;
    fn active_vt(&self) -> Option<u32>;
    /// Switch to `vt` and wait (bounded) until it is on screen.
    fn switch_to(&self, vt: u32) -> bool;
    fn switch_lock(&self, on: bool);
    /// Lock unit installed, `cage` present, and a GUI build.
    fn gui_available(&self) -> bool;
    fn start_gui(&self, vt: u32) -> bool;
    fn stop_gui(&self, vt: u32);
    fn gui_running(&self, vt: u32) -> bool;
    fn start_text(&self, vt: u32) -> bool;
    fn stop_text(&self);
    fn text_running(&self) -> bool;
    /// Say something on `user`'s own terminals (only when they have no desktop).
    fn tell_ttys(&self, user: &str, msg: &str);
}

/// The real machine.
pub struct SystemHost {
    exec: crate::util::Exec,
    shared: SharedRef,
    tx: LockTx,
    text: Mutex<Option<text::TextLock>>,
}

impl SystemHost {
    pub fn new(exec: crate::util::Exec, shared: SharedRef, tx: LockTx) -> Self {
        SystemHost {
            exec,
            shared,
            tx,
            text: Mutex::new(None),
        }
    }

    fn text_slot(&self) -> std::sync::MutexGuard<'_, Option<text::TextLock>> {
        self.text.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Is `bin` installed in one of the usual places?
pub fn which(bin: &str) -> bool {
    ["/usr/bin", "/usr/local/bin", "/bin", "/usr/sbin"]
        .iter()
        .any(|d| std::path::Path::new(d).join(bin).exists())
}

impl Host for SystemHost {
    fn freeze(&self, user: &str, on: bool, hard: bool) {
        if let Err(e) = crate::enforce::screentime::freeze_user(&self.exec, user, on, hard) {
            tracing::warn!("{} {user} failed: {e}", if on { "freeze" } else { "thaw" });
        }
    }
    fn is_frozen(&self, user: &str) -> Option<bool> {
        crate::enforce::screentime::is_frozen(user)
    }
    fn logged_in(&self, user: &str) -> bool {
        // A login session, not merely a user slice: a lingering user's
        // manager keeps a slice around with nobody there.
        self.sessions()
            .iter()
            .any(|s| s.user == user && s.class.starts_with("user") && s.state != "closing")
    }
    fn login_users(&self) -> Vec<String> {
        crate::sysusers::login_users()
            .into_iter()
            .map(|u| u.username)
            .collect()
    }
    fn sessions(&self) -> Vec<Session> {
        let list = self
            .exec
            .probe("loginctl", &["list-sessions", "--no-legend"]);
        let ids: Vec<&str> = list
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .take(64)
            .collect();
        if ids.is_empty() {
            return Vec::new();
        }
        let mut args = vec!["show-session"];
        args.extend(ids.iter().copied());
        for p in [
            "Id", "Name", "Seat", "VTNr", "Active", "Type", "TTY", "Class", "State",
        ] {
            args.extend(["-p", p]);
        }
        parse_sessions(&self.exec.probe("loginctl", &args))
    }
    fn active_vt(&self) -> Option<u32> {
        vt::active()
    }
    fn switch_to(&self, target: u32) -> bool {
        if self.exec.dry_run() {
            tracing::info!(target: "dry_run", "WOULD SWITCH to VT {target}");
            return true;
        }
        if vt::switch_to(target, Duration::from_secs(3)) {
            return true;
        }
        // Whoever owns the VT on screen didn't let go. Take it back to
        // automatic switching (root may) and try once more.
        if let Some(cur) = vt::active() {
            tracing::warn!("VT {cur} did not release; resetting it to automatic switching");
            vt::force_auto(cur);
        }
        vt::switch_to(target, Duration::from_secs(2))
    }
    fn switch_lock(&self, on: bool) {
        if self.exec.dry_run() {
            tracing::info!(target: "dry_run", "WOULD {} VT switching", if on { "LOCK" } else { "UNLOCK" });
            return;
        }
        if !vt::set_switch_lock(on) {
            tracing::warn!(
                "could not {} VT switching",
                if on { "lock" } else { "unlock" }
            );
        }
    }
    fn gui_available(&self) -> bool {
        cfg!(feature = "gui")
            && !self.exec.dry_run()
            && std::path::Path::new(UNIT_TEMPLATE_PATH).exists()
            && users::get_user_by_name(LOCK_USER).is_some()
            && which("cage")
    }
    fn start_gui(&self, vt: u32) -> bool {
        let unit = unit_name(vt);
        let _ = self.exec.run("systemctl", &["reset-failed", &unit]);
        match self.exec.run("systemctl", &["start", "--no-block", &unit]) {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("could not start the lock ({unit}): {e}");
                false
            }
        }
    }
    fn stop_gui(&self, vt: u32) {
        let _ = self
            .exec
            .run("systemctl", &["stop", "--no-block", &unit_name(vt)]);
    }
    fn gui_running(&self, vt: u32) -> bool {
        if self.exec.dry_run() {
            return true;
        }
        let out = self.exec.probe("systemctl", &["is-active", &unit_name(vt)]);
        matches!(out.trim(), "active" | "activating" | "reloading")
    }
    fn start_text(&self, vt: u32) -> bool {
        if self.exec.dry_run() {
            tracing::info!(target: "dry_run", "WOULD DRAW the text lock on VT {vt}");
            mark_seen(&self.shared);
            return true;
        }
        let mut slot = self.text_slot();
        if slot.as_ref().is_some_and(|t| t.running()) {
            return true;
        }
        match text::TextLock::start(vt, self.shared.clone(), self.tx.clone()) {
            Ok(t) => {
                *slot = Some(t);
                true
            }
            Err(e) => {
                tracing::warn!("could not draw the text lock on VT {vt}: {e}");
                false
            }
        }
    }
    fn stop_text(&self) {
        if let Some(t) = self.text_slot().take() {
            t.stop();
        }
    }
    fn text_running(&self) -> bool {
        self.exec.dry_run() || self.text_slot().as_ref().is_some_and(|t| t.running())
    }
    fn tell_ttys(&self, user: &str, msg: &str) {
        use std::io::Write;
        let ttys = ttys_of(&self.sessions(), user);
        if self.exec.dry_run() {
            tracing::info!(target: "dry_run", "WOULD TELL {user} on {ttys:?}: {msg}");
            return;
        }
        for t in ttys {
            let path = format!("/dev/{t}");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
                .open(&path)
            {
                let _ = write!(f, "\r\n\x07OpenScreenTime: {msg}\r\n");
            }
        }
    }
}

use std::os::unix::fs::OpenOptionsExt;

// ── The lock's lifetime ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Gui,
    Text,
}

/// A lock on screen. Persisted with the freeze state, so a restarted agent
/// adopts the lock it left up instead of forgetting it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shown {
    /// The person the lock stands in front of.
    pub subject: String,
    pub vt: u32,
    /// The VT that was on screen before, if the subject's session can't be found.
    #[serde(default)]
    pub return_vt: Option<u32>,
    pub mode: Mode,
    /// A lock from a previous boot is history, not a lock.
    #[serde(default)]
    pub boot_id: String,
}

pub struct LockScreen {
    host: Box<dyn Host>,
    shared: SharedRef,
    vt: u32,
    shown: Option<Shown>,
}

impl LockScreen {
    pub fn new(host: Box<dyn Host>, shared: SharedRef, persisted: Option<Shown>) -> Self {
        let shown = persisted.filter(|s| s.boot_id == boot_id());
        if shown.is_some() {
            // Adopted from the previous run: give its UI the benefit of the
            // doubt until it has had time to reconnect.
            mark_seen(&shared);
        }
        LockScreen {
            host,
            shared,
            vt: shown.as_ref().map(|s| s.vt).unwrap_or(LOCK_VT),
            shown,
        }
    }

    pub fn host(&self) -> &dyn Host {
        self.host.as_ref()
    }

    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    pub fn subject(&self) -> Option<&str> {
        self.shown.as_ref().map(|s| s.subject.as_str())
    }

    /// Publish what the lock says (both locks read it).
    pub fn publish(&self, face: Face) {
        with_shared(&self.shared, |s| s.face = Some(face));
    }

    fn ui_seen_since(&self, t: Instant) -> bool {
        with_shared(&self.shared, |s| s.ui_seen.is_some_and(|x| x >= t))
    }

    async fn wait_ui(&self, since: Instant, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if self.ui_seen_since(since) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.ui_seen_since(since)
    }

    fn set_mode(&mut self, mode: Mode) {
        if let Some(s) = self.shown.as_mut() {
            s.mode = mode;
        }
    }

    /// Put the lock in front of `subject`. Returns once the lock's VT is on
    /// screen and a lock UI is drawing — only then may the caller freeze.
    /// `false` means nothing could be shown; the caller must not freeze
    /// (a frozen desktop with nothing on it is a brick, not a lock).
    pub async fn present(&mut self, subject: &str, face: Face) -> bool {
        self.publish(face);
        if let Some(s) = self.shown.as_mut() {
            s.subject = subject.to_string();
            return self.reassert().await;
        }
        let return_vt = self.host.active_vt().filter(|v| *v != self.vt);
        let gui = self.host.gui_available();
        self.shown = Some(Shown {
            subject: subject.to_string(),
            vt: self.vt,
            return_vt,
            mode: if gui { Mode::Gui } else { Mode::Text },
            boot_id: boot_id(),
        });
        if gui {
            if self.bring_up_gui().await {
                return true;
            }
            tracing::warn!("the graphical lock did not come up; the text lock takes over");
            self.host.stop_gui(self.vt);
            self.set_mode(Mode::Text);
        }
        if self.bring_up_text().await {
            return true;
        }
        tracing::error!("no lock could be shown on VT {}", self.vt);
        self.release();
        false
    }

    async fn bring_up_gui(&mut self) -> bool {
        let since = Instant::now();
        if !self.host.start_gui(self.vt) {
            return false;
        }
        // Switch while the person's compositor is still alive: it gets to hand
        // over the display and input cleanly. The lock draws a moment later.
        let _ = self.host.switch_to(self.vt);
        self.wait_ui(since, GUI_TIMEOUT).await && self.host.switch_to(self.vt)
    }

    async fn bring_up_text(&mut self) -> bool {
        let since = Instant::now();
        if !self.host.start_text(self.vt) {
            return false;
        }
        if !self.host.switch_to(self.vt) {
            self.host.stop_text();
            return false;
        }
        self.host.switch_lock(true);
        self.wait_ui(since, TEXT_TIMEOUT).await
    }

    /// Keep the lock on screen: bring it back if it died or hung, switch back
    /// to it if something switched away. Called every tick while it's up.
    pub async fn reassert(&mut self) -> bool {
        let Some(s) = self.shown.clone() else {
            return false;
        };
        match s.mode {
            Mode::Gui => {
                let alive = self.host.gui_running(s.vt)
                    && with_shared(&self.shared, |sh| {
                        sh.ui_seen.is_some_and(|t| t.elapsed() < UI_STALE)
                    });
                if !alive {
                    tracing::warn!(
                        "the graphical lock stopped answering; the text lock takes over"
                    );
                    self.host.stop_gui(s.vt);
                    self.set_mode(Mode::Text);
                    return self.bring_up_text().await;
                }
            }
            Mode::Text => {
                if !self.host.text_running() {
                    if !self.host.start_text(s.vt) {
                        return false;
                    }
                    self.host.switch_lock(true);
                }
            }
        }
        if self.host.active_vt() == Some(s.vt) {
            return true;
        }
        if s.mode == Mode::Text {
            // Switching is locked; only we switch, and only to here.
            self.host.switch_lock(false);
            let ok = self.host.switch_to(s.vt);
            self.host.switch_lock(true);
            ok
        } else {
            self.host.switch_to(s.vt)
        }
    }

    /// Take the lock down. The caller has already thawed; this puts the
    /// person's own session back on screen and stops the lock, in that order.
    pub fn release(&mut self) {
        let Some(s) = self.shown.take() else {
            return;
        };
        if s.mode == Mode::Text {
            self.host.stop_text();
            self.host.switch_lock(false);
        }
        if self.host.active_vt() == Some(s.vt) {
            let back = session_vt(&self.host.sessions(), &s.subject).or(s.return_vt);
            if let Some(v) = back {
                if !self.host.switch_to(v) {
                    tracing::warn!("could not switch back to VT {v}");
                }
            }
        }
        self.host.stop_gui(s.vt);
        with_shared(&self.shared, |sh| sh.face = None);
    }
}

/// `ost unlock` / `ost recover` run in their own process, possibly with the
/// agent dead: take down whatever lock the agent recorded, after the caller
/// has thawed everyone.
pub fn teardown_recorded(exec: &crate::util::Exec, recorded: Option<Shown>) {
    let (tx, _rx) = mpsc::channel(1);
    let host = SystemHost::new(exec.clone(), shared(), tx);
    let mut lock = LockScreen::new(Box::new(host), shared(), recorded);
    if lock.shown().is_some_and(|s| s.mode == Mode::Text) {
        // The text lock lived in the agent process; its switch lock outlives it.
        lock.host.switch_lock(false);
    }
    lock.release();
}

#[cfg(test)]
pub mod testing {
    //! A fake machine that records what the lock and freeze did, in order.
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[derive(Default)]
    pub struct World {
        pub log: Vec<String>,
        pub vt: u32,
        pub frozen: HashMap<String, bool>,
        pub logged_in: HashSet<String>,
        pub sessions: Vec<Session>,
        pub gui: bool,
        pub gui_running: bool,
        /// Whether a lock UI says hello when started.
        pub ui_answers: bool,
        pub text_running: bool,
        pub told: Vec<(String, String)>,
    }

    #[derive(Clone)]
    pub struct FakeHost {
        pub world: Arc<Mutex<World>>,
        pub shared: SharedRef,
    }

    impl FakeHost {
        pub fn new(shared: SharedRef) -> Self {
            let w = World {
                vt: 1,
                ui_answers: true,
                ..Default::default()
            };
            FakeHost {
                world: Arc::new(Mutex::new(w)),
                shared,
            }
        }
        pub fn w(&self) -> std::sync::MutexGuard<'_, World> {
            self.world.lock().unwrap()
        }
        fn hello(&self) {
            if self.w().ui_answers {
                mark_seen(&self.shared);
            }
        }
    }

    impl Host for FakeHost {
        fn freeze(&self, user: &str, on: bool, _hard: bool) {
            let mut w = self.w();
            w.log
                .push(format!("{} {user}", if on { "freeze" } else { "thaw" }));
            if w.logged_in.contains(user) {
                w.frozen.insert(user.to_string(), on);
            }
        }
        fn is_frozen(&self, user: &str) -> Option<bool> {
            let w = self.w();
            w.logged_in
                .contains(user)
                .then(|| w.frozen.get(user).copied().unwrap_or(false))
        }
        fn logged_in(&self, user: &str) -> bool {
            self.w().logged_in.contains(user)
        }
        fn login_users(&self) -> Vec<String> {
            let mut u: Vec<String> = self.w().logged_in.iter().cloned().collect();
            u.sort();
            u
        }
        fn sessions(&self) -> Vec<Session> {
            self.w().sessions.clone()
        }
        fn active_vt(&self) -> Option<u32> {
            Some(self.w().vt)
        }
        fn switch_to(&self, vt: u32) -> bool {
            let mut w = self.w();
            w.log.push(format!("switch {vt}"));
            w.vt = vt;
            true
        }
        fn switch_lock(&self, on: bool) {
            self.w().log.push(format!("switchlock {on}"));
        }
        fn gui_available(&self) -> bool {
            self.w().gui
        }
        fn start_gui(&self, vt: u32) -> bool {
            {
                let mut w = self.w();
                w.log.push(format!("start gui {vt}"));
                w.gui_running = true;
            }
            self.hello();
            true
        }
        fn stop_gui(&self, vt: u32) {
            let mut w = self.w();
            if w.gui_running {
                w.log.push(format!("stop gui {vt}"));
            }
            w.gui_running = false;
        }
        fn gui_running(&self, _vt: u32) -> bool {
            self.w().gui_running
        }
        fn start_text(&self, vt: u32) -> bool {
            {
                let mut w = self.w();
                w.log.push(format!("start text {vt}"));
                w.text_running = true;
            }
            mark_seen(&self.shared);
            true
        }
        fn stop_text(&self) {
            let mut w = self.w();
            if w.text_running {
                w.log.push("stop text".into());
            }
            w.text_running = false;
        }
        fn text_running(&self) -> bool {
            self.w().text_running
        }
        fn tell_ttys(&self, user: &str, msg: &str) {
            self.w().told.push((user.into(), msg.into()));
        }
    }

    pub fn session(id: &str, user: &str, vt: u32, active: bool) -> Session {
        Session {
            id: id.into(),
            user: user.into(),
            seat: "seat0".into(),
            vt: Some(vt),
            active,
            graphical: true,
            tty: format!("tty{vt}"),
            class: "user".into(),
            state: if active { "active" } else { "online" }.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn face() -> Face {
        Face::waiting()
    }

    #[test]
    fn parses_loginctl_sessions() {
        let out = "Id=2\nName=mia\nSeat=seat0\nVTNr=2\nActive=yes\nType=wayland\nTTY=tty2\nClass=user\nState=active\n\n\
                   Id=c1\nName=ost-lock\nSeat=seat0\nVTNr=13\nActive=no\nType=wayland\nTTY=tty13\nClass=user\nState=online\n\n\
                   Id=5\nName=mia\nSeat=\nVTNr=0\nActive=yes\nType=tty\nTTY=pts/1\nClass=user\nState=active\n";
        let s = parse_sessions(out);
        assert_eq!(s.len(), 3);
        assert_eq!(on_screen_user(&s).as_deref(), Some("mia"));
        assert_eq!(session_vt(&s, "mia"), Some(2));
        assert!(has_graphical_session(&s, "mia"));
        assert_eq!(
            ttys_of(&s, "mia"),
            vec!["pts/1".to_string(), "tty2".to_string()]
        );
        // The lock's own session is never "the person on screen".
        let only_lock: Vec<Session> = s.iter().filter(|x| x.user == LOCK_USER).cloned().collect();
        let mut active_lock = only_lock.clone();
        active_lock[0].active = true;
        assert_eq!(on_screen_user(&active_lock), None);
    }

    #[tokio::test]
    async fn graphical_lock_switches_while_the_desktop_is_alive_and_releases_back() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        {
            let mut w = host.w();
            w.gui = true;
            w.vt = 2;
            w.sessions = vec![session("2", "mia", 2, true)];
        }
        let mut lock = LockScreen::new(Box::new(host.clone()), sh.clone(), None);
        assert!(lock.present("mia", face()).await);
        assert_eq!(lock.shown().unwrap().mode, Mode::Gui);
        assert_eq!(host.w().vt, LOCK_VT);
        lock.release();
        let log = host.w().log.clone();
        assert_eq!(
            log,
            vec![
                "start gui 13".to_string(),
                "switch 13".into(),
                "switch 13".into(),
                "switch 2".into(),
                "stop gui 13".into(),
            ]
        );
        assert!(current_face(&sh).is_none());
    }

    #[tokio::test]
    async fn text_lock_takes_over_when_cage_never_answers() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        {
            let mut w = host.w();
            w.gui = true;
            w.ui_answers = false; // cage starts but the UI never says hello
            w.vt = 2;
        }
        let mut lock = LockScreen::new(Box::new(host.clone()), sh, None);
        tokio::time::pause();
        assert!(lock.present("mia", face()).await);
        assert_eq!(lock.shown().unwrap().mode, Mode::Text);
        let log = host.w().log.clone();
        assert!(log.contains(&"stop gui 13".to_string()));
        assert!(log.contains(&"start text 13".to_string()));
        assert!(log.contains(&"switchlock true".to_string()));
        // Releasing the text lock unlocks switching before switching back.
        lock.release();
        let log = host.w().log.clone();
        let unlock = log.iter().position(|l| l == "switchlock false").unwrap();
        let back = log.iter().position(|l| l == "switch 2").unwrap();
        assert!(unlock < back);
    }

    #[tokio::test]
    async fn a_lock_from_another_boot_is_not_adopted() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        let stale = Shown {
            subject: "mia".into(),
            vt: LOCK_VT,
            return_vt: Some(2),
            mode: Mode::Text,
            boot_id: "some-other-boot".into(),
        };
        let lock = LockScreen::new(Box::new(host.clone()), sh.clone(), Some(stale.clone()));
        assert!(lock.shown().is_none());
        let same_boot = Shown {
            boot_id: boot_id(),
            ..stale
        };
        let lock = LockScreen::new(Box::new(host), sh, Some(same_boot));
        assert_eq!(lock.subject(), Some("mia"));
    }

    #[tokio::test]
    async fn reassert_brings_the_lock_back_on_screen() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        let mut lock = LockScreen::new(Box::new(host.clone()), sh, None);
        assert!(lock.present("mia", face()).await);
        host.w().vt = 3; // something switched away
        host.w().log.clear();
        assert!(lock.reassert().await);
        assert_eq!(host.w().vt, LOCK_VT);
        // The text lock (no GUI here) holds the switch lock around its own switch.
        assert_eq!(
            host.w().log,
            vec![
                "switchlock false".to_string(),
                "switch 13".into(),
                "switchlock true".into()
            ]
        );
    }
}
