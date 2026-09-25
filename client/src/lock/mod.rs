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
//! * **Text lock** (no cage, a headless build, or cage gave up or is slow):
//!   the agent itself draws a plain text lock on the VT next door
//!   ([`TEXT_VT`]) and locks VT switching (`VT_LOCKSWITCH`, what `vlock -a`
//!   does). A slow cage that comes up later takes over from it.
//!
//! On a shared computer the lock stands in front of the stopped person only:
//! "Switch user" steps aside for the display manager's login screen, the
//! stopped person stays frozen behind it, and their session coming back on
//! screen meets the lock first ([`placement`]).
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

/// The graphical lock's VT. High enough to stay clear of display managers
/// (tty1), user sessions and logind's autovt gettys (tty1–6), and journald's
/// traditional console (tty12).
pub const LOCK_VT: u32 = 13;
/// The text lock's VT: its own, next door. The graphical lock's unit resets,
/// hangs up and deallocates VT 13 every time cage starts or stops, so a text
/// lock sharing it was wiped (a black screen) for as long as cage kept
/// failing. Next door, it can be on screen at once while cage is still
/// trying.
pub const TEXT_VT: u32 = 14;
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

/// How long the screen may stay black while cage starts before the text lock
/// is shown instead. A cage that works says hello well within it; one that
/// can't drive the GPU gives up sooner still (and is noticed at once).
const GUI_FIRST: Duration = Duration::from_millis(2500);
/// How long a slow cage keeps trying in the background, behind the text lock,
/// before it is given up on. If it says hello in time, the lock moves to it.
const GUI_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the in-process text lock gets to draw.
const TEXT_TIMEOUT: Duration = Duration::from_secs(3);
/// A graphical lock UI polls every second or two; this long without a word
/// means it hung, and the text lock takes over.
const UI_STALE: Duration = Duration::from_secs(20);
/// After "Switch user", and after someone else last had the screen: how long
/// a VT with nobody on it (a login screen still starting, a session just
/// ended) is left alone before the lock comes back.
const SWITCH_GRACE: Duration = Duration::from_secs(20);
/// How long a login screen keeps the screen before the stopped person's
/// lock comes back (see [`Grace::login_screen`]).
const LOGIN_SCREEN_GRACE: Duration = Duration::from_secs(90);
/// After the lock hands the screen back: how long a desktop that locks
/// itself on waking is unlocked again (it was open when the lock went up).
const RETURN_WATCH: Duration = Duration::from_secs(6);

pub fn unit_name(vt: u32) -> String {
    format!("openscreentime-lock@{vt}.service")
}

// ── What the lock says ────────────────────────────────────────────────────────

/// Which ring the lock draws (brand board 05a). Every stop is the day's ring
/// completed — red, with "0 min left" inside — except a parent's pause, which
/// is the neutral dashed ring: someone paused it, nothing ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    /// Time's up, or a whole-device stop.
    Wall,
    /// Bedtime / outside allowed hours (drawn as `Wall`; kept so the text
    /// lock and an older lock UI can still tell a clock stop apart).
    Night,
    /// A parent paused it: the dashed ring.
    Paused,
}

/// Why the screen is stopped, as far as the words go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The day's time is used. `minutes` is the day's time (limit + earned),
    /// `used` what was used today (more, when a parent's time ran past it);
    /// `back` when screens come back ("tomorrow at 07:00").
    Limit {
        minutes: u32,
        used: u32,
        back: Option<String>,
    },
    Bedtime {
        until: Option<String>,
    },
    OutsideHours {
        until: Option<String>,
    },
    /// A parent paused this computer (now, or on a schedule).
    Paused,
    /// OpenScreenTime was changed without a parent's code.
    Tamper,
    /// Out of touch with the family server for too long.
    Offline,
}

/// "90 minutes", "1 hour", "3 hours" — a day's time the way a person says it.
pub fn span_words(minutes: u32) -> String {
    match minutes {
        1 => "1 minute".into(),
        60 => "1 hour".into(),
        m if m >= 120 && m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{m} minutes"),
    }
}

/// The lock's ring, title and second line for a stop (brand board 05a and
/// the voice in 06). `self_set`: the person set these limits themselves (an
/// adult, or anyone self-managed) — their own limit, never a parent's.
pub fn stop_words(stop: &Stop, self_set: bool) -> (Look, String, String) {
    let until = |t: &Option<String>, what: &str| match t {
        Some(t) => format!("{what} until {t}"),
        None => what.to_string(),
    };
    match stop {
        Stop::Limit {
            minutes,
            used,
            back,
        } => {
            // "All 5 minutes" only when that is what was used: time a parent
            // gave on top (a code's 30 minutes running past midnight) makes
            // it more, and the lock says so. A save-your-work countdown's
            // minute or two past the limit is still "all of it".
            let over = *used > minutes + 2;
            let used = match (self_set, over) {
                (true, false) => format!(
                    "You've used the {} you set for today.",
                    span_words(*minutes)
                ),
                (true, true) => format!(
                    "You've used {} — you set {} for today.",
                    span_words(*used),
                    span_words(*minutes)
                ),
                (false, false) => format!("You used all {}.", span_words(*minutes)),
                (false, true) => format!(
                    "You used {}; today's time was {}.",
                    span_words(*used),
                    span_words(*minutes)
                ),
            };
            let detail = match back {
                Some(b) => format!("{used} Screens come back {b}."),
                None => used,
            };
            (Look::Wall, "Time's up for today".into(), detail)
        }
        Stop::Bedtime { until: t } => (
            Look::Night,
            until(t, "Bedtime"),
            if self_set {
                "Screens are off until morning — the bedtime you set.".into()
            } else {
                "Screens are off until morning.".into()
            },
        ),
        Stop::OutsideHours { until: t } => (
            Look::Night,
            until(t, "Outside allowed hours"),
            if self_set {
                "Screens are off at this time of day — the hours you set.".into()
            } else {
                "Screens are off at this time of day.".into()
            },
        ),
        Stop::Paused => (
            Look::Paused,
            "Paused by a parent".into(),
            "It comes back when they lift the pause.".into(),
        ),
        Stop::Tamper => (
            Look::Wall,
            "Stopped until a parent checks this computer".into(),
            "OpenScreenTime was changed without a parent's code.".into(),
        ),
        Stop::Offline => (
            Look::Wall,
            "Stopped until this computer reaches the family server".into(),
            "It hasn't been in touch for days.".into(),
        ),
    }
}

// ── The self-set escape hatch ────────────────────────────────────────────────

/// Minutes "Give me 15 more minutes" buys.
pub const SNOOZE_MINUTES: u32 = 15;
/// How often a day.
pub const SNOOZES_PER_DAY: u32 = 3;
/// How long the lock has to have been up before it can be used — long enough
/// to be a decision, not a reflex.
pub const SNOOZE_WAIT_SECS: u64 = 60;

/// Someone who set their own limits (an adult, or anyone self-managed) can
/// give themselves a little more — after a visible wait, a few times a day.
/// Never offered to anyone a parent sets limits for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Snooze {
    #[default]
    Hidden,
    /// Offered, usable in `secs`.
    Wait { secs: u64 },
    /// Usable now; `left` of today's remain after this one.
    Ready { left: u32 },
    /// Today's are used; `back` when screens come back, if known.
    UsedUp {
        #[serde(default)]
        back: Option<String>,
    },
}

