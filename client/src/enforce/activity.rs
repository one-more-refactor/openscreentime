//! Who is really using this computer right now — the input to every
//! screen-time minute.
//!
//! A minute counts for a person only while
//!
//! 1. their session is the **foreground session on a seat** (logind:
//!    `Active=yes`, `State=active`, a non-empty `Seat`, a `user*` class), and
//! 2. there was **keyboard/mouse/touch/gamepad input on that seat within the
//!    last [`IDLE_CUTOFF`]**, *or* **audio is playing**.
//!
//! Everything else is not screen time: the systemd ≥ 256 `Class=manager`
//! session, a `closing` session left behind by a surviving process, an SSH
//! login (seatless), a fast-user-switched background session, a locked screen
//! nobody touches, a laptop with its lid closed.
//!
//! Both signals are read by root and cannot be switched off from the child's
//! account: input via evdev (`/dev/input/event*`, read **non-exclusively** —
//! never grabbed, so nothing else on the machine notices), audio via
//! `/proc/asound/card*/pcm*p/sub*/status` (`state: RUNNING`). Faking either one
//! can only make time count *more*. That is what makes this safe where logind's
//! `IdleHint` was not (the session's owner can set that one; see `df96882`).
//!
//! Privacy: the reader looks at the event *type* only (key / pointer / axis),
//! keeps one number per seat — when the last input happened — and throws the
//! rest away. No key codes are stored, logged or sent anywhere.
//!
//! Where input can't be read at all (no `/dev/input`, not root, a container),
//! the seat falls back to presence — the old behaviour — rather than never
//! counting; [`Activity::measured`] says which one is in force.
//!
//! The billable decision is the pure [`billable`]; the probes sit behind
//! [`ActivityProbe`] so it can be tested without a seat.

use crate::util::Exec;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// No input for this long (and no sound) and the minute stops counting —
/// roughly when a desktop would blank the screen.
pub const IDLE_CUTOFF: Duration = Duration::from_secs(5 * 60);

/// One logind session, as far as screen time cares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    pub user: String,
    pub seat: String,
    pub class: String,
    pub state: String,
    pub active: bool,
}

impl Session {
    /// The foreground login of a real person on a real seat.
    pub fn at_seat(&self) -> bool {
        !self.user.is_empty()
            && !self.seat.is_empty()
            && self.active
            // Older logind has no State; newer marks background/leftover
            // sessions `online` / `closing`.
            && (self.state.is_empty() || self.state == "active")
            // user, user-early, user-light, user-incomplete… — never manager,
            // greeter, lock-screen, background, none.
            && (self.class.is_empty() || self.class.starts_with("user"))
    }
}

/// Parse `loginctl show-session <ids…> -p Name -p Active -p Class -p State
/// -p Seat` output: one blank-line-separated block per session. Properties
/// with empty values are simply absent (loginctl skips them without `-a`).
pub fn parse_sessions(out: &str) -> Vec<Session> {
    out.split("\n\n")
        .filter_map(|block| {
            let mut s = Session::default();
            let mut any = false;
            for line in block.lines() {
                let Some((k, v)) = line.split_once('=') else {
                    continue;
                };
                any = true;
                let v = v.trim();
                match k.trim() {
                    "Name" => s.user = v.to_string(),
                    "Seat" => s.seat = v.to_string(),
                    "Class" => s.class = v.to_string(),
                    "State" => s.state = v.to_string(),
                    "Active" => s.active = v == "yes",
                    _ => {}
                }
            }
            any.then_some(s)
        })
        .collect()
}

/// Who is at a seat right now: `(user, seat)`, one entry per user.
pub fn present(sessions: &[Session]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for s in sessions.iter().filter(|s| s.at_seat()) {
        if !out.iter().any(|(u, _)| *u == s.user) {
            out.push((s.user.clone(), s.seat.clone()));
        }
    }
    out
}

/// THE decision: which of the present users' time counts this minute.
///
/// `input_idle(seat)` is how long since the last input on that seat, or
/// `None` when no input device on it can be read (then presence counts, the
/// honest fallback — see the module docs). `audio` is any sound playing.
pub fn billable(
    present: &[(String, String)],
    input_idle: &dyn Fn(&str) -> Option<Duration>,
    audio: bool,
    cutoff: Duration,
) -> Vec<String> {
    present
        .iter()
        .filter(|(_, seat)| match input_idle(seat) {
            None => true,
            Some(idle) => idle <= cutoff || audio,
        })
        .map(|(u, _)| u.clone())
        .collect()
}

