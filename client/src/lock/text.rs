//! The text lock: the fallback that needs nothing but a VT.
//!
//! Drawn by the agent itself on the lock VT when there is no `cage`, the build
//! has no GUI, or the graphical lock didn't come up. VT switching is locked
//! while it is up (the runner does that), so the keyboard can't leave; the code
//! typed here goes to the runner exactly like the graphical lock's does.
//!
//! Deliberately plain, and the same words as the graphical lock: the ring as
//! a few lines of text (with its tick at 12 and "0 min left" inside), one
//! sentence saying why, the code prompt with tries left, and how to ask. ASCII
//! only — console fonts can't be trusted with anything else. No box art.

use super::socket::{Outcome, Pending, Request};
use super::{
    current_face, mark_seen, AskState, CodeState, Face, LockEvent, LockTx, Look, SharedRef, Snooze,
};
use nix::sys::termios;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct TextLock {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl TextLock {
    /// Open the VT, put it in text mode and start drawing and reading keys.
    pub fn start(vt: u32, shared: SharedRef, tx: LockTx) -> std::io::Result<TextLock> {
        let tty = super::vt::open_vt(vt)?;
        super::vt::make_text(&tty);
        let saved = termios::tcgetattr(&tty).map_err(std::io::Error::from)?;
        let mut raw = saved.clone();
        termios::cfmakeraw(&mut raw);
        termios::tcsetattr(&tty, termios::SetArg::TCSANOW, &raw).map_err(std::io::Error::from)?;
        // Keys pressed before the lock was up (or during an agent restart) are
        // not a code anyone meant to type.
        let _ = termios::tcflush(&tty, termios::FlushArg::TCIFLUSH);
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (s, d) = (stop.clone(), done.clone());
        std::thread::Builder::new()
            .name("ost-text-lock".into())
            .spawn(move || {
                let mut ui = Ui {
                    tty,
                    shared,
                    tx,
                    typed: String::new(),
                    message: None,
                    drawn: String::new(),
                };
                ui.run(&s);
                let _ = termios::tcsetattr(&ui.tty, termios::SetArg::TCSANOW, &saved);
                let _ = ui.tty.write_all(b"\x1b[0m\x1b[2J\x1b[H\x1b[?25h");
                d.store(true, Ordering::SeqCst);
            })?;
        Ok(TextLock { stop, done })
    }

    pub fn running(&self) -> bool {
        !self.done.load(Ordering::SeqCst)
    }

    /// Ask the drawing thread to finish. Not joined: it may be waiting on the
    /// very reply whose handling is stopping it.
    pub fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct Ui {
    tty: std::fs::File,
    shared: SharedRef,
    tx: LockTx,
    typed: String,
    message: Option<Outcome>,
    drawn: String,
}

impl Ui {
    fn run(&mut self, stop: &AtomicBool) {
        let mut buf = [0u8; 64];
        while !stop.load(Ordering::SeqCst) {
            self.draw();
            mark_seen(&self.shared);
            if !readable(&self.tty, Duration::from_millis(250)) {
                continue;
            }
            let n = match self.tty.read(&mut buf) {
                Ok(0) | Err(_) => {
                    // Hung up underneath us: let the runner notice and restart.
                    std::thread::sleep(Duration::from_millis(250));
                    return;
                }
                Ok(n) => n,
            };
            let face = current_face(&self.shared).unwrap_or_else(Face::waiting);
            let mut i = 0;
            while i < n {
                match buf[i] {
                    b'0'..=b'9' if self.typed.len() < 8 => {
                        self.typed.push(buf[i] as char);
                        self.message = None;
                    }
                    0x7f | 0x08 => {
                        self.typed.pop();
                    }
                    0x15 => self.typed.clear(), // Ctrl-U
                    b'\r' | b'\n' if !self.typed.is_empty() => {
                        if matches!(face.code, CodeState::Ready { .. }) {
                            let code = std::mem::take(&mut self.typed);
                            self.message = Some(Outcome::yes("Checking..."));
                            self.draw();
                            self.message = self.ask_runner(Request::Code { code });
                        }
                    }
                    b'a' | b'A' if face.ask == AskState::Ready => {
                        self.message = Some(Outcome::yes("Asking..."));
                        self.draw();
                        self.message = self.ask_runner(Request::Ask);
                    }
                    b'g' | b'G' if matches!(face.snooze, Snooze::Ready { .. }) => {
                        self.message = Some(Outcome::yes("One moment..."));
                        self.draw();
                        self.message = self.ask_runner(Request::Snooze);
                    }
                    0x1b => {
                        // An escape sequence (arrows, F-keys): skip the rest of this read.
                        break;
                    }
                    _ => {}
                }
                i += 1;
            }
        }
    }

    fn ask_runner(&self, req: Request) -> Option<Outcome> {
        let (rtx, rrx) = tokio::sync::oneshot::channel();
        if self
            .tx
            .blocking_send(LockEvent::Request(Pending { req, reply: rtx }))
            .is_err()
        {
            return Some(Outcome::no(
                "Can't check that right now - try again in a moment.",
            ));
        }
        match rrx.blocking_recv() {
            Ok(r) => r.result,
            Err(_) => Some(Outcome::no(
                "Can't check that right now - try again in a moment.",
            )),
        }
    }

    fn draw(&mut self) {
        let face = current_face(&self.shared).unwrap_or_else(Face::waiting);
        let (cols, rows) = size(&self.tty);
        let screen = render(&face, &self.typed, self.message.as_ref(), cols, rows);
        if screen == self.drawn {
            return;
        }
        let _ = self.tty.write_all(screen.as_bytes());
        self.drawn = screen;
    }
}

fn readable(f: &std::fs::File, t: Duration) -> bool {
    let mut pfd = libc::pollfd {
        fd: f.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for an fd we own.
    let r = unsafe { libc::poll(&mut pfd, 1, t.as_millis() as libc::c_int) };
    r > 0
}

fn size(f: &std::fs::File) -> (usize, usize) {
    let mut ws = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ fills one winsize.
    let ok = unsafe { libc::ioctl(f.as_raw_fd(), libc::TIOCGWINSZ as _, &mut ws) } == 0;
    if ok && ws.ws_col >= 40 && ws.ws_row >= 16 {
        (ws.ws_col as usize, ws.ws_row as usize)
    } else {
        (80, 25)
    }
}

/// Console fonts are not to be trusted beyond ASCII.
fn ascii(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '—' | '–' => '-',
            '’' | '‘' => '\'',
            '“' | '”' => '"',
            '·' | '•' => '-',
            '…' => '.',
            c if c.is_ascii() && !c.is_ascii_control() => c,
            _ => ' ',
        })
        .collect()
}

/// "123456" → "123 456", "12345678" → "1234 5678".
pub fn group(d: &str) -> String {
    let split = if d.len() > 6 { 4 } else { 3 };
    if d.len() > split {
        format!("{} {}", &d[..split], &d[split..])
    } else {
        d.to_string()
    }
}

/// The whole screen as one write: clear, then each line centred.
pub fn render(
    face: &Face,
    typed: &str,
    message: Option<&Outcome>,
    cols: usize,
    rows: usize,
) -> String {
    let ring_colour = match face.look {
        Look::Wall | Look::Night => "\x1b[1;31m",
        Look::Paused => "\x1b[2;37m",
    };
    let mut lines: Vec<(String, &str)> = Vec::new();
    // The tick at 12, then the ring with what's left inside it.
    lines.push(("|".to_string(), "\x1b[1m"));
    let (a, b) = if face.look == Look::Paused {
        ("", "paused")
    } else {
        ("0", "min left")
    };
    let inside = |t: &str| format!("o{:^15}o", t);
    for l in [
        "o  o  o".to_string(),
        "o           o".to_string(),
        inside(a),
        inside(b),
        inside(""),
        "o           o".to_string(),
        "o  o  o".to_string(),
    ] {
        let l = if face.look == Look::Paused {
            l.replace('o', ".")
        } else {
            l
        };
        lines.push((l, ring_colour));
    }
    lines.push((String::new(), ""));
    lines.push((ascii(&face.title), "\x1b[1m"));
    if !face.detail.is_empty() {
        lines.push((ascii(&face.detail), ""));
    }
    lines.push((String::new(), ""));
    match &face.code {
        CodeState::Ready { tries_left } => {
            let shown = if typed.is_empty() {
                "_".to_string()
            } else {
                format!("{}_", group(typed))
            };
            lines.push((format!("Unlock code:  {shown}"), "\x1b[1m"));
            lines.push((
                format!(
                    "{tries_left} {} left - Enter unlocks",
                    if *tries_left == 1 { "try" } else { "tries" }
                ),
                "\x1b[2m",
            ));
        }
        CodeState::Wait { secs } => {
            lines.push((format!("Too many tries - wait {secs} seconds."), "\x1b[1m"));
        }
        CodeState::Unavailable => {
            lines.push(("There's no unlock code on this computer yet.".into(), ""));
        }
    }
    if let Some(m) = message {
        lines.push((
            ascii(&m.message),
            if m.ok { "\x1b[32m" } else { "\x1b[31m" },
        ));
    } else {
        lines.push((String::new(), ""));
    }
    lines.push((String::new(), ""));
    match face.ask {
        AskState::Ready => lines.push(("Press A to ask for more time.".into(), "")),
        AskState::Sent => lines.push(("Asked - a parent will see it.".into(), "\x1b[2m")),
        AskState::Hidden => {}
    }
    match &face.snooze {
        Snooze::Hidden => {}
        Snooze::Wait { secs } => lines.push((
            format!("In {secs} s you can give yourself 15 more minutes."),
            "\x1b[2m",
        )),
        Snooze::Ready { .. } => {
            lines.push(("Press G to give yourself 15 more minutes.".into(), ""))
        }
        Snooze::UsedUp { back } => lines.push((
            match back {
                Some(b) => format!("That's today's extra time - back {b}."),
                None => "That's today's extra time.".into(),
            },
            "\x1b[2m",
        )),
    }
    lines.push((String::new(), ""));
    lines.push((ascii(&face.help), "\x1b[2m"));

    let top = rows.saturating_sub(lines.len()) / 2;
    let mut out = String::from("\x1b[0m\x1b[?25l\x1b[2J");
    for (i, (text, style)) in lines.iter().enumerate() {
        let text: String = text.chars().take(cols.saturating_sub(2)).collect();
        let col = cols.saturating_sub(text.len()) / 2 + 1;
        out.push_str(&format!(
            "\x1b[{};{}H{}{}\x1b[0m",
            top + i + 1,
            col,
            style,
            text
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_grouped_as_printed() {
        assert_eq!(group("123"), "123");
        assert_eq!(group("1234"), "123 4");
        assert_eq!(group("123456"), "123 456");
        assert_eq!(group("1234567"), "1234 567");
        assert_eq!(group("12345678"), "1234 5678");
    }

    #[test]
    fn the_text_lock_says_why_and_how_out() {
        let face = Face {
            look: Look::Night,
            title: "Bedtime until 07:00".into(),
            detail: "Screens are off until morning.".into(),
            who: "mia".into(),
            code: CodeState::Ready { tries_left: 4 },
            ask: AskState::Ready,
            snooze: Snooze::Hidden,
            help: super::super::HELP.into(),
        };
        let s = render(&face, "1234", None, 80, 25);
        assert!(s.contains("Bedtime until 07:00"));
        assert!(s.contains("Unlock code:  123 4_"));
        assert!(s.contains("4 tries left"));
        assert!(s.contains("Press A to ask"));
        assert!(s.contains("min left"), "the ring says what's left");
        assert!(s.is_ascii(), "console fonts only get ASCII");
        // The same words as the graphical lock, for the self-set snooze too.
        let adult = Face {
            ask: AskState::Hidden,
            snooze: Snooze::Wait { secs: 42 },
            ..face.clone()
        };
        let s = render(&adult, "", None, 80, 25);
        assert!(s.contains("In 42 s you can give yourself 15 more minutes."));
        assert!(!s.contains("Press A"));
        let none = Face {
            code: CodeState::Unavailable,
            help: super::super::HELP_NO_CODE.into(),
            ..face
        };
        let s = render(&none, "", None, 80, 25);
        assert!(s.contains("no unlock code on this computer yet"));
        assert!(!s.contains("Unlock code:"));
    }
}
