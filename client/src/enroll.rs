//! `enroll` subcommand: report identity + OS users to the server, receive
//! `device_id` + `device_token`, and persist them to the root-owned config.

use crate::client::{self, EnrollRequest};
use crate::config::AgentConfig;
use crate::sysusers;
use anyhow::Result;

pub async fn run(server: &str, token: &str) -> Result<()> {
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
    // Loud on purpose. This is the only time the PIN is ever shown, and it is
    // the only way back into this machine if it locks itself out with no
    // network — at which point nobody can look it up anywhere.
    if let Some(pin) = resp.recovery_pin.as_deref() {
        println!();
        println!("  ┌──────────────────────────────────────────────┐");
        println!("  │  RECOVERY PIN   {pin}                     │");
        println!("  └──────────────────────────────────────────────┘");
        println!("  Write this down NOW — it is shown once and stored only as a hash.");
        println!("  If this device ever locks you out with no network:");
        println!("      sudo sentinel-agent unlock --pin {pin} --minutes 60");
        println!();
    } else {
        // An older server that does not mint one. Say so rather than leaving
        // the operator assuming a recovery path exists.
        println!();
        println!("  WARNING: this server did not issue a recovery PIN.");
        println!("  If this device locks itself out, there is NO offline way back in");
        println!("  short of masking the systemd unit from the boot loader.");
        println!();
    }
    Ok(())
}
