//! `install-service` and `status` subcommands. Installs the hardened systemd unit,
//! the watchdog timer, and the polkit rule (TAMPER.md level 1), then enables them.

use crate::config::{AgentConfig, AgentCtx};
use crate::tamper;
use crate::util::Exec;
use anyhow::Result;
use std::sync::Arc;

const UNIT: &str = include_str!("../systemd/sentinel-agent.service");
const WATCHDOG_SERVICE: &str = include_str!("../systemd/sentinel-watchdog.service");
const WATCHDOG_TIMER: &str = include_str!("../systemd/sentinel-watchdog.timer");

const UNIT_PATH: &str = "/etc/systemd/system/sentinel-agent.service";
const WATCHDOG_SVC_PATH: &str = "/etc/systemd/system/sentinel-watchdog.service";
const WATCHDOG_TIMER_PATH: &str = "/etc/systemd/system/sentinel-watchdog.timer";
const BIN_TARGET: &str = "/usr/local/bin/sentinel-agent";

pub fn install_service(ctx: Arc<AgentCtx>) -> Result<()> {
    ctx.require_root_for_enforcement()?;
    let exec = Exec::new(ctx.clone());

    // Copy our own binary into place so ExecStart path is stable.
    if let Ok(self_exe) = std::env::current_exe() {
        let self_exe = self_exe.to_string_lossy().to_string();
        if self_exe != BIN_TARGET {
            let _ = exec.run("install", &["-m", "0755", &self_exe, BIN_TARGET]);
        }
    }

    exec.write_file(UNIT_PATH, UNIT)?;
    exec.write_file(WATCHDOG_SVC_PATH, WATCHDOG_SERVICE)?;
    exec.write_file(WATCHDOG_TIMER_PATH, WATCHDOG_TIMER)?;
    tamper::install_polkit(&exec, 1)?;

    exec.run("systemctl", &["daemon-reload"])?;
    exec.run("systemctl", &["enable", "--now", "sentinel-agent.service"])?;
    exec.run("systemctl", &["enable", "--now", "sentinel-watchdog.timer"])?;

    tracing::info!("hardened unit + watchdog + polkit installed and enabled");
    println!("Installed sentinel-agent.service (hardened) + watchdog timer.");
    Ok(())
}

pub fn status() -> Result<()> {
    match AgentConfig::load() {
        Ok(cfg) => {
            println!("ENROLLED");
            println!("  server      {}", cfg.server_url);
            println!("  device_id   {}", cfg.device_id);
            println!("  tamper      level {}", cfg.tamper_level);
            println!("  poll        {}s", cfg.poll_interval_secs);
        }
        Err(_) => {
            println!(
                "NOT ENROLLED (no {} — run `enroll`)",
                crate::config::CONFIG_PATH
            );
        }
    }
    println!("  root        {}", crate::config::is_root());
    // Best-effort service state (read-only; safe even non-root).
    let out = std::process::Command::new("systemctl")
        .args(["is-active", "sentinel-agent.service"])
        .output();
    if let Ok(o) = out {
        println!(
            "  service     {}",
            String::from_utf8_lossy(&o.stdout).trim()
        );
    }
    Ok(())
}