/// The probes behind the decision.
pub trait ActivityProbe {
    fn sessions(&self) -> Vec<Session>;
    /// Time since the last input on `seat`; `None` if nothing there is readable.
    fn input_idle(&self, seat: &str) -> Option<Duration>;
    fn audio_playing(&self) -> bool;
}

/// One sample of the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    /// Users at a seat (whether or not they are doing anything).
    pub present: Vec<String>,
    /// Users whose time counts right now.
    pub billable: Vec<String>,
    /// Every present seat had readable input (false = presence fallback).
    pub measured: bool,
}

pub fn sample(probe: &dyn ActivityProbe) -> Activity {
    let at = present(&probe.sessions());
    let audio = probe.audio_playing();
    let idle = |seat: &str| probe.input_idle(seat);
    Activity {
        billable: billable(&at, &idle, audio, IDLE_CUTOFF),
        measured: at.iter().all(|(_, seat)| probe.input_idle(seat).is_some()),
        present: at.into_iter().map(|(u, _)| u).collect(),
    }
}

// ---- the real probes ----------------------------------------------------------

/// logind + evdev + ALSA.
pub struct SystemProbe<'a> {
    pub exec: &'a Exec,
    pub input: &'a InputTracker,
}

impl ActivityProbe for SystemProbe<'_> {
    fn sessions(&self) -> Vec<Session> {
        let listing = self
            .exec
            .probe("loginctl", &["list-sessions", "--no-legend"]);
        // Bounded: a managed user can open sessions without sudo; one
        // show-session call for all of them, seated ones first so a flood of
        // SSH logins can't push the real seat past the cap.
        const MAX_SESSIONS: usize = 64;
        let mut rows: Vec<(&str, bool)> = listing
            .lines()
            .filter_map(|l| {
                let c: Vec<&str> = l.split_whitespace().collect();
                // SESSION UID USER SEAT …
                c.first()
                    .map(|id| (*id, c.get(3).is_some_and(|s| s.starts_with("seat"))))
            })
            .collect();
        rows.sort_by_key(|(_, seated)| !*seated);
        let ids: Vec<&str> = rows.iter().map(|(id, _)| *id).take(MAX_SESSIONS).collect();
        if ids.is_empty() {
            return Vec::new();
        }
        let mut args = vec!["show-session"];
        args.extend(ids.iter().copied());
        args.extend([
            "-p", "Name", "-p", "Active", "-p", "Class", "-p", "State", "-p", "Seat",
        ]);
        parse_sessions(&self.exec.probe("loginctl", &args))
    }

    fn input_idle(&self, seat: &str) -> Option<Duration> {
        self.input.idle(seat, crate::clock::boottime())
    }

    fn audio_playing(&self) -> bool {
        audio_playing_in(Path::new("/proc/asound"))
    }
}

