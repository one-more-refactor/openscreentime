//! The unlock code — a per-device TOTP (RFC 6238) verified **offline**, plus
//! one-time recovery codes.
//!
//! The server mints a secret per device and keeps it; the parent never holds
//! it. They read the *current* 6-digit code off the OpenScreenTime console
//! (after proving it's them), and this agent — which received the same secret
//! in the policy bundle (`parent_code.totp_secret`, cached root-only) —
//! verifies what gets typed with no server round-trip: the lockout overlay's
//! parent field, `ost unlock`, and the PAM helper that gates `sudo` on a
//! managed machine all go through [`Verifier::verify`].
//!
//! Rules (docs/CONTRACT-0.4.md §4, CONTRACT-0.5.md §1):
//! * SHA1, 6 digits, 30 s steps, ±1 step of clock drift.
//! * **Single-use**: the last accepted counter is persisted, so a code that
//!   was just typed cannot be replayed within its window by someone watching.
//! * Five wrong codes → 60 s lockout, doubling per further failure, capped at
//!   15 min. Persisted, so a restart does not reset the clock.
//! * **Recovery codes**: 8 digits, one-time, generated in the console and
//!   delivered here as `{id, mac}` with `mac = HMAC-SHA256(secret, digits)`.
//!   Accepted offline, remembered as spent in the state file, and reported as
//!   `parent_code_backup_used` with the id so the server retires it too.
//! * A profile-level **backup code** (`parent_pin_hash`, argon2) still opens
//!   the door when an admin set one deliberately; it is reported as such.
//!
//! Why per-device rather than the parent's account TOTP: extracting this
//! secret needs root on the device, and root on the device is already game
//! over *for that device* — it must not also be game over for the parent's
//! console account.

use crate::policy::RecoveryCode;
use crate::protocol::{Event, SEV_INFO, SEV_WARN};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::Sha1;
use std::path::{Path, PathBuf};

