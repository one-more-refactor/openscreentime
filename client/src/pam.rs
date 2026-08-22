//! `openscreentime pam-auth` — the PAM half of "sudo on a managed machine asks
//! for the parent's authenticator code" (docs/CONTRACT-0.4.md §8).
//!
//! Wired from `/etc/pam.d/openscreentime-parent` as
//! `auth required pam_exec.so expose_authtok quiet …/openscreentime pam-auth`,
//! and selected for managed users by a sudoers `Defaults:%ost-managed
//! pam_service=openscreentime-parent`. pam_exec runs us with the privileges of
//! the calling process — sudo is setuid root — so the root-only bundle cache
//! (TOTP secret, backup hash) is readable, and we write the replay counter.
//!
//! Exit 0 = accept, 1 = refuse. Nothing is printed (pam_exec's `quiet` hides
//! our stdout anyway; the user sees sudo's own "Sorry, try again.").

use crate::parentcode::{self, Verifier};
use anyhow::Result;
use std::io::Read;

/// What pam_exec hands us on stdin: the token, NUL-terminated in current
/// Linux-PAM, newline-terminated in older builds. Take up to the first of either.
pub fn token_from_stdin_bytes(raw: &[u8]) -> String {
    let end = raw
        .iter()
        .position(|b| *b == 0 || *b == b'\n')
        .unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_string()
}

pub async fn run() -> Result<()> {
    let mut raw = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut raw);
    let code = token_from_stdin_bytes(&raw);
    let user = std::env::var("PAM_USER").unwrap_or_default();
    let service = std::env::var("PAM_SERVICE").unwrap_or_default();
    let rhost = std::env::var("PAM_RHOST").unwrap_or_default();

    let verifier = Verifier::from_device();
    let verdict = if code.is_empty() {
        parentcode::Verdict::Wrong
    } else {
        verifier.verify(&code)
    };

    // Audit trail, best-effort: a sudo that worked — or did not — on a child's
    // machine is exactly what a parent wants in the feed. Never block PAM on
    // the network: short timeout, failure only logged.
    let mut ev = parentcode::event(&verdict, "pam", &user);
    if let Some(obj) = ev.payload.as_object_mut() {
        obj.insert("pam_service".into(), service.clone().into());
        if !rhost.is_empty() {
            obj.insert("rhost".into(), rhost.into());
        }
    }
    if let Ok(cfg) = crate::config::AgentConfig::load() {
        if let Ok(client) = crate::client::ServerClient::new(&cfg.server_url, &cfg.device_token) {
            let post = client.post_events(std::slice::from_ref(&ev));
            if let Err(e) = tokio::time::timeout(std::time::Duration::from_secs(5), post).await {
                tracing::debug!("pam-auth event not delivered: {e}");
            }
        }
    }

    if verdict.accepted() {
        tracing::info!("pam-auth: {} for {user} via {service}", verdict.message());
        Ok(())
    } else {
        tracing::info!("pam-auth refused for {user} via {service}: {}", verdict.message());
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_stops_at_nul_or_newline() {
        assert_eq!(token_from_stdin_bytes(b"123456\0garbage"), "123456");
        assert_eq!(token_from_stdin_bytes(b"123456\n"), "123456");
        assert_eq!(token_from_stdin_bytes(b"  123 456 "), "123 456");
        assert_eq!(token_from_stdin_bytes(b""), "");
    }
}
