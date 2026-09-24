//! Parent-PIN verification (the escape hatch for "can't connect the devices").
//!
//! The server hashes the parent's PIN with `Argon2::default()` into a
//! self-describing PHC string (`policy.parent_pin_hash`); verification here is
//! param-agnostic — it just needs the hash, never the plaintext PIN, and works
//! fully offline (no server round-trip). Checked (as the backup code) for a
//! code typed at the lock, at `ost unlock` and at `sudo`.

use argon2::{Argon2, PasswordHash, PasswordVerifier};

/// Verify `pin` against a stored argon2 PHC hash. Returns `false` (never panics
/// or errors out) on a malformed hash or a wrong PIN — both are just "no".
pub fn verify_pin(pin: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(pin.as_bytes(), &parsed)
            .is_ok(),
        Err(e) => {
            tracing::warn!("parent_pin_hash is not a valid PHC string: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};

    fn hash_of(pin: &str) -> String {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(pin.as_bytes(), &salt)
            .expect("hash")
            .to_string()
    }

    #[test]
    fn known_hash_verifies() {
        let hash = hash_of("1234");
        assert!(verify_pin("1234", &hash));
        assert!(!verify_pin("4321", &hash));
    }

    #[test]
    fn malformed_hash_never_verifies() {
        assert!(!verify_pin("1234", "not-a-real-hash"));
    }
}