pub const STEP_SECS: u64 = 30;
pub const DIGITS: u32 = 6;
/// Recovery codes are 8 digits ("1234 5678" on the printout).
pub const RECOVERY_DIGITS: usize = 8;
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
    /// A fresh, in-window unlock code.
    Ok,
    /// A one-time recovery code (its server id). Still an unlock, reported so
    /// the console can retire the code and the parent sees it was used.
    Recovery(String),
    /// Matched a profile-level backup code (argon2 `parent_pin_hash`). Still
    /// an unlock, but the parent should hear the unlock code was not used.
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
        matches!(self, Verdict::Ok | Verdict::Recovery(_) | Verdict::Backup)
    }

    /// One line for a person at the keyboard.
    pub fn message(&self) -> String {
        match self {
            Verdict::Ok => "unlock code accepted".into(),
            Verdict::Recovery(_) => "recovery code accepted (it is now used up)".into(),
            Verdict::Backup => "backup code accepted (unlock code not used)".into(),
            Verdict::Wrong => "wrong code".into(),
            Verdict::LockedOut(s) => format!("too many wrong codes — try again in {s}s"),
            Verdict::NotConfigured => {
                "no unlock code is set up on this computer yet (the agent has not pulled one from the server)".into()
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
    /// Recovery codes already spent on this device (by server id), so one is
    /// single-use even while offline and before the server hears about it.
    #[serde(default)]
    pub used_recovery: Vec<String>,
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
            // 0600 from creation — this holds the TOTP secret; a write-then-
            // chmod left it briefly world-readable in the 0755 state dir.
            Ok(body) => {
                if let Err(e) = crate::config::write_private(path, body.as_bytes()) {
                    // warn, not debug: losing this is how a code becomes replayable.
                    tracing::warn!("could not persist parent-code state: {e}");
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

/// The recovery-code MAC: hex HMAC-SHA256 over the 8 ASCII digits, keyed by
/// the decoded TOTP secret. Must equal the server's `stepup::recovery_mac`
/// byte for byte (shared test vector below). Only the server *produces* MACs;
/// the agent only ever checks them (`recovery_matches`, constant-time), which
/// is why this lives in the tests.
#[cfg(test)]
pub fn recovery_mac(key: &[u8], digits: &str) -> String {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(digits.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Constant-time: does this code's MAC match the stored one?
fn recovery_matches(key: &[u8], digits: &str, stored_hex: &str) -> bool {
    let Ok(stored) = hex::decode(stored_hex.trim()) else {
        return false;
    };
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(digits.as_bytes());
    mac.verify_slice(&stored).is_ok()
}

fn now_counter() -> u64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs / STEP_SECS
}

/// Verifies unlock codes for this device.
#[derive(Debug, Clone)]
pub struct Verifier {
    /// Base32 TOTP secret (from the policy bundle's `parent_code`).
    secret: Option<String>,
    /// Unused recovery codes `{id, mac}` from the same bundle.
    recovery: Vec<RecoveryCode>,
    /// Argon2 PHC hash of a profile-level backup code.
    backup_hash: Option<String>,
    state_path: PathBuf,
}

impl Verifier {
    pub fn new(secret: Option<String>, backup_hash: Option<String>) -> Self {
        Verifier {
            secret: secret.filter(|s| !s.trim().is_empty()),
            recovery: Vec::new(),
            backup_hash: backup_hash.filter(|h| !h.trim().is_empty()),
            state_path: state_path(),
        }
    }

    /// Also accept these one-time recovery codes (needs the secret to check).
    pub fn with_recovery(mut self, codes: Vec<RecoveryCode>) -> Self {
        self.recovery = codes
            .into_iter()
            .filter(|c| !c.id.trim().is_empty() && !c.mac.trim().is_empty())
            .collect();
        self
    }

    /// The verifier for this device, from what the agent last cached: the
    /// bundle's `parent_code` (secret + recovery codes) plus any profile-level
    /// backup-code hash. Works with no agent process and no network.
    pub fn from_device() -> Self {
        let bundle = crate::policy::load_bundle_cache().ok();
        let secret = bundle
            .as_ref()
            .and_then(|b| b.parent_code.as_ref())
            .map(|p| p.totp_secret.clone());
        let recovery = bundle
            .as_ref()
            .and_then(|b| b.parent_code.as_ref())
            .map(|p| p.recovery_codes.clone())
            .unwrap_or_default();
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
        Self::new(secret, backup).with_recovery(recovery)
    }

    /// Recovery codes still usable on this device (the bundle's unused set
    /// minus what this device already spent offline).
    pub fn recovery_codes_left(&self) -> usize {
        let st = State::load_from(&self.state_path);
        self.recovery
            .iter()
            .filter(|c| !st.used_recovery.contains(&c.id))
            .count()
    }

    #[cfg(test)]
    pub(crate) fn with_state_path(mut self, p: PathBuf) -> Self {
        self.state_path = p;
        self
    }

    pub fn configured(&self) -> bool {
        self.secret.is_some() || self.backup_hash.is_some()
    }

    /// Check a typed code. Persists the replay counter and failure lockout.
    pub fn verify(&self, code: &str) -> Verdict {
        self.verify_at(code, now_counter(), chrono::Utc::now())
    }

    fn verify_at(&self, code: &str, counter: u64, now: chrono::DateTime<chrono::Utc>) -> Verdict {
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
        // Recovery codes: 8 digits, keyed MAC, single-use (spent ids persist).
        if verdict == Verdict::Wrong
            && code.len() == RECOVERY_DIGITS
            && code.chars().all(|c| c.is_ascii_digit())
        {
            if let Some(key) = self.secret.as_deref().and_then(base32_decode) {
                if let Some(hit) = self
                    .recovery
                    .iter()
                    .filter(|c| !st.used_recovery.contains(&c.id))
                    .find(|c| recovery_matches(&key, &code, &c.mac))
                {
                    st.used_recovery.push(hit.id.clone());
                    verdict = Verdict::Recovery(hit.id.clone());
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
            Verdict::Ok | Verdict::Recovery(_) | Verdict::Backup => {
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
/// `parent_code_backup_used` (warn; carries `recovery_id` when a recovery
/// code was spent, so the server retires it) / `parent_code_failed` (warn).
/// `via` is where it was typed: `"overlay"`, `"unlock"`, `"tray"`, `"pam"`.
pub fn event(verdict: &Verdict, via: &str, user: &str) -> Event {
    let (kind, sev, detail) = match verdict {
        Verdict::Ok => (EV_PARENT_CODE_OK, SEV_INFO, "unlock code accepted"),
        Verdict::Recovery(_) => (
            EV_PARENT_CODE_BACKUP_USED,
            SEV_WARN,
            "recovery code accepted — it is now used up",
        ),
        Verdict::Backup => (
            EV_PARENT_CODE_BACKUP_USED,
            SEV_WARN,
            "backup code accepted — the unlock code was not used",
        ),
        Verdict::Wrong => (EV_PARENT_CODE_FAILED, SEV_WARN, "wrong code"),
        Verdict::LockedOut(_) => (
            EV_PARENT_CODE_FAILED,
            SEV_WARN,
            "locked out after repeated wrong codes",
        ),
        Verdict::NotConfigured => (EV_PARENT_CODE_FAILED, SEV_WARN, "no unlock code configured"),
    };
    let mut payload = json!({ "via": via, "user": user, "detail": detail });
    if let Verdict::Recovery(id) = verdict {
        payload["recovery_id"] = json!(id);
    }
    let mut ev = Event::new(kind, sev, payload);
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
        let v =
            Verifier::new(Some(RFC_SECRET_B32.into()), None).with_state_path(tmp("replay.json"));
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
        let with_space = format!(
            "{} {}",
            &totp_at(&key, c + 2)[..3],
            &totp_at(&key, c + 2)[3..]
        );
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
        assert!(matches!(
            v.verify_at(&totp_at(&key, 5), 5, now),
            Verdict::LockedOut(_)
        ));
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
        let v = Verifier::new(Some(RFC_SECRET_B32.into()), Some(hash))
            .with_state_path(tmp("backup.json"));
        let now = chrono::Utc::now();
        assert_eq!(v.verify_at("12345678", 9, now), Verdict::Backup);
        assert_eq!(v.verify_at("87654321", 9, now), Verdict::Wrong);
        assert_eq!(
            event(&Verdict::Backup, "unlock", "kid").ev_type,
            EV_PARENT_CODE_BACKUP_USED
        );
        assert_eq!(event(&Verdict::Ok, "pam", "kid").ev_type, EV_PARENT_CODE_OK);
        assert_eq!(event(&Verdict::Wrong, "overlay", "kid").severity, SEV_WARN);
    }

    /// Shared with the server (`stepup::recovery_mac` test): both sides must
    /// produce this exact MAC or no recovery code would ever open a door.
    #[test]
    fn recovery_mac_matches_the_shared_vector() {
        let key = base32_decode("GEZDGNBVGY3TQOJQ").unwrap();
        assert_eq!(
            recovery_mac(&key, "12345678"),
            "0008171f02a4c9c7b347dcc77ff65745007d09e8b442eef48f92de5f11e953cd"
        );
    }

    #[test]
    fn recovery_code_opens_once_and_is_reported_by_id() {
        let key = base32_decode("GEZDGNBVGY3TQOJQ").unwrap();
        let codes = vec![
            RecoveryCode {
                id: "rc-a".into(),
                mac: recovery_mac(&key, "12345678"),
            },
            RecoveryCode {
                id: "rc-b".into(),
                mac: recovery_mac(&key, "87654321"),
            },
        ];
        let v = Verifier::new(Some("GEZDGNBVGY3TQOJQ".into()), None)
            .with_recovery(codes)
            .with_state_path(tmp("recovery.json"));
        let now = chrono::Utc::now();
        assert_eq!(v.recovery_codes_left(), 2);
        // "1234 5678" as printed
        assert_eq!(
            v.verify_at("1234 5678", 3, now),
            Verdict::Recovery("rc-a".into())
        );
        assert_eq!(v.recovery_codes_left(), 1);
        // spent: the same code is now just wrong, even offline
        assert_eq!(v.verify_at("12345678", 3, now), Verdict::Wrong);
        // the other one still works; the 6-digit TOTP path is untouched
        assert_eq!(
            v.verify_at("87654321", 3, now),
            Verdict::Recovery("rc-b".into())
        );
        assert_eq!(v.verify_at(&totp_at(&key, 4), 4, now), Verdict::Ok);
        // the event carries the id for the server to retire it
        let ev = event(&Verdict::Recovery("rc-a".into()), "pam", "kid");
        assert_eq!(ev.ev_type, EV_PARENT_CODE_BACKUP_USED);
        assert_eq!(ev.payload["recovery_id"], "rc-a");
        // a recovery code without the secret can never be checked
        let no_secret = Verifier::new(None, None)
            .with_recovery(vec![RecoveryCode {
                id: "x".into(),
                mac: recovery_mac(&key, "11112222"),
            }])
            .with_state_path(tmp("recovery-nosecret.json"));
        assert_eq!(
            no_secret.verify_at("11112222", 3, now),
            Verdict::NotConfigured
        );
    }

    #[test]
    fn nothing_configured_never_opens() {
        let v = Verifier::new(None, Some("   ".into())).with_state_path(tmp("none.json"));
        assert_eq!(
            v.verify_at("123456", 1, chrono::Utc::now()),
            Verdict::NotConfigured
        );
        assert!(!v.configured());
    }
}