/// Why a snooze was refused. Checked by the agent — the lock UI only asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozeRefusal {
    /// A parent sets this person's limits.
    NotSelfSet,
    /// A pause, a tamper stop or an offline stop: not theirs to skip.
    NotTheirStop,
    TooSoon,
    UsedUp,
}

/// May `user` give themselves more time now? `waited`: seconds the lock has
/// been up; `used`: snoozes already taken today.
pub fn snooze_check(
    self_set: bool,
    own_rules_stop: bool,
    waited: u64,
    used: u32,
) -> Result<(), SnoozeRefusal> {
    if !self_set {
        Err(SnoozeRefusal::NotSelfSet)
    } else if !own_rules_stop {
        Err(SnoozeRefusal::NotTheirStop)
    } else if used >= SNOOZES_PER_DAY {
        Err(SnoozeRefusal::UsedUp)
    } else if waited < SNOOZE_WAIT_SECS {
        Err(SnoozeRefusal::TooSoon)
    } else {
        Ok(())
    }
}

/// The snooze state a face shows, from the same inputs as [`snooze_check`].
pub fn snooze_state(
    self_set: bool,
    own_rules_stop: bool,
    waited: u64,
    used: u32,
    back: Option<String>,
) -> Snooze {
    match snooze_check(self_set, own_rules_stop, waited, used) {
        Err(SnoozeRefusal::NotSelfSet | SnoozeRefusal::NotTheirStop) => Snooze::Hidden,
        Err(SnoozeRefusal::UsedUp) => Snooze::UsedUp { back },
        Err(SnoozeRefusal::TooSoon) => Snooze::Wait {
            secs: SNOOZE_WAIT_SECS - waited,
        },
        Ok(()) => Snooze::Ready {
            left: SNOOZES_PER_DAY - used - 1,
        },
    }
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
    /// The self-set escape hatch ("Give me 15 more minutes").
    #[serde(default)]
    pub snooze: Snooze,
    pub help: String,
    /// Under the code field.
    #[serde(default = "code_hint")]
    pub code_hint: String,
    /// A shared computer: offer "Switch user" (the login screen), so one
    /// person's stop doesn't stop everyone. Only with a display manager.
    #[serde(default)]
    pub switch_user: bool,
}

fn code_hint() -> String {
    CODE_HINT.into()
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
            snooze: Snooze::Hidden,
            help: HELP.into(),
            code_hint: CODE_HINT.into(),
            switch_user: false,
        }
    }
}

/// "Switch user" on the graphical lock. Quiet: the stopped person's own way
/// out comes first; this is for whoever else wants the computer.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub const SWITCH_USER: &str = "Switch user";

/// How the way out reads: (help line, code hint). Someone who sets their
/// own limits has no parent in it (board 05f) — the code is theirs.
pub fn way_out(self_set: bool, has_code: bool) -> (&'static str, &'static str) {
    match (self_set, has_code) {
        (false, true) => (HELP, CODE_HINT),
        (false, false) => (HELP_NO_CODE, CODE_HINT),
        (true, true) => (HELP_SELF, CODE_HINT_SELF),
        (true, false) => (HELP_SELF_NO_CODE, CODE_HINT_SELF),
    }
}

pub const HELP: &str = "A parent can also unlock this computer from their console.";
pub const HELP_NO_CODE: &str =
    "There's no unlock code on this computer yet — a parent can unlock it from their console.";
/// Under the code field (board 05a).
pub const CODE_HINT: &str = "A parent's code from their console, or one of the recovery codes.";
pub const HELP_SELF: &str = "You can also unlock this computer from your console.";
pub const HELP_SELF_NO_CODE: &str =
    "There's no unlock code on this computer yet — you can unlock it from your console.";
pub const CODE_HINT_SELF: &str = "The code from your console, or one of your recovery codes.";
/// A wrong code (board 06): one line, then it settles.
pub const WRONG_CODE: &str = "That's not the code — try again.";

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
    /// Last time the graphical lock spoke (its socket requests only).
    pub gui_seen: Option<Instant>,
    /// The text lock is up and cage is still starting behind it: the
    /// graphical lock's first word wakes the runner to move the lock there.
    pub wake_on_gui: bool,
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

/// The graphical lock spoke. Returns whether the runner asked to be woken
/// for it (once).
pub fn mark_gui_seen(s: &SharedRef) -> bool {
    with_shared(s, |sh| {
        let now = Instant::now();
        sh.ui_seen = Some(now);
        sh.gui_seen = Some(now);
        std::mem::take(&mut sh.wake_on_gui)
    })
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
    /// The graphical lock said its first word while the text lock stood in
    /// for it: move the lock there.
    GuiUp,
}

pub type LockTx = mpsc::Sender<LockEvent>;

/// Watch the active VT and wake the runner the moment it changes, so a person
/// who switches (or logs in) to a stopped session meets the lock at once, not
/// a frozen desktop for up to a tick. The kernel notifies a change of
/// `/sys/class/tty/tty0/active` (`POLLPRI`, what logind itself waits on);
/// the 300 ms timeout is only a fallback.
pub fn spawn_vt_watch(tx: LockTx) {
    std::thread::spawn(move || {
        let mut last = vt::active();
        loop {
            vt::wait_change(Duration::from_millis(300));
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
    /// The desktop's own screen lock is up (logind's `LockedHint`).
    pub locked: bool,
    /// When it started, on `CLOCK_MONOTONIC` (logind's `TimestampMonotonic`).
    pub since: Option<Duration>,
}

impl Session {
    /// How long it has been up.
    pub fn age(&self) -> Option<Duration> {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: clock_gettime writes into the timespec we own.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
            return None;
        }
        let now = Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32);
        now.checked_sub(self.since?)
    }
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
                    "LockedHint" => s.locked = v == "yes",
                    "TimestampMonotonic" => {
                        s.since = v
                            .parse::<u64>()
                            .ok()
                            .filter(|us| *us > 0)
                            .map(Duration::from_micros)
                    }
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
    desktop_session(sessions, user).and_then(|s| s.vt)
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

/// A person's own session (not the lock's, not a login screen).
fn is_person(s: &Session) -> bool {
    s.class.starts_with("user") && s.user != LOCK_USER && !s.user.is_empty()
}

/// A display manager's login screen (or a lock screen of its own), never ours.
fn is_login_screen(s: &Session) -> bool {
    matches!(s.class.as_str(), "greeter" | "lock-screen") && s.user != LOCK_USER
}

/// The display manager's login screen on seat0, if one is running — the
/// cheapest way to another person: just switch to it.
pub fn login_screen(sessions: &[Session]) -> Option<&Session> {
    sessions
        .iter()
        .find(|s| s.seat == "seat0" && s.state != "closing" && s.vt.is_some() && is_login_screen(s))
}

/// `user`'s graphical seat0 session — the one the lock stands in front of.
pub fn desktop_session<'a>(sessions: &'a [Session], user: &str) -> Option<&'a Session> {
    let mut mine: Vec<&Session> = sessions
        .iter()
        .filter(|s| s.user == user && s.seat == "seat0" && s.state != "closing" && s.vt.is_some())
        .collect();
    mine.sort_by_key(|s| !s.graphical);
    mine.first().copied()
}

