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
    Ok(())
}