/// Any ALSA playback substream in `state: RUNNING`.
pub fn audio_playing_in(asound: &Path) -> bool {
    let Ok(cards) = std::fs::read_dir(asound) else {
        return false;
    };
    for card in cards.flatten() {
        let name = card.file_name();
        if !name.to_string_lossy().starts_with("card") {
            continue;
        }
        let Ok(pcms) = std::fs::read_dir(card.path()) else {
            continue;
        };
        for pcm in pcms.flatten() {
            let pn = pcm.file_name().to_string_lossy().to_string();
            if !(pn.starts_with("pcm") && pn.ends_with('p')) {
                continue; // capture devices (…c) are not "watching"
            }
            let Ok(subs) = std::fs::read_dir(pcm.path()) else {
                continue;
            };
            for sub in subs.flatten() {
                if !sub.file_name().to_string_lossy().starts_with("sub") {
                    continue;
                }
                if std::fs::read_to_string(sub.path().join("status"))
                    .is_ok_and(|s| s.lines().any(|l| l.trim() == "state: RUNNING"))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Watches every input device, per seat, from background threads.
///
/// One blocking reader thread per `/dev/input/event*` node (a handful on a
/// normal machine); a node that disappears ends its thread, a new one is
/// picked up by the next [`rescan`](Self::rescan).
#[derive(Clone, Default)]
pub struct InputTracker {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// seat → boottime of the last input there.
    last: Mutex<HashMap<String, Duration>>,
    /// device node → seat, for nodes a reader thread holds open.
    watching: Mutex<HashMap<PathBuf, String>>,
}

impl InputTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start readers for input devices not yet watched. Cheap; call each tick.
    pub fn rescan(&self) {
        let Ok(dir) = std::fs::read_dir("/dev/input") else {
            return;
        };
        for entry in dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("event") {
                continue;
            }
            if self.inner.watching.lock().unwrap().contains_key(&path) {
                continue;
            }
            if is_accelerometer(&name) {
                continue; // tilting a convertible is not using it
            }
            let Ok(file) = std::fs::File::open(&path) else {
                continue; // not root / gone: nothing to watch here
            };
            let seat = seat_of(&file).unwrap_or_else(|| "seat0".to_string());
            self.inner
                .watching
                .lock()
                .unwrap()
                .insert(path.clone(), seat.clone());
            let inner = self.inner.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("ost-input-{name}"))
                .spawn(move || read_events(file, &path, &seat, &inner));
            if spawned.is_err() {
                // No thread, no watching.
                self.inner.watching.lock().unwrap().remove(&entry.path());
            }
        }
    }

    /// Time since the last input on `seat`, or `None` when no device on that
    /// seat is being watched.
    pub fn idle(&self, seat: &str, boot_now: Duration) -> Option<Duration> {
        let watched = self
            .inner
            .watching
            .lock()
            .unwrap()
            .values()
            .any(|s| s == seat);
        if !watched {
            return None;
        }
        Some(
            self.inner
                .last
                .lock()
                .unwrap()
                .get(seat)
                .map(|t| boot_now.saturating_sub(*t))
                .unwrap_or(Duration::MAX),
        )
    }
}

/// `/sys/class/input/eventN/device/properties` has INPUT_PROP_ACCELEROMETER
/// (bit 6) set.
fn is_accelerometer(event_name: &str) -> bool {
    std::fs::read_to_string(format!("/sys/class/input/{event_name}/device/properties"))
        .ok()
        .and_then(|s| u64::from_str_radix(s.trim(), 16).ok())
        .is_some_and(|bits| bits & (1 << 6) != 0)
}

/// The seat udev assigned this device (`ID_SEAT`), if any.
fn seat_of(file: &std::fs::File) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let rdev = file.metadata().ok()?.rdev();
    let (major, minor) = (libc::major(rdev), libc::minor(rdev));
    let db = std::fs::read_to_string(format!("/run/udev/data/c{major}:{minor}")).ok()?;
    db.lines()
        .find_map(|l| l.strip_prefix("E:ID_SEAT="))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `struct input_event`'s `type` sits 8 bytes from the end on every ABI
/// (timeval, then u16 type, u16 code, i32 value).
fn batch_has_input(buf: &[u8]) -> bool {
    const EV_KEY: u16 = 1;
    const EV_REL: u16 = 2;
    const EV_ABS: u16 = 3;
    let size = std::mem::size_of::<libc::input_event>();
    buf.chunks_exact(size).any(|ev| {
        let t = u16::from_ne_bytes([ev[size - 8], ev[size - 7]]);
        matches!(t, EV_KEY | EV_REL | EV_ABS)
    })
}

fn read_events(mut file: std::fs::File, path: &Path, seat: &str, inner: &Inner) {
    let size = std::mem::size_of::<libc::input_event>();
    let mut buf = vec![0u8; size * 64];
    let mut last_mark = Duration::ZERO;
    loop {
        match file.read(&mut buf) {
            Ok(0) | Err(_) => break, // unplugged
            Ok(n) => {
                if !batch_has_input(&buf[..n - n % size]) {
                    continue;
                }
                let now = crate::clock::boottime();
                // A mouse sends ~1000 events/s; one timestamp a second is plenty.
                if now.saturating_sub(last_mark) >= Duration::from_secs(1) {
                    last_mark = now;
                    inner.last.lock().unwrap().insert(seat.to_string(), now);
                }
            }
        }
    }
    inner.watching.lock().unwrap().remove(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `loginctl show-session` output from a systemd 261 box: the seat
    /// session, the seatless manager session, a closing leftover, an SSH
    /// login, a greeter and a fast-user-switched sibling.
    const SHOW: &str = "\
Name=mia
Seat=seat0
Class=user
State=active
Active=yes

Name=mia
Class=manager
State=active
Active=yes

Name=mia
Class=user
State=closing
Active=yes

Name=leo
Class=user
State=active
Active=yes

Name=gdm
Seat=seat0
Class=greeter
State=online
Active=no

Name=ben
Seat=seat0
Class=user
State=online
Active=no
";

    #[test]
    fn only_the_foreground_seat_session_is_present() {
        let s = parse_sessions(SHOW);
        assert_eq!(s.len(), 6);
        assert_eq!(present(&s), vec![("mia".to_string(), "seat0".to_string())]);
    }

    #[test]
    fn manager_closing_ssh_and_background_sessions_never_count() {
        // Without the seat session, nobody is present — the manager session,
        // the closing leftover and the SSH login are not screen time.
        let rest = SHOW.split("\n\n").skip(1).collect::<Vec<_>>().join("\n\n");
        assert!(present(&parse_sessions(&rest)).is_empty());
    }

    #[test]
    fn older_logind_without_state_or_class_still_works() {
        let s = parse_sessions("Name=kid\nSeat=seat0\nActive=yes\n");
        assert_eq!(present(&s).len(), 1);
    }

    fn at() -> Vec<(String, String)> {
        vec![("mia".into(), "seat0".into())]
    }

    #[test]
    fn a_minute_is_billable_with_recent_input_or_sound() {
        let recent = |_: &str| Some(Duration::from_secs(30));
        let away = |_: &str| Some(Duration::from_secs(20 * 60));
        assert_eq!(billable(&at(), &recent, false, IDLE_CUTOFF), vec!["mia"]);
        // Walked away for dinner: the locked screen costs nothing.
        assert!(billable(&at(), &away, false, IDLE_CUTOFF).is_empty());
        // Watching a film without touching anything: sound keeps it counting.
        assert_eq!(billable(&at(), &away, true, IDLE_CUTOFF), vec!["mia"]);
        // Exactly at the cutoff still counts; a second later doesn't.
        let edge = |_: &str| Some(IDLE_CUTOFF);
        assert_eq!(billable(&at(), &edge, false, IDLE_CUTOFF).len(), 1);
        let past = |_: &str| Some(IDLE_CUTOFF + Duration::from_secs(1));
        assert!(billable(&at(), &past, false, IDLE_CUTOFF).is_empty());
    }

    #[test]
    fn unmeasurable_input_falls_back_to_presence() {
        let unknown = |_: &str| None;
        assert_eq!(billable(&at(), &unknown, false, IDLE_CUTOFF), vec!["mia"]);
    }

    struct Fake {
        show: &'static str,
        idle: Option<Duration>,
        audio: bool,
    }
    impl ActivityProbe for Fake {
        fn sessions(&self) -> Vec<Session> {
            parse_sessions(self.show)
        }
        fn input_idle(&self, _: &str) -> Option<Duration> {
            self.idle
        }
        fn audio_playing(&self) -> bool {
            self.audio
        }
    }

    #[test]
    fn sample_reports_presence_and_billing_separately() {
        let a = sample(&Fake {
            show: SHOW,
            idle: Some(Duration::from_secs(3600)),
            audio: false,
        });
        assert_eq!(a.present, vec!["mia"]);
        assert!(
            a.billable.is_empty(),
            "present but idle: stopped at bedtime still, not billed"
        );
        assert!(a.measured);
        let a = sample(&Fake {
            show: SHOW,
            idle: None,
            audio: false,
        });
        assert_eq!(a.billable, vec!["mia"]);
        assert!(!a.measured);
    }

    #[test]
    fn input_batches_are_classified_by_type_only() {
        let size = std::mem::size_of::<libc::input_event>();
        let ev = |t: u16| {
            let mut v = vec![0u8; size];
            v[size - 8..size - 6].copy_from_slice(&t.to_ne_bytes());
            v
        };
        assert!(
            !batch_has_input(&[ev(0), ev(5)].concat()),
            "SYN + SW (lid) is not input"
        );
        assert!(batch_has_input(&[ev(0), ev(1)].concat()), "a key is");
        assert!(batch_has_input(&ev(2)), "a mouse move is");
        assert!(batch_has_input(&ev(3)), "a touch/stick is");
    }

    #[test]
    fn audio_state_is_read_from_proc_asound_layout() {
        let dir = std::env::temp_dir().join(format!("ost-asound-{}", std::process::id()));
        let sub = dir.join("card0/pcm0p/sub0");
        let cap = dir.join("card0/pcm0c/sub0");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&cap).unwrap();
        std::fs::write(sub.join("status"), "closed\n").unwrap();
        std::fs::write(cap.join("status"), "state: RUNNING\n").unwrap();
        assert!(
            !audio_playing_in(&dir),
            "a running microphone is not watching"
        );
        std::fs::write(sub.join("status"), "state: RUNNING\nowner_pid   : 1\n").unwrap();
        assert!(audio_playing_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