/// Where the lock belongs right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// It is on screen: keep it there.
    Hold,
    /// Put it (back) on screen, in front of this person: a stopped session is
    /// on screen, or nothing is and nobody stepped aside for a login screen.
    Front(String),
    /// Someone else has the screen — a login screen, or another person's
    /// session. The lock waits on its VT; whoever it stopped stays frozen
    /// behind it, and comes back to the lock, not their desktop.
    Aside,
}

/// How long the lock leaves the screen to someone lately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Grace {
    /// Someone asked for the login screen, or another person had the screen
    /// a moment ago: a VT with nobody on it yet is a login screen on its
    /// way (or one coming back after a log-out), not a way around the lock.
    pub empty_vt: bool,
    /// The login screen has been up only a little while: someone may be
    /// signing in. Past that, the stopped person's lock (which offers
    /// "Switch user" again) is the better resting screen — GDM can't take
    /// anyone back into a frozen session from its login screen (it
    /// re-authenticates through processes inside it).
    pub login_screen: bool,
}

/// The lock on a shared computer. It stands in front of a stopped person —
/// nobody else. `frozen`: who is stopped.
pub fn placement(
    sessions: &[Session],
    active_vt: Option<u32>,
    lock_vt: u32,
    subject: &str,
    frozen: &dyn Fn(&str) -> bool,
    grace: Grace,
) -> Placement {
    if active_vt == Some(lock_vt) {
        return Placement::Hold;
    }
    let here: Vec<&Session> = sessions
        .iter()
        .filter(|s| {
            active_vt.is_some() && s.vt == active_vt && s.seat == "seat0" && s.state != "closing"
        })
        .collect();
    // A stopped person on screen first, whatever else shares the VT: the
    // lock goes in front of them before anything else.
    if let Some(s) = here.iter().find(|s| is_person(s) && frozen(&s.user)) {
        return Placement::Front(s.user.clone());
    }
    if here.iter().any(|s| s.user == LOCK_USER) {
        // The lock's other VT (a graphical lock still starting): ours.
        return Placement::Front(subject.to_string());
    }
    let aside_if = |ok: bool| {
        if ok {
            Placement::Aside
        } else {
            Placement::Front(subject.to_string())
        }
    };
    if here.iter().any(|s| is_person(s)) {
        return Placement::Aside;
    }
    if here.iter().any(|s| is_login_screen(s)) {
        return aside_if(grace.login_screen);
    }
    aside_if(grace.empty_vt)
}

/// `systemctl show -p ActiveState -p SubState -p Result <lock unit>`: has
/// cage given up (exited and waiting to restart, or failed)? A unit still
/// queued to start is not "given up".
pub fn unit_gave_up(show: &str) -> bool {
    let get = |k: &str| {
        show.lines()
            .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
            .map(str::trim)
            .unwrap_or("")
    };
    let result = get("Result");
    get("ActiveState") == "failed"
        || get("SubState") == "auto-restart"
        || (!result.is_empty() && result != "success")
}

