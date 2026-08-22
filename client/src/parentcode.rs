//! The parent code — a per-device TOTP (RFC 6238) verified **offline**.
//!
//! The server mints a secret per device and shows it once as an `otpauth://`
//! QR; the parent scans it into their authenticator app. The agent receives
//! the same secret in the policy bundle (`parent_code.totp_secret`, cached
//! root-only) and verifies typed codes here with no server round-trip: the
//! lockout overlay's parent field, `ost unlock`, and the PAM helper that gates
//! `sudo` on a managed machine all go through [`Verifier::verify`].
//!
//! Rules (docs/CONTRACT-0.4.md §4):
//! * SHA1, 6 digits, 30 s steps, ±1 step of clock drift.
//! * **Single-use**: the last accepted counter is persisted, so a code that
//!   was just typed cannot be replayed within its window by someone watching.
//! * Five wrong codes → 60 s lockout, doubling per further failure, capped at
//!   15 min. Persisted, so a restart does not reset the clock.
//! * The device recovery PIN survives only as the **backup code**
//!   (`parent_pin_hash`, argon2): accepted, but reported as such so the parent
//!   knows the authenticator path was bypassed.
//!
//! Why per-device rather than the parent's account TOTP: extracting this
//! secret needs root on the device, and root on the device is already game
//! over *for that device* — it must not also be game over for the parent's
//! console account. One authenticator entry per managed computer is the price.

use crate::protocol::{Event, SEV_INFO, SEV_WARN};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::Sha1;
use std::path::{Path, PathBuf};

pub const STEP_SECS: u64 = 30;
pub const DIGITS: u32 = 6;
/// Accept the previous and next step as well as the current one.
const WINDOW: u64 = 1;
/// Wrong attempts before the first lockout.
const FAILURES_BEFORE_LOCKOUT: u32 = 5;
const LOCKOUT_BASE_SECS: u64 = 60;
const LOCKOUT_MAX_SECS: u64 = 15 * 60;

/// Event types (mirrored in the server's `events.type` CHECK, migration 0015).
pub const EV_PARENT_CODE_OK: &str = "parent_code_ok";
pub const EV_PARENT_CODE_FAILED: &str = "parent_code_failed";
pub const EV_PARENT_CODE_BACKUP_USED: &str = "parent_code_backup_used";

/// Where the replay counter / failure lockout live. Root-only state dir.
pub fn state_path() -> PathBuf {
    crate::paths::state("parent_code.json")
}

/// The outcome of checking a typed code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A fresh, in-window authenticator code.
    Ok,
    /// Matched the backup code (the legacy recovery PIN). Still an unlock,
    /// but the parent should hear that the authenticator was not used.
    Backup,
    Wrong,
    /// Too many wrong attempts; try again in this many seconds.
    LockedOut(u64),
    /// Neither a TOTP secret nor a backup hash is known on this device.
    NotConfigured,
}

impl Verdict {
    /// Anything that should open the door.
    pub fn accepted(&self) -> bool {
        matches!(self, Verdict::Ok | Verdict::Backup)
    }

    /// One line for a person at the keyboard.
    pub fn message(&self) -> String {
        match self {
            Verdict::Ok => "parent code accepted".into(),
            Verdict::Backup => "backup code accepted (authenticator not used)".into(),
            Verdict::Wrong => "wrong code".into(),
            Verdict::LockedOut(s) => format!("too many wrong codes — try again in {s}s"),
            Verdict::NotConfigured => {
                "no parent code is set up on this computer (no authenticator secret and no backup code)".into()
            }
        }
    }
}

/// Persisted replay / lockout state.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct State {
    /// Highest TOTP counter ever accepted. A code is single-use.
    #[serde(default)]
    pub last_counter: u64,
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub locked_until: Option<chrono::DateTime<chrono::Utc>>,
}

impl State {
    fn load_from(path: &Path) -> State {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_to(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string(self) {
            Ok(body) => {
                if let Err(e) = std::fs::write(path, body) {
                    // warn, not debug: losing this is how a code becomes replayable.
                    tracing::warn!("could not persist parent-code state: {e}");
                } else {
                    crate::config::set_owner_only_600(path);
                }
            }
            Err(e) => tracing::warn!("could not serialize parent-code state: {e}"),
        }
    }
}

/// Decode RFC 4648 base32 (the `otpauth://` secret alphabet). Case-insensitive,
/// padding and whitespace ignored. `None` on any other character.
pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let mut buf: u64 = 0;
    let mut bits = 0u32;
    for c in s.chars() {
        let c = c.to_ascii_uppercase();
        if c == '=' || c.is_whitespace() || c == '-' {
            continue;
        }
        let v = match c {
            'A'..='Z' => c as u64 - 'A' as u64,
            '2'..='7' => c as u64 - '2' as u64 + 26,
            _ => return None,
        };
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// The TOTP value for a key at a given step counter, zero-padded to 6 digits.
pub fn totp_at(key: &[u8], counter: u64) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let bin = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    let code = bin % 10u32.pow(DIGITS);
    format!("{code:0width$}", width = DIGITS as usize)
}

fn now_counter() -> u64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs / STEP_SECS
}

