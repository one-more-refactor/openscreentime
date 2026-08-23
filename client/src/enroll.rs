//! `enroll` subcommand: report identity + OS users to the server, receive
//! `device_id` + `device_token`, and persist them to the root-owned config.

use crate::client::{self, EnrollRequest};
use crate::config::AgentConfig;
use crate::sysusers;
use anyhow::Result;

/// A plaintext `server_url` makes the self-updater's sha256 check decorative:
/// an on-path attacker (open Wi-Fi, LAN ARP-spoof) controls both the manifest
/// and the bytes it hashes, so the check always "passes". Refuse `http://`
/// except to loopback/`.local`, where it's a legitimate dev/LAN setup.
fn ensure_secure_server(server: &str) -> Result<()> {
    let lower = server.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = lower.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        let local = host == "localhost"
            || host == "127.0.0.1"
            || host == "::1"
            || host == "[::1]"
            || host.ends_with(".local");
        if local {
            tracing::warn!("enrolling over plaintext http:// to {host} (local only)");
            return Ok(());
        }
    }
    anyhow::bail!(
        "refusing to enroll against a non-https server URL ({server}): plaintext \
         transport defeats update verification. Use https://, or a loopback/.local \
         host for local testing."
    )
}

pub async fn run(server: &str, token: &str) -> Result<()> {
    ensure_secure_server(server)?;
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let os_users = sysusers::login_users();
    tracing::info!(
        "enrolling {} against {} with {} OS user(s)",
        hostname,
        server,
        os_users.len()
    );

    let req = EnrollRequest {
        enroll_token: token.to_string(),
        hostname,
        os: "linux".to_string(),
        agent_version: client::AGENT_VERSION.to_string(),
        os_users,
    };

    let resp = client::enroll(server, &req).await?;
    tracing::info!("enrolled: device_id={}", resp.device_id);

    let cfg = AgentConfig {
        server_url: server.trim_end_matches('/').to_string(),
        device_id: resp.device_id,
        device_token: resp.device_token,
        poll_interval_secs: resp.poll_interval_secs,
        tamper_level: 1,
        auto_update: true,
    };
    cfg.save()?;
    tracing::info!("wrote {} (0600)", crate::config::CONFIG_PATH);
    println!("Enrolled. Config written to {}", crate::config::CONFIG_PATH);
    // Nothing to write down: the keys to this computer live in the console.
    // The unlock code (6 digits, changes every 30 s) and the one-time recovery
    // codes are read there after a step-up, and verified here offline.
    println!();
    println!("  Unlock code: open the OpenScreenTime console → this computer → Unlock code.");
    println!("  It opens the lock screen, `sudo`, and `sudo ost unlock` — no internet needed.");
    println!("  Recovery codes (for when your phone is not around) are generated in the");
    println!("  same place; generate a set now and keep it somewhere safe.");
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_secure_server;

    #[test]
    fn https_is_accepted() {
        assert!(ensure_secure_server("https://ost.example.com").is_ok());
    }

    #[test]
    fn loopback_http_is_allowed_for_dev() {
        assert!(ensure_secure_server("http://localhost:8080").is_ok());
        assert!(ensure_secure_server("http://127.0.0.1:8080").is_ok());
        assert!(ensure_secure_server("http://box.local").is_ok());
    }

    #[test]
    fn public_http_is_rejected() {
        assert!(ensure_secure_server("http://ost.example.com").is_err());
        assert!(ensure_secure_server("http://203.0.113.7:8080").is_err());
    }
}
