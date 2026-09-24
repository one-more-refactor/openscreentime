//! Virtual-terminal plumbing for the lock: which VT is on screen, switching to
//! one, and the kernel's VT-switch lock (the mechanism `vlock -a` uses).
//!
//! Everything here is a root-side ioctl on the console. None of it needs the
//! person's compositor: when root switches VTs, logind (which owns the VT of a
//! logind-managed compositor) acknowledges the release itself. That is what
//! lets the lock appear, and the code be typed, while the desktop underneath is
//! about to be — or already is — frozen.

use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

// <linux/vt.h>, <linux/kd.h>
const VT_SETMODE: u32 = 0x5602;
const VT_ACTIVATE: u32 = 0x5606;
const VT_LOCKSWITCH: u32 = 0x560B;
const VT_UNLOCKSWITCH: u32 = 0x560C;
const KDSETMODE: u32 = 0x4B3A;
const KD_TEXT: libc::c_int = 0;
const KDSKBMODE: u32 = 0x4B45;
const K_UNICODE: libc::c_int = 3;
const VT_AUTO: i8 = 0;

#[repr(C)]
struct VtMode {
    mode: i8,
    waitv: i8,
    relsig: i16,
    acqsig: i16,
    frsig: i16,
}

/// The VT on screen right now (`/sys/class/tty/tty0/active` → `tty2` → 2).
pub fn active() -> Option<u32> {
    let s = std::fs::read_to_string("/sys/class/tty/tty0/active").ok()?;
    parse_active(&s)
}

fn parse_active(s: &str) -> Option<u32> {
    s.trim().strip_prefix("tty")?.parse().ok()
}

fn open(path: &str) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC)
        .open(path)
}

fn ioctl_int(f: &std::fs::File, req: u32, arg: libc::c_int) -> bool {
    // SAFETY: plain integer-argument console ioctls on an fd we own.
    unsafe { libc::ioctl(f.as_raw_fd(), req as _, arg) == 0 }
}

/// Ask the kernel to switch to `vt` and wait (bounded) until it is on screen.
pub fn switch_to(vt: u32, timeout: Duration) -> bool {
    if active() == Some(vt) {
        return true;
    }
    let Ok(con) = open("/dev/tty0") else {
        return false;
    };
    if !ioctl_int(&con, VT_ACTIVATE, vt as libc::c_int) {
        tracing::warn!(
            "VT_ACTIVATE {vt} failed: {}",
            std::io::Error::last_os_error()
        );
        return false;
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if active() == Some(vt) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    active() == Some(vt)
}

/// Take `vt` back to automatic switching. A VT in `VT_PROCESS` mode whose
/// owner never acknowledges a release blocks every switch away from it; root
/// may reset it. Only used when a switch to the lock did not happen in time.
pub fn force_auto(vt: u32) -> bool {
    let Ok(f) = open(&format!("/dev/tty{vt}")) else {
        return false;
    };
    let mode = VtMode {
        mode: VT_AUTO,
        waitv: 0,
        relsig: 0,
        acqsig: 0,
        frsig: 0,
    };
    // SAFETY: VT_SETMODE reads one `struct vt_mode` from the pointer.
    unsafe { libc::ioctl(f.as_raw_fd(), VT_SETMODE as _, &mode as *const VtMode) == 0 }
}

/// Stop (or allow) VT switching from the keyboard *and* by ioctl — the kernel
/// flag `vlock -a` sets. Used only by the text lock, which has no compositor
/// to swallow Ctrl+Alt+Fn for it.
pub fn set_switch_lock(on: bool) -> bool {
    let Ok(con) = open("/dev/tty0") else {
        return false;
    };
    ioctl_int(&con, if on { VT_LOCKSWITCH } else { VT_UNLOCKSWITCH }, 0)
}

/// Put a VT back into text mode with a normal keyboard, whatever a crashed
/// compositor left behind, so the text lock can draw and read keys there.
pub fn make_text(f: &std::fs::File) {
    let _ = ioctl_int(f, KDSETMODE, KD_TEXT);
    let _ = ioctl_int(f, KDSKBMODE, K_UNICODE);
}

/// Open a VT's device for the text lock (root).
pub fn open_vt(vt: u32) -> std::io::Result<std::fs::File> {
    open(&format!("/dev/tty{vt}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_active_vt() {
        assert_eq!(parse_active("tty2\n"), Some(2));
        assert_eq!(parse_active("tty13"), Some(13));
        assert_eq!(parse_active(""), None);
        assert_eq!(parse_active("ttyS0"), None);
    }
}
