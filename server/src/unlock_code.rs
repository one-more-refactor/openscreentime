//! The keys to a managed computer: its **unlock code** (a 6-digit TOTP the
//! console shows and the agent verifies offline) and its one-time **recovery
//! codes**. Not account sign-in — the other direction: how a parent proves
//! themselves *at the child's computer*. The parent never holds the secret;
//! the server is the authenticator and the console only shows codes.
//!
//! The verifier has to read the secret: TOTP secrets are stored base32, not
//! hashed, because a one-way digest cannot generate the next code.

use chrono::Utc;
use hmac::{Hmac, Mac};
use rand::Rng;
use sha1::Sha1;

/// RFC 6238 defaults.
pub const TOTP_STEP: u64 = 30;
const TOTP_DIGITS: u32 = 6;

/// RFC 6238 over HMAC-SHA1 — the same shape the agent verifies offline.
pub fn totp_at(secret_b32: &str, counter: u64) -> Option<String> {
    let key = base32::decode(
        base32::Alphabet::Rfc4648 { padding: false },
        &secret_b32.to_uppercase(),
    )?;
    let mut mac = Hmac::<Sha1>::new_from_slice(&key).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let code = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]) % 10u32.pow(TOTP_DIGITS);
    Some(format!("{code:0width$}", width = TOTP_DIGITS as usize))
}

/// The code that is valid *right now* for a secret, and how many seconds it
/// has left. This is what the console shows a parent as a device's unlock
/// code: the server is the authenticator, the parent just reads.
pub fn current_totp(secret_b32: &str) -> Option<(String, u64)> {
    let now = Utc::now().timestamp() as u64;
    let code = totp_at(secret_b32, now / TOTP_STEP)?;
    Some((code, TOTP_STEP - now % TOTP_STEP))
}

/// Recovery-code MAC: hex HMAC-SHA256 over the ASCII digits of the code,
/// keyed by the device's decoded TOTP secret. The agent computes the same
/// (client `parentcode::recovery_mac`) to verify offline; the test vector
/// below is shared with the client's tests so the two never drift.
pub fn recovery_mac(secret_b32: &str, code: &str) -> Option<String> {
    let key = base32::decode(
        base32::Alphabet::Rfc4648 { padding: false },
        &secret_b32.to_uppercase(),
    )?;
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&key).ok()?;
    let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    mac.update(digits.as_bytes());
    Some(hex::encode(mac.finalize().into_bytes()))
}

/// A fresh 160-bit secret, base32.
pub fn gen_totp_secret() -> String {
    let bytes: [u8; 20] = rand::thread_rng().gen();
    base32::encode(base32::Alphabet::Rfc4648 { padding: false }, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 test vector: the ASCII secret "12345678901234567890" at
    /// T=59 (counter 1) is 287082 for SHA-1/6 digits.
    #[test]
    fn totp_matches_the_rfc_vector() {
        let secret = base32::encode(
            base32::Alphabet::Rfc4648 { padding: false },
            b"12345678901234567890",
        );
        assert_eq!(totp_at(&secret, 1).as_deref(), Some("287082"));
        assert_eq!(totp_at(&secret, 37037036).as_deref(), Some("081804"));
    }

    #[test]
    fn totp_rejects_a_bad_secret() {
        assert!(totp_at("not base32 !!", 1).is_none());
    }

    /// Shared with the client (`parentcode::recovery_mac` test): the two sides
    /// must agree byte-for-byte or no recovery code would ever open a door.
    #[test]
    fn recovery_mac_matches_the_shared_vector() {
        assert_eq!(
            recovery_mac("GEZDGNBVGY3TQOJQ", "12345678").as_deref(),
            Some("0008171f02a4c9c7b347dcc77ff65745007d09e8b442eef48f92de5f11e953cd")
        );
        // The space people type is not part of the message.
        assert_eq!(
            recovery_mac("GEZDGNBVGY3TQOJQ", "1234 5678"),
            recovery_mac("GEZDGNBVGY3TQOJQ", "12345678")
        );
        assert!(recovery_mac("not base32 !!", "12345678").is_none());
    }

    #[test]
    fn current_totp_counts_down_within_the_step() {
        let (code, left) = current_totp("GEZDGNBVGY3TQOJQ").unwrap();
        assert_eq!(code.len(), 6);
        assert!((1..=TOTP_STEP).contains(&left));
    }
}