/// The display manager's seats from `busctl get-property … Seats`
/// (`ao 1 "/org/freedesktop/DisplayManager/Seat0"`); LightDM's and SDDM's
/// usual seat0 paths when it said nothing.
pub fn dm_seat_paths(seats: &str) -> Vec<String> {
    let found: Vec<String> = seats
        .split_whitespace()
        .map(|w| w.trim_matches('"'))
        .filter(|w| w.starts_with("/org/freedesktop/DisplayManager/"))
        .map(str::to_string)
        .collect();
    if found.is_empty() {
        vec![
            "/org/freedesktop/DisplayManager/Seat0".into(),
            "/org/freedesktop/DisplayManager/seat0".into(),
        ]
    } else {
        found
    }
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
    /// Freeze `user`'s apps (see `enforce::screentime::freeze`), or thaw
    /// everything of theirs.
    fn freeze(&self, user: &str, on: bool, hard: bool);
    /// Anything of theirs frozen right now (a stop, or what's left of one).
    fn is_frozen(&self, user: &str) -> Option<bool>;
    /// Keep a stop holding, quietly: freeze what has appeared since (an app
    /// a timer started, a login), never escalating. Returns whether the stop
    /// holds now (`None`: no slice).
    fn refreeze(&self, user: &str) -> Option<bool>;
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
    /// The graphical lock's unit gave up (cage exited or failed).
    fn gui_failed(&self, vt: u32) -> bool;
    /// Say something on `user`'s own terminals (only when they have no desktop).
    fn tell_ttys(&self, user: &str, msg: &str);
    /// Is there a login screen to switch to — a display manager, and more
    /// than one person who could sign in?
    fn can_switch_user(&self) -> bool;
    /// Bring up the display manager's login screen ("Switch user"): switch to
    /// the one that is running, or ask the display manager for one.
    fn login_screen(&self) -> bool;
    /// Take the desktop's own screen lock off a session (logind `Unlock`).
    fn unlock_session(&self, id: &str);
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

/// Every logind session on the machine (`loginctl show-session`).
pub fn query_sessions(exec: &crate::util::Exec) -> Vec<Session> {
    let list = exec.probe("loginctl", &["list-sessions", "--no-legend"]);
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
        "Id",
        "Name",
        "Seat",
        "VTNr",
        "Active",
        "Type",
        "TTY",
        "Class",
        "State",
        "LockedHint",
        "TimestampMonotonic",
    ] {
        args.extend(["-p", p]);
    }
    parse_sessions(&exec.probe("loginctl", &args))
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
    fn refreeze(&self, user: &str) -> Option<bool> {
        crate::enforce::screentime::refreeze(&self.exec, user)
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
        query_sessions(&self.exec)
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
    fn gui_failed(&self, vt: u32) -> bool {
        if self.exec.dry_run() {
            return false;
        }
        let unit = unit_name(vt);
        let show = self.exec.probe(
            "systemctl",
            &[
                "show",
                "-p",
                "ActiveState",
                "-p",
                "SubState",
                "-p",
                "Result",
                &unit,
            ],
        );
        unit_gave_up(&show)
    }
    fn can_switch_user(&self) -> bool {
        // `systemctl enable gdm|lightdm|sddm` makes this alias.
        let dm = std::path::Path::new("/etc/systemd/system/display-manager.service").exists();
        dm && self.login_users().len() > 1
    }
    fn login_screen(&self) -> bool {
        if let Some(g) = login_screen(&self.sessions()) {
            tracing::info!("switch user: to the login screen (session {})", g.id);
            if self.exec.run("loginctl", &["activate", &g.id]).is_ok() {
                return true;
            }
        }
        // GDM: a new login screen on a VT of its own.
        let gdm = self.exec.run(
            "busctl",
            &[
                "--system",
                "--timeout=5",
                "call",
                "org.gnome.DisplayManager",
                "/org/gnome/DisplayManager/LocalDisplayFactory",
                "org.gnome.DisplayManager.LocalDisplayFactory",
                "CreateTransientDisplay",
            ],
        );
        if gdm.is_ok() {
            tracing::info!("switch user: GDM opened a login screen");
            return true;
        }
        // LightDM and SDDM: the freedesktop DisplayManager seat's
        // SwitchToGreeter (what `dm-tool switch-to-greeter` calls).
        let seats = self.exec.probe(
            "busctl",
            &[
                "--system",
                "--timeout=5",
                "get-property",
                "org.freedesktop.DisplayManager",
                "/org/freedesktop/DisplayManager",
                "org.freedesktop.DisplayManager",
                "Seats",
            ],
        );
        for p in dm_seat_paths(&seats) {
            let ok = self
                .exec
                .run(
                    "busctl",
                    &[
                        "--system",
                        "--timeout=5",
                        "call",
                        "org.freedesktop.DisplayManager",
                        &p,
                        "org.freedesktop.DisplayManager.Seat",
                        "SwitchToGreeter",
                    ],
                )
                .is_ok();
            if ok {
                tracing::info!("switch user: the display manager opened a login screen ({p})");
                return true;
            }
        }
        tracing::warn!("switch user: no login screen, and no display manager answered");
        false
    }
    fn unlock_session(&self, id: &str) {
        if self.exec.dry_run() {
            tracing::info!(target: "dry_run", "WOULD UNLOCK session {id} if its desktop locks itself");
            return;
        }
        // The desktop may lock itself as it wakes (it saw its screen taken),
        // a moment after it is back on screen: watch that moment, not just
        // this instant. Only `LockedHint` going up is answered, so a desktop
        // that stays open is never touched.
        let exec = self.exec.clone();
        let id = id.to_string();
        std::thread::spawn(move || {
            let until = Instant::now() + RETURN_WATCH;
            let mut unlocked = 0;
            while Instant::now() < until && unlocked < 3 {
                let hint = exec.probe(
                    "loginctl",
                    &["show-session", &id, "-p", "LockedHint", "--value"],
                );
                if hint.trim() == "yes" {
                    match exec.run("loginctl", &["unlock-session", &id]) {
                        Ok(_) => tracing::info!("took the desktop's own lock off session {id}"),
                        Err(e) => tracing::warn!("could not unlock session {id}: {e}"),
                    }
                    unlocked += 1;
                    // The desktop takes a moment to lower its hint.
                    std::thread::sleep(Duration::from_millis(1500));
                    continue;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
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

impl Mode {
    /// Each lock has its own VT (see [`TEXT_VT`]).
    pub fn vt(self) -> u32 {
        match self {
            Mode::Gui => LOCK_VT,
            Mode::Text => TEXT_VT,
        }
    }
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
    /// Whether the subject's own desktop lock was already up when ours went
    /// up (`None`: not known). Only a desktop that was open is opened again
    /// on the way back — never one its owner had locked.
    #[serde(default)]
    pub desktop_locked: Option<bool>,
}

/// How a graphical lock's start went.
enum GuiStart {
    /// On screen and answering.
    Up,
    /// Still starting after [`GUI_FIRST`]: the text lock goes up meanwhile.
    Slow(Instant),
    Failed,
}

pub struct LockScreen {
    host: Box<dyn Host>,
    shared: SharedRef,
    shown: Option<Shown>,
    /// The text lock is up while cage keeps starting behind it (since when).
    upgrade: Option<Instant>,
    /// Someone else has the screen; the lock waits on its VT.
    aside: bool,
    /// Until then, a VT with nobody on it is a login screen on its way.
    grace_until: Option<Instant>,
    /// Since when a login screen has had the screen.
    login_screen_since: Option<Instant>,
    /// What this process last set `VT_LOCKSWITCH` to (`None`: not yet — a
    /// previous run may have left it either way).
    switch_locked: Option<bool>,
}

impl LockScreen {
    pub fn new(host: Box<dyn Host>, shared: SharedRef, persisted: Option<Shown>) -> Self {
        let shown = persisted.filter(|s| s.boot_id == boot_id());
        if shown.is_some() {
            // Adopted from the previous run: give its UI the benefit of the
            // doubt until it has had time to reconnect.
            mark_seen(&shared);
            with_shared(&shared, |sh| sh.gui_seen = Some(Instant::now()));
        }
        LockScreen {
            host,
            shared,
            shown,
            upgrade: None,
            aside: false,
            grace_until: None,
            login_screen_since: None,
            switch_locked: None,
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

    /// Someone else has the screen and the lock is waiting on its VT.
    #[cfg(test)]
    pub fn is_aside(&self) -> bool {
        self.shown.is_some() && self.aside
    }

    /// Publish what the lock says (both locks read it).
    pub fn publish(&self, face: Face) {
        with_shared(&self.shared, |s| s.face = Some(face));
    }

    fn ui_seen_since(&self, t: Instant) -> bool {
        with_shared(&self.shared, |s| s.ui_seen.is_some_and(|x| x >= t))
    }

    fn gui_seen_since(&self, t: Instant) -> bool {
        with_shared(&self.shared, |s| s.gui_seen.is_some_and(|x| x >= t))
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

    fn in_grace(&self) -> bool {
        self.grace_until.is_some_and(|t| Instant::now() < t)
    }

    fn set_switch_lock(&mut self, on: bool) {
        if self.switch_locked != Some(on) {
            self.host.switch_lock(on);
            self.switch_locked = Some(on);
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        if let Some(s) = self.shown.as_mut() {
            s.mode = mode;
            s.vt = mode.vt();
        }
    }

    fn wake_on_gui(&self, on: bool) {
        with_shared(&self.shared, |sh| sh.wake_on_gui = on);
    }

    /// Whether `subject`'s own desktop lock is up right now.
    fn desktop_locked(&self, subject: &str) -> Option<bool> {
        desktop_session(&self.host.sessions(), subject).map(|s| s.locked)
    }

    /// Stand in front of someone else who is stopped (they switched to their
    /// session while the lock was up for another person).
    pub fn retarget(&mut self, subject: &str) {
        if self.subject() == Some(subject) {
            return;
        }
        let locked = self.desktop_locked(subject);
        if let Some(s) = self.shown.as_mut() {
            s.subject = subject.to_string();
            s.desktop_locked = locked;
        }
    }

    /// Put the lock in front of `subject`. Returns once a lock is on screen
    /// and drawing — only then may the caller freeze. `false` means nothing
    /// could be shown; the caller must not freeze (a frozen desktop with
    /// nothing on it is a brick, not a lock).
    ///
    /// With cage, its lock gets [`GUI_FIRST`] to come up; one that gives up
    /// sooner, or needs longer, has the text lock on screen meanwhile (on its
    /// own VT) — never a black screen for long — and a slow one takes over
    /// when it says hello.
    pub async fn present(&mut self, subject: &str, face: Face) -> bool {
        self.publish(face);
        if self.shown.is_some() {
            self.retarget(subject);
            self.grace_until = None;
            return self.bring_to_front().await;
        }
        let return_vt = self
            .host
            .active_vt()
            .filter(|v| *v != LOCK_VT && *v != TEXT_VT);
        let desktop_locked = self.desktop_locked(subject);
        let gui = self.host.gui_available();
        let mode = if gui { Mode::Gui } else { Mode::Text };
        self.shown = Some(Shown {
            subject: subject.to_string(),
            vt: mode.vt(),
            return_vt,
            mode,
            boot_id: boot_id(),
            desktop_locked,
        });
        self.aside = false;
        self.grace_until = None;
        self.upgrade = None;
        if gui {
            match self.bring_up_gui().await {
                GuiStart::Up => return true,
                GuiStart::Slow(since) => {
                    tracing::warn!(
                        "the graphical lock is slow to come up; the text lock shows meanwhile"
                    );
                    self.upgrade = Some(since);
                    self.wake_on_gui(true);
                }
                GuiStart::Failed => {
                    tracing::warn!("the graphical lock did not come up; the text lock takes over");
                    self.host.stop_gui(LOCK_VT);
                }
            }
            self.set_mode(Mode::Text);
        }
        if self.bring_up_text().await {
            return true;
        }
        if let Some(since) = self.upgrade {
            // No text lock either (no VT to draw on?): cage is the last hope.
            if self.wait_gui(since, GUI_TIMEOUT).await {
                self.upgrade = None;
                self.wake_on_gui(false);
                self.set_mode(Mode::Gui);
                self.set_switch_lock(false);
                if self.host.switch_to(LOCK_VT) {
                    return true;
                }
            }
        }
        tracing::error!("no lock could be shown");
        self.release();
        false
    }

    async fn wait_gui(&self, since: Instant, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if self.gui_seen_since(since) {
                return true;
            }
            if self.host.gui_failed(LOCK_VT) {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        self.gui_seen_since(since)
    }

    async fn bring_up_gui(&mut self) -> GuiStart {
        let since = Instant::now();
        if !self.host.start_gui(LOCK_VT) {
            return GuiStart::Failed;
        }
        // Switch while the person's compositor is still alive: it gets to hand
        // over the display and input cleanly. The lock draws a moment later.
        self.set_switch_lock(false);
        let _ = self.host.switch_to(LOCK_VT);
        let deadline = tokio::time::Instant::now() + GUI_FIRST;
        loop {
            if self.gui_seen_since(since) {
                return if self.host.switch_to(LOCK_VT) {
                    GuiStart::Up
                } else {
                    GuiStart::Failed
                };
            }
            if self.host.gui_failed(LOCK_VT) {
                return GuiStart::Failed;
            }
            if tokio::time::Instant::now() >= deadline {
                return GuiStart::Slow(since);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    async fn bring_up_text(&mut self) -> bool {
        let since = Instant::now();
        if !self.host.start_text(TEXT_VT) {
            return false;
        }
        self.set_switch_lock(false);
        if !self.host.switch_to(TEXT_VT) {
            self.host.stop_text();
            return false;
        }
        self.set_switch_lock(true);
        self.wait_ui(since, TEXT_TIMEOUT).await
    }

    /// A slow graphical lock said hello (or gave up) behind the text lock.
    async fn check_upgrade(&mut self) {
        let Some(since) = self.upgrade else {
            return;
        };
        if self.gui_seen_since(since) {
            tracing::info!("the graphical lock is up; it takes over from the text lock");
            self.upgrade = None;
            self.wake_on_gui(false);
            let on_screen = self.host.active_vt() == Some(TEXT_VT);
            self.set_mode(Mode::Gui);
            self.set_switch_lock(false);
            if on_screen && !self.host.switch_to(LOCK_VT) {
                tracing::warn!("could not switch to the graphical lock; the text lock stays");
                self.host.stop_gui(LOCK_VT);
                self.set_mode(Mode::Text);
                return;
            }
            self.host.stop_text();
        } else if self.host.gui_failed(LOCK_VT) || since.elapsed() >= GUI_TIMEOUT {
            tracing::warn!("the graphical lock never came up; the text lock stays");
            self.upgrade = None;
            self.wake_on_gui(false);
            self.host.stop_gui(LOCK_VT);
        }
    }

    /// The lock is up, drawing, and on screen (switching locked for the text
    /// lock) — whatever is on screen now.
    async fn bring_to_front(&mut self) -> bool {
        if !self.keep_alive().await {
            return false;
        }
        let Some(s) = self.shown.clone() else {
            return false;
        };
        self.aside = false;
        if self.host.active_vt() != Some(s.vt) {
            // Switching is locked for the text lock; only we switch, and only
            // to here.
            self.set_switch_lock(false);
            if !self.host.switch_to(s.vt) {
                return false;
            }
        }
        if s.mode == Mode::Text {
            self.set_switch_lock(true);
        }
        true
    }

    /// Bring the lock back if it died or hung (not where it is on screen).
    async fn keep_alive(&mut self) -> bool {
        self.check_upgrade().await;
        let Some(s) = self.shown.clone() else {
            return false;
        };
        match s.mode {
            Mode::Gui => {
                let alive = self.host.gui_running(LOCK_VT)
                    && with_shared(&self.shared, |sh| {
                        sh.gui_seen.is_some_and(|t| t.elapsed() < UI_STALE)
                    });
                if !alive {
                    tracing::warn!(
                        "the graphical lock stopped answering; the text lock takes over"
                    );
                    let on_screen = self.host.active_vt() == Some(LOCK_VT);
                    self.host.stop_gui(LOCK_VT);
                    self.set_mode(Mode::Text);
                    if on_screen {
                        return self.bring_up_text().await;
                    }
                    return self.host.start_text(TEXT_VT);
                }
                true
            }
            Mode::Text => self.host.text_running() || self.host.start_text(s.vt),
        }
    }

    /// Keep the lock where it belongs: alive, and on screen in front of
    /// whoever it stopped — or waiting on its VT while someone else has the
    /// computer (see [`placement`]). `frozen`: who is stopped. Called every
    /// tick, command, lock request and VT change while it's up. `false`: the
    /// lock can't be shown.
    pub async fn reassert(&mut self, frozen: &(dyn Fn(&str) -> bool + Sync)) -> bool {
        if !self.keep_alive().await {
            return false;
        }
        let Some(s) = self.shown.clone() else {
            return false;
        };
        let sessions = self.host.sessions();
        let active = self.host.active_vt();
        let on_screen = |what: fn(&Session) -> bool| {
            sessions.iter().any(|x| {
                x.vt.is_some()
                    && x.vt == active
                    && x.seat == "seat0"
                    && x.state != "closing"
                    && what(x)
            })
        };
        if on_screen(is_login_screen) {
            self.login_screen_since.get_or_insert_with(Instant::now);
        } else {
            self.login_screen_since = None;
        }
        let grace = Grace {
            empty_vt: self.in_grace(),
            login_screen: self
                .login_screen_since
                .is_some_and(|t| t.elapsed() < LOGIN_SCREEN_GRACE),
        };
        match placement(&sessions, active, s.vt, &s.subject, frozen, grace) {
            Placement::Hold => {
                self.aside = false;
                // Switching stays unlocked only while a login screen asked
                // for is on its way.
                if s.mode == Mode::Text && !self.in_grace() {
                    self.set_switch_lock(true);
                }
                true
            }
            Placement::Front(who) => {
                if who != s.subject {
                    self.retarget(&who);
                }
                self.grace_until = None;
                self.bring_to_front().await
            }
            Placement::Aside => {
                if !self.aside {
                    tracing::info!(
                        "someone else has the screen; the lock waits (VT {}), {} stays stopped",
                        s.vt,
                        s.subject
                    );
                }
                self.aside = true;
                if s.mode == Mode::Text {
                    self.set_switch_lock(false);
                }
                if on_screen(is_person) {
                    // A session ending (a log-out) leaves a moment with
                    // nobody on screen before the login screen is back.
                    self.grace_until = Some(Instant::now() + SWITCH_GRACE);
                }
                true
            }
        }
    }

    /// "Switch user" at the lock: step aside for the display manager's login
    /// screen. The stopped person stays frozen; switching back to them brings
    /// the lock back first.
    pub fn switch_user(&mut self) -> bool {
        let Some(s) = self.shown.clone() else {
            return false;
        };
        self.grace_until = Some(Instant::now() + SWITCH_GRACE);
        // A login screen asked for now gets its full time.
        self.login_screen_since = None;
        // The login screen switches VTs itself: let it.
        self.set_switch_lock(false);
        if self.host.login_screen() {
            return true;
        }
        self.grace_until = None;
        if s.mode == Mode::Text && self.host.active_vt() == Some(s.vt) {
            self.set_switch_lock(true);
        }
        false
    }

    /// Take the lock down. The caller has already thawed; this puts the
    /// person's own session back on screen and stops the lock, in that order.
    /// If someone else has the screen, it stays theirs.
    pub fn release(&mut self) {
        let Some(s) = self.shown.take() else {
            return;
        };
        self.upgrade = None;
        self.aside = false;
        self.grace_until = None;
        self.login_screen_since = None;
        self.wake_on_gui(false);
        self.host.stop_text();
        // Nothing but a text lock on screen may hold VT switching (and a
        // text lock of a previous run may have left it held).
        self.set_switch_lock(false);
        let active = self.host.active_vt();
        if active == Some(LOCK_VT) || active == Some(TEXT_VT) {
            let sessions = self.host.sessions();
            let back = session_vt(&sessions, &s.subject).or(s.return_vt);
            if let Some(v) = back {
                if !self.host.switch_to(v) {
                    tracing::warn!("could not switch back to VT {v}");
                }
            }
            // Their desktop saw its screen go away and may have locked
            // itself. It was open when the lock went up, so it opens again:
            // a code or a parent's time must not end at a second lock.
            if s.desktop_locked == Some(false) {
                if let Some(d) = desktop_session(&sessions, &s.subject) {
                    self.host.unlock_session(&d.id);
                }
            }
        }
        self.host.stop_gui(LOCK_VT);
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
    // The text lock lived in the agent process; its switch lock outlives it
    // (release lets go of it, knowing nothing of this process's own).
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
        /// cage gives up at once (a GPU it can't drive).
        pub gui_fails: bool,
        pub text_running: bool,
        /// `VT_LOCKSWITCH`, as the kernel keeps it: nobody switches while set.
        pub switch_locked: bool,
        /// A display manager whose login screen comes up on this VT.
        pub greeter_vt: Option<u32>,
        pub told: Vec<(String, String)>,
        /// Sessions whose desktop lock was taken off.
        pub unlocked: Vec<String>,
    }

    impl World {
        /// logind's view follows the VT: the seat0 session on it is active.
        fn follow_vt(&mut self) {
            let vt = self.vt;
            for s in &mut self.sessions {
                if s.seat == "seat0" {
                    s.active = s.vt == Some(vt);
                }
            }
        }
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
        /// Someone (not the agent) switches the screen to `vt` — a person at
        /// the greeter, or the display manager.
        pub fn user_switches_to(&self, vt: u32) -> bool {
            let mut w = self.w();
            if w.switch_locked {
                return false;
            }
            w.vt = vt;
            w.follow_vt();
            true
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
        fn refreeze(&self, user: &str) -> Option<bool> {
            let mut w = self.w();
            if !w.logged_in.contains(user) {
                return None;
            }
            // A slice that came back thawed (a re-login) is frozen again.
            if w.frozen.get(user) != Some(&true) {
                w.log.push(format!("refreeze {user}"));
                w.frozen.insert(user.to_string(), true);
            }
            Some(true)
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
            if w.vt == vt {
                return true;
            }
            if w.switch_locked {
                w.log.push(format!("switch {vt} refused"));
                return false;
            }
            w.log.push(format!("switch {vt}"));
            w.vt = vt;
            w.follow_vt();
            true
        }
        fn switch_lock(&self, on: bool) {
            let mut w = self.w();
            w.log.push(format!("switchlock {on}"));
            w.switch_locked = on;
        }
        fn gui_available(&self) -> bool {
            self.w().gui
        }
        fn start_gui(&self, vt: u32) -> bool {
            let hello = {
                let mut w = self.w();
                w.log.push(format!("start gui {vt}"));
                w.gui_running = !w.gui_fails;
                w.ui_answers && !w.gui_fails
            };
            if hello {
                mark_gui_seen(&self.shared);
            }
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
        fn gui_failed(&self, _vt: u32) -> bool {
            self.w().gui_fails
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
        fn can_switch_user(&self) -> bool {
            self.w().greeter_vt.is_some()
        }
        fn login_screen(&self) -> bool {
            let mut w = self.w();
            let Some(vt) = w.greeter_vt else {
                w.log.push("login screen: no display manager".into());
                return false;
            };
            if w.switch_locked {
                // A display manager can't switch past VT_LOCKSWITCH either.
                w.log.push("login screen refused".into());
                return false;
            }
            w.log.push(format!("login screen {vt}"));
            if !w.sessions.iter().any(|s| s.class == "greeter") {
                let mut g = session("g1", "gdm", vt, false);
                g.class = "greeter".into();
                w.sessions.push(g);
            }
            w.vt = vt;
            w.follow_vt();
            true
        }
        fn unlock_session(&self, id: &str) {
            let mut w = self.w();
            w.log.push(format!("unlock session {id}"));
            w.unlocked.push(id.to_string());
            for s in &mut w.sessions {
                if s.id == id {
                    s.locked = false;
                }
            }
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
            locked: false,
            since: None,
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
    fn every_stop_says_why_in_one_sentence() {
        let limit = Stop::Limit {
            minutes: 90,
            used: 90,
            back: Some("tomorrow at 07:00".into()),
        };
        let (look, title, detail) = stop_words(&limit, false);
        assert_eq!(look, Look::Wall);
        assert_eq!(title, "Time's up for today");
        assert_eq!(
            detail,
            "You used all 90 minutes. Screens come back tomorrow at 07:00."
        );
        // The person set it themselves: their own limit, in their words.
        let (_, _, detail) = stop_words(
            &Stop::Limit {
                minutes: 180,
                used: 180,
                back: None,
            },
            true,
        );
        assert_eq!(detail, "You've used the 3 hours you set for today.");
        // More than the day's time was used (a parent's 30 minutes ran past
        // midnight): never "all 5 minutes" when it was 10.
        let over = Stop::Limit {
            minutes: 5,
            used: 10,
            back: Some("tomorrow".into()),
        };
        let (_, title, detail) = stop_words(&over, false);
        assert_eq!(title, "Time's up for today");
        assert_eq!(
            detail,
            "You used 10 minutes; today's time was 5 minutes. Screens come back tomorrow."
        );
        let (_, _, detail) = stop_words(&over, true);
        assert!(detail.starts_with("You've used 10 minutes — you set 5 minutes for today."));

        let (look, title, _) = stop_words(
            &Stop::Bedtime {
                until: Some("07:00".into()),
            },
            false,
        );
        assert_eq!((look, title.as_str()), (Look::Night, "Bedtime until 07:00"));
        let (_, title, _) = stop_words(
            &Stop::OutsideHours {
                until: Some("15:00".into()),
            },
            false,
        );
        assert_eq!(title, "Outside allowed hours until 15:00");
        // A parent's pause is the neutral ring, never the red one.
        let (look, title, _) = stop_words(&Stop::Paused, false);
        assert_eq!((look, title.as_str()), (Look::Paused, "Paused by a parent"));
        for s in [Stop::Tamper, Stop::Offline] {
            let (look, title, detail) = stop_words(&s, false);
            assert_eq!(look, Look::Wall);
            assert!(!detail.is_empty());
            assert_ne!(title, title.to_uppercase(), "sentence case");
        }
        assert_eq!(span_words(60), "1 hour");
        assert_eq!(span_words(75), "75 minutes");
        assert_eq!(span_words(120), "2 hours");
    }

    #[test]
    fn only_the_person_who_set_the_limit_can_snooze_it() {
        use SnoozeRefusal::*;
        // A child: never, whatever else is true.
        assert_eq!(snooze_check(false, true, 600, 0), Err(NotSelfSet));
        // An adult at their own limit, after the wait: yes.
        assert_eq!(snooze_check(true, true, 60, 0), Ok(()));
        // Not before the wait, not past three, not a parent's pause.
        assert_eq!(snooze_check(true, true, 59, 0), Err(TooSoon));
        assert_eq!(snooze_check(true, true, 600, 3), Err(UsedUp));
        assert_eq!(snooze_check(true, false, 600, 0), Err(NotTheirStop));
        // What the lock shows follows the same rule.
        assert_eq!(snooze_state(false, true, 600, 0, None), Snooze::Hidden);
        assert_eq!(
            snooze_state(true, true, 20, 0, None),
            Snooze::Wait { secs: 40 }
        );
        assert_eq!(
            snooze_state(true, true, 90, 1, None),
            Snooze::Ready { left: 1 }
        );
        assert_eq!(
            snooze_state(true, true, 90, 3, Some("tomorrow at 07:00".into())),
            Snooze::UsedUp {
                back: Some("tomorrow at 07:00".into())
            }
        );
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

    fn frozen_mia(u: &str) -> bool {
        u == "mia"
    }

    /// mia's desktop on tty2, on screen; `sam` has an account too.
    fn mia_on_screen(host: &FakeHost, gui: bool) {
        let mut w = host.w();
        w.gui = gui;
        w.vt = 2;
        w.sessions = vec![session("2", "mia", 2, true)];
        w.logged_in.insert("mia".into());
    }

    #[tokio::test]
    async fn graphical_lock_switches_while_the_desktop_is_alive_and_releases_back() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        mia_on_screen(&host, true);
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
                "switchlock false".into(),
                "switch 13".into(),
                "switch 2".into(),
                // Her desktop was open when the lock went up: if it locked
                // itself meanwhile, it is opened again.
                "unlock session 2".into(),
                "stop gui 13".into(),
            ]
        );
        assert!(current_face(&sh).is_none());
    }

    #[tokio::test]
    async fn a_cage_that_cannot_start_gives_way_to_the_text_lock_at_once() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        mia_on_screen(&host, true);
        host.w().gui_fails = true; // wlroots refuses the GPU (QEMU std VGA)
        let mut lock = LockScreen::new(Box::new(host.clone()), sh, None);
        tokio::time::pause();
        let t0 = tokio::time::Instant::now();
        assert!(lock.present("mia", face()).await);
        // No 14 s — not even the 2.5 s a slow cage gets: it gave up, so the
        // text lock is on screen at once.
        assert!(
            t0.elapsed() < Duration::from_millis(500),
            "{:?}",
            t0.elapsed()
        );
        let s = lock.shown().unwrap();
        assert_eq!((s.mode, s.vt), (Mode::Text, TEXT_VT));
        let w = host.w();
        assert_eq!(w.vt, TEXT_VT);
        assert!(w.switch_locked, "the text lock holds VT switching");
        assert!(w.log.contains(&"start text 14".to_string()));
    }

    #[tokio::test]
    async fn a_slow_cage_takes_over_from_the_text_lock_when_it_says_hello() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        mia_on_screen(&host, true);
        host.w().ui_answers = false; // cage runs, the window is still coming
        let mut lock = LockScreen::new(Box::new(host.clone()), sh.clone(), None);
        tokio::time::pause();
        let t0 = tokio::time::Instant::now();
        assert!(lock.present("mia", face()).await);
        assert!(t0.elapsed() <= GUI_FIRST + Duration::from_millis(500));
        // Meanwhile: the text lock, on its own VT, holding switching.
        assert_eq!(lock.shown().unwrap().mode, Mode::Text);
        assert_eq!(host.w().vt, TEXT_VT);
        assert!(host.w().gui_running, "cage keeps starting behind it");
        // The graphical lock's first word wakes the runner (once)…
        assert!(mark_gui_seen(&sh));
        assert!(!mark_gui_seen(&sh));
        // …and the lock moves to it.
        assert!(lock.reassert(&frozen_mia).await);
        let s = lock.shown().unwrap();
        assert_eq!((s.mode, s.vt), (Mode::Gui, LOCK_VT));
        let w = host.w();
        assert_eq!(w.vt, LOCK_VT);
        assert!(!w.switch_locked, "cage keeps the keyboard itself");
        assert!(!w.text_running);
    }

    #[tokio::test]
    async fn a_cage_that_gives_up_behind_the_text_lock_is_stopped() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        mia_on_screen(&host, true);
        host.w().ui_answers = false;
        let mut lock = LockScreen::new(Box::new(host.clone()), sh, None);
        tokio::time::pause();
        assert!(lock.present("mia", face()).await);
        host.w().gui_fails = true;
        host.w().log.clear();
        assert!(lock.reassert(&frozen_mia).await);
        assert_eq!(lock.shown().unwrap().mode, Mode::Text);
        assert_eq!(host.w().log, vec!["stop gui 13".to_string()]);
        assert_eq!(host.w().vt, TEXT_VT);
    }

    #[tokio::test]
    async fn a_lock_from_another_boot_is_not_adopted() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        let stale = Shown {
            subject: "mia".into(),
            vt: TEXT_VT,
            return_vt: Some(2),
            mode: Mode::Text,
            boot_id: "some-other-boot".into(),
            desktop_locked: Some(false),
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
        mia_on_screen(&host, false);
        let mut lock = LockScreen::new(Box::new(host.clone()), sh, None);
        assert!(lock.present("mia", face()).await);
        {
            let mut w = host.w();
            w.switch_locked = false;
            w.vt = 3; // something switched away, to a VT with nobody on it
            w.log.clear();
        }
        assert!(lock.reassert(&frozen_mia).await);
        assert_eq!(host.w().vt, TEXT_VT);
        // The text lock (no GUI here) holds the switch lock around its own switch.
        assert_eq!(
            host.w().log,
            vec![
                "switchlock false".to_string(),
                "switch 14".into(),
                "switchlock true".into()
            ]
        );
    }

    /// The shared family computer: mia is stopped, sam wants the computer.
    #[tokio::test]
    async fn switch_user_steps_aside_and_her_session_meets_the_lock_again() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        mia_on_screen(&host, false);
        host.w().greeter_vt = Some(1);
        let mut lock = LockScreen::new(Box::new(host.clone()), sh, None);
        assert!(lock.present("mia", face()).await);
        assert!(host.w().switch_locked);
        // With switching locked nobody — the display manager included — gets
        // past the lock. "Switch user" lets go of it for the login screen.
        assert!(!host.user_switches_to(1));
        assert!(lock.switch_user());
        {
            let w = host.w();
            let log = &w.log;
            let released = log.iter().rposition(|l| l == "switchlock false").unwrap();
            let asked = log.iter().position(|l| l == "login screen 1").unwrap();
            assert!(released < asked);
            assert_eq!(w.vt, 1);
        }
        // The lock waits on its VT while the login screen has the screen…
        assert!(lock.reassert(&frozen_mia).await);
        assert!(lock.is_aside());
        assert_eq!(host.w().vt, 1);
        assert!(!host.w().switch_locked, "sam can switch VTs as usual");
        // …for a while: a login screen left alone gives way to her lock
        // (which offers "Switch user" again).
        lock.login_screen_since = Some(Instant::now() - LOGIN_SCREEN_GRACE);
        lock.grace_until = None;
        assert!(lock.reassert(&frozen_mia).await);
        assert!(!lock.is_aside());
        assert_eq!(host.w().vt, TEXT_VT);
        assert!(lock.switch_user());
        assert!(lock.reassert(&frozen_mia).await);
        assert!(lock.is_aside(), "a fresh login screen gets its full time");
        // …and while sam uses his own session.
        {
            let mut w = host.w();
            w.sessions.push(session("5", "sam", 3, false));
            w.logged_in.insert("sam".into());
        }
        assert!(host.user_switches_to(3));
        assert!(lock.reassert(&frozen_mia).await);
        assert_eq!(host.w().vt, 3, "sam keeps the screen");
        assert_eq!(lock.subject(), Some("mia"));
        // Someone switches to mia's (frozen) session: the lock comes first.
        assert!(host.user_switches_to(2));
        host.w().log.clear();
        assert!(lock.reassert(&frozen_mia).await);
        assert!(!lock.is_aside());
        let w = host.w();
        assert_eq!(w.vt, TEXT_VT);
        assert!(w.switch_locked);
        assert_eq!(
            w.log,
            vec!["switch 14".to_string(), "switchlock true".into(),]
        );
    }

    #[tokio::test]
    async fn switch_user_with_no_display_manager_keeps_the_lock_up() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        mia_on_screen(&host, false);
        let mut lock = LockScreen::new(Box::new(host.clone()), sh, None);
        assert!(lock.present("mia", face()).await);
        assert!(!lock.switch_user());
        let w = host.w();
        assert_eq!(w.vt, TEXT_VT);
        assert!(w.switch_locked, "switching is locked again");
    }

    #[tokio::test]
    async fn a_release_while_someone_else_has_the_screen_leaves_it_theirs() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        mia_on_screen(&host, false);
        host.w().greeter_vt = Some(1);
        let mut lock = LockScreen::new(Box::new(host.clone()), sh, None);
        assert!(lock.present("mia", face()).await);
        assert!(lock.switch_user());
        assert!(lock.reassert(&frozen_mia).await);
        host.w().log.clear();
        // mia's time came back (a parent's grant): the lock goes, the login
        // screen stays, and nothing touches her desktop's own lock.
        lock.release();
        let w = host.w();
        assert_eq!(w.vt, 1);
        assert!(!w.switch_locked);
        assert!(w.unlocked.is_empty());
        assert!(
            w.log.iter().all(|l| !l.starts_with("switch ")),
            "{:?}",
            w.log
        );
    }

    #[tokio::test]
    async fn a_desktop_its_owner_had_locked_stays_locked() {
        let sh = shared();
        let host = FakeHost::new(sh.clone());
        mia_on_screen(&host, false);
        host.w().sessions[0].locked = true; // she walked away; GNOME locked
        let mut lock = LockScreen::new(Box::new(host.clone()), sh, None);
        assert!(lock.present("mia", face()).await);
        assert_eq!(lock.shown().unwrap().desktop_locked, Some(true));
        lock.release();
        let w = host.w();
        assert_eq!(w.vt, 2);
        assert!(w.unlocked.is_empty(), "never opened by the lock");
    }

    fn with_class(mut s: Session, class: &str) -> Session {
        s.class = class.into();
        s
    }

    #[test]
    fn the_lock_stands_in_front_of_stopped_people_only() {
        let mia_frozen = |u: &str| u == "mia";
        let sessions = vec![
            session("2", "mia", 2, false),
            session("5", "sam", 3, false),
            with_class(session("g1", "Debian-gdm", 1, false), "greeter"),
            with_class(session("c1", LOCK_USER, LOCK_VT, false), "greeter"),
        ];
        let all = Grace {
            empty_vt: true,
            login_screen: true,
        };
        let none = Grace::default();
        let place = |vt: u32, grace: Grace| {
            placement(&sessions, Some(vt), TEXT_VT, "mia", &mia_frozen, grace)
        };
        assert_eq!(place(TEXT_VT, none), Placement::Hold);
        // Her frozen desktop on screen: the lock goes in front, grace or not.
        assert_eq!(place(2, all), Placement::Front("mia".into()));
        // Someone else's session: theirs, for as long as they like.
        assert_eq!(place(3, none), Placement::Aside);
        // The login screen: someone may be signing in — for a while.
        assert_eq!(place(1, all), Placement::Aside);
        assert_eq!(place(1, none), Placement::Front("mia".into()));
        // The lock's own other VT is still the lock.
        assert_eq!(place(LOCK_VT, all), Placement::Front("mia".into()));
        // Nobody on the VT: a login screen on its way (grace), or not.
        let empty = Grace {
            empty_vt: true,
            login_screen: false,
        };
        assert_eq!(place(6, empty), Placement::Aside);
        assert_eq!(place(6, none), Placement::Front("mia".into()));
        // A second stopped person switched to: the lock is theirs now.
        let both = |u: &str| u == "mia" || u == "sam";
        assert_eq!(
            placement(&sessions, Some(3), TEXT_VT, "mia", &both, none),
            Placement::Front("sam".into())
        );
        // A session that is closing is nobody.
        let mut closing = sessions.clone();
        closing[1].state = "closing".into();
        assert_eq!(
            placement(&closing, Some(3), TEXT_VT, "mia", &mia_frozen, none),
            Placement::Front("mia".into())
        );
        assert_eq!(login_screen(&sessions).map(|s| s.id.as_str()), Some("g1"));
        assert!(
            login_screen(&sessions[3..]).is_none(),
            "our lock is no login screen"
        );
    }

    #[test]
    fn reads_whether_cage_gave_up() {
        assert!(!unit_gave_up(
            "ActiveState=active\nSubState=running\nResult=success\n"
        ));
        // Queued, not started yet.
        assert!(!unit_gave_up(
            "ActiveState=inactive\nSubState=dead\nResult=success\n"
        ));
        assert!(unit_gave_up(
            "ActiveState=activating\nSubState=auto-restart\nResult=exit-code\n"
        ));
        assert!(unit_gave_up(
            "ActiveState=failed\nSubState=failed\nResult=exit-code\n"
        ));
        assert!(unit_gave_up(
            "ActiveState=inactive\nSubState=dead\nResult=exit-code\n"
        ));
        assert!(!unit_gave_up(""));
        assert_eq!(
            dm_seat_paths("ao 1 \"/org/freedesktop/DisplayManager/Seat0\"\n"),
            vec!["/org/freedesktop/DisplayManager/Seat0".to_string()]
        );
        assert_eq!(dm_seat_paths("").len(), 2);
    }
}