/// Verifies parent codes for this device.
#[derive(Debug, Clone)]
pub struct Verifier {
    /// Base32 TOTP secret (from the policy bundle's `parent_code`).
    secret: Option<String>,
    /// Argon2 PHC hash of the backup code (the legacy recovery PIN).
    backup_hash: Option<String>,
    state_path: PathBuf,
}

impl Verifier {
    pub fn new(secret: Option<String>, backup_hash: Option<String>) -> Self {
        Verifier {
            secret: secret.filter(|s| !s.trim().is_empty()),
            backup_hash: backup_hash.filter(|h| !h.trim().is_empty()),
            state_path: state_path(),
        }
    }

    /// The verifier for this device, from what the agent last cached: the
    /// bundle's `parent_code` secret plus the device's backup-code hash
    /// (from any user's policy — the server fills the device PIN in wherever
    /// a profile sets none). Works with no agent process and no network.
    pub fn from_device() -> Self {
        let bundle = crate::policy::load_bundle_cache().ok();
        let secret = bundle
            .as_ref()
            .and_then(|b| b.parent_code.as_ref())
            .map(|p| p.totp_secret.clone());
        let backup = bundle
            .as_ref()
            .and_then(|b| {
                b.users
                    .iter()
                    .find_map(|u| u.policy.parent_pin_hash.clone())
            })
            .or_else(|| {
                crate::policy::load_cache()
                    .ok()
                    .and_then(|p| p.parent_pin_hash)
            });
        Self::new(secret, backup)
    }

    #[cfg(test)]
    pub(crate) fn with_state_path(mut self, p: PathBuf) -> Self {
        self.state_path = p;
        self
    }

    pub fn has_totp(&self) -> bool {
        self.secret.is_some()
    }

    pub fn configured(&self) -> bool {
        self.secret.is_some() || self.backup_hash.is_some()
    }

    /// Check a typed code. Persists the replay counter and failure lockout.
    pub fn verify(&self, code: &str) -> Verdict {
        self.verify_at(code, now_counter(), chrono::Utc::now())
    }

