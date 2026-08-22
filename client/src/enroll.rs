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
    // The parent code for this computer lives in the parent's authenticator
    // app (scanned from the console when the device was added). This PIN is
    // the BACKUP code: the one way in if the phone is gone too. Shown once.
    if let Some(pin) = resp.recovery_pin.as_deref() {
        println!();
        println!("  Parent code: scan the QR shown in the console (Add device → Parent code)");
        println!("  into your authenticator app. It unlocks this computer offline.");
        println!();
        println!("  ┌──────────────────────────────────────────────┐");
        println!("  │  BACKUP CODE    {pin}                     │");
        println!("  └──────────────────────────────────────────────┘");
        println!("  Write this down NOW — it is shown once and stored only as a hash.");
        println!("  If this device ever locks you out and the authenticator is lost:");
        // NOT `--pin {pin}`: arguments land in /proc/<pid>/cmdline, which is
        // world-readable, so any local user — including the child this is meant
        // to constrain — can capture the plaintext PIN with a five-line loop
        // the first time a parent uses the documented recovery path. The same
        // reasoning already governs the PIN *hash* elsewhere in this codebase.
        println!("      sudo ost unlock --minutes 60      (it will ask for the parent or backup code)");
        println!();
    } else {
        // An older server that does not mint one. Say so rather than leaving
        // the operator assuming a recovery path exists.
        println!();
        println!("  WARNING: this server did not issue a backup code.");
        println!("  If this device locks itself out, there is NO offline way back in");
        println!("  short of masking the systemd unit from the boot loader.");
        println!();
    }
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