    fn verify_at(
        &self,
        code: &str,
        counter: u64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Verdict {
        if !self.configured() {
            return Verdict::NotConfigured;
        }
        let mut st = State::load_from(&self.state_path);
        if let Some(until) = st.locked_until {
            if until > now {
                return Verdict::LockedOut((until - now).num_seconds().max(1) as u64);
            }
        }

        let code = code.trim().replace(' ', "");
        let mut verdict = Verdict::Wrong;

        if let Some(key) = self.secret.as_deref().and_then(base32_decode) {
            if code.len() == DIGITS as usize && code.chars().all(|c| c.is_ascii_digit()) {
                let lo = counter.saturating_sub(WINDOW);
                for c in lo..=counter + WINDOW {
                    // Single-use: never accept a counter at or below the last one.
                    if c > st.last_counter && totp_at(&key, c) == code {
                        st.last_counter = c;
                        verdict = Verdict::Ok;
                        break;
                    }
                }
            }
        }
        if verdict == Verdict::Wrong {
            if let Some(hash) = self.backup_hash.as_deref() {
                if crate::pin::verify_pin(&code, hash) {
                    verdict = Verdict::Backup;
                }
            }
        }

        match verdict {
            Verdict::Ok | Verdict::Backup => {
                st.failures = 0;
                st.locked_until = None;
            }
            _ => {
                st.failures += 1;
                if st.failures >= FAILURES_BEFORE_LOCKOUT {
                    let extra = st.failures - FAILURES_BEFORE_LOCKOUT;
                    let secs = LOCKOUT_BASE_SECS
                        .saturating_mul(1u64 << extra.min(10))
                        .min(LOCKOUT_MAX_SECS);
                    st.locked_until = Some(now + chrono::Duration::seconds(secs as i64));
                    verdict = Verdict::LockedOut(secs);
                }
            }
        }
        st.save_to(&self.state_path);
        verdict
    }
}

/// The audit event for a verification attempt: `parent_code_ok` /
/// `parent_code_backup_used` (warn) / `parent_code_failed` (warn).
/// `via` is where it was typed: `"overlay"`, `"unlock"`, `"tray"`, `"pam"`.
pub fn event(verdict: &Verdict, via: &str, user: &str) -> Event {
    let (kind, sev, detail) = match verdict {
        Verdict::Ok => (EV_PARENT_CODE_OK, SEV_INFO, "authenticator code accepted"),
        Verdict::Backup => (
            EV_PARENT_CODE_BACKUP_USED,
            SEV_WARN,
            "backup code accepted — the authenticator was not used",
        ),
        Verdict::Wrong => (EV_PARENT_CODE_FAILED, SEV_WARN, "wrong code"),
        Verdict::LockedOut(_) => (EV_PARENT_CODE_FAILED, SEV_WARN, "locked out after repeated wrong codes"),
        Verdict::NotConfigured => (EV_PARENT_CODE_FAILED, SEV_WARN, "no parent code configured"),
    };
    let mut ev = Event::new(
        kind,
        sev,
        json!({ "via": via, "user": user, "detail": detail }),
    );
    if !user.is_empty() {
        ev = ev.for_user(user);
    }
    ev
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    use argon2::Argon2;

    /// RFC 6238 appendix B: ASCII secret "12345678901234567890", T=59 → counter 1,
    /// 8-digit SHA1 code 94287082 — so the 6-digit code is 287082.
    const RFC_SECRET_B32: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ost-parentcode-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn base32_decodes_the_rfc_secret() {
        assert_eq!(
            base32_decode(RFC_SECRET_B32).unwrap(),
            b"12345678901234567890".to_vec()
        );
        // lower-case, spaces and padding are tolerated; junk is not
        assert_eq!(
            base32_decode("gezd gnbv gy3t qojq gezd gnbv gy3t qojq====").unwrap(),
            b"12345678901234567890".to_vec()
        );
        assert!(base32_decode("not!base32").is_none());
    }

    #[test]
    fn rfc6238_vectors() {
        let key = base32_decode(RFC_SECRET_B32).unwrap();
        assert_eq!(totp_at(&key, 1), "287082"); // T=59
        assert_eq!(totp_at(&key, 37037036), "081804"); // T=1111111109 → 07081804
        assert_eq!(totp_at(&key, 66666666), "279037"); // T=2000000000 → 69279037
    }

    #[test]
    fn accepts_window_and_rejects_replay() {
        let key = base32_decode(RFC_SECRET_B32).unwrap();
        let v = Verifier::new(Some(RFC_SECRET_B32.into()), None).with_state_path(tmp("replay.json"));
        let now = chrono::Utc::now();
        let c = 1000u64;
        // previous step is fine
        assert_eq!(v.verify_at(&totp_at(&key, c - 1), c, now), Verdict::Ok);
        // the same code again is a replay
        assert_eq!(v.verify_at(&totp_at(&key, c - 1), c, now), Verdict::Wrong);
        // the current step still works (higher counter)
        assert_eq!(v.verify_at(&totp_at(&key, c), c, now), Verdict::Ok);
        // next step works, two steps ahead does not
        assert_eq!(v.verify_at(&totp_at(&key, c + 1), c, now), Verdict::Ok);
        assert_eq!(v.verify_at(&totp_at(&key, c + 3), c, now), Verdict::Wrong);
        // "123 456" style input is tolerated
        let with_space = format!("{} {}", &totp_at(&key, c + 2)[..3], &totp_at(&key, c + 2)[3..]);
        assert_eq!(v.verify_at(&with_space, c + 1, now), Verdict::Ok);
    }

    #[test]
    fn locks_out_after_five_and_doubles() {
        let v = Verifier::new(Some(RFC_SECRET_B32.into()), None).with_state_path(tmp("lock.json"));
        let now = chrono::Utc::now();
        for _ in 0..4 {
            assert_eq!(v.verify_at("000000", 5, now), Verdict::Wrong);
        }
        assert_eq!(v.verify_at("000000", 5, now), Verdict::LockedOut(60));
        // while locked, even a right code is refused
        let key = base32_decode(RFC_SECRET_B32).unwrap();
        assert!(matches!(v.verify_at(&totp_at(&key, 5), 5, now), Verdict::LockedOut(_)));
        // after the lockout, one more wrong doubles it
        let later = now + chrono::Duration::seconds(61);
        assert_eq!(v.verify_at("000000", 5, later), Verdict::LockedOut(120));
        // a right code after it expires clears everything
        let later2 = later + chrono::Duration::seconds(121);
        assert_eq!(v.verify_at(&totp_at(&key, 6), 6, later2), Verdict::Ok);
        let st = State::load_from(&tmp_existing("lock.json"));
        assert_eq!(st.failures, 0);
        assert!(st.locked_until.is_none());
    }

    fn tmp_existing(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("ost-parentcode-{}", std::process::id()))
            .join(name)
    }

    #[test]
    fn backup_code_is_accepted_but_flagged() {
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(b"12345678", &salt)
            .unwrap()
            .to_string();
        let v = Verifier::new(Some(RFC_SECRET_B32.into()), Some(hash)).with_state_path(tmp("backup.json"));
        let now = chrono::Utc::now();
        assert_eq!(v.verify_at("12345678", 9, now), Verdict::Backup);
        assert_eq!(v.verify_at("87654321", 9, now), Verdict::Wrong);
        assert_eq!(event(&Verdict::Backup, "unlock", "kid").ev_type, EV_PARENT_CODE_BACKUP_USED);
        assert_eq!(event(&Verdict::Ok, "pam", "kid").ev_type, EV_PARENT_CODE_OK);
        assert_eq!(event(&Verdict::Wrong, "overlay", "kid").severity, SEV_WARN);
    }

    #[test]
    fn nothing_configured_never_opens() {
        let v = Verifier::new(None, Some("   ".into())).with_state_path(tmp("none.json"));
        assert_eq!(v.verify_at("123456", 1, chrono::Utc::now()), Verdict::NotConfigured);
        assert!(!v.configured());
    }
}
