//! sentinel-agent — the Linux client for the Sentinel zero-trust device management
//! platform. Single binary; subcommands: enroll, run, install-service, status.
//!
//! Global safety flags (honored everywhere):
//!   --dry-run       log actions instead of executing them (safe as non-root)
//!   --tamper-max    raise the tamper ceiling to level 3 (opt-in, TAMPER.md)

mod client;
mod config;
mod discovery;
mod enforce;
mod enroll;
mod gamify;
mod lockout;
mod policy;
mod protocol;
mod runner;
mod service;
mod ssh;
mod sysusers;
mod tamper;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::AgentCtx;

#[derive(Parser)]
#[command(
    name = "sentinel-agent",
    version,
    about = "Sentinel zero-trust device agent"
)]
struct Cli {
    /// Log the enforcement actions that WOULD be taken, without touching the host.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Raise the tamper ceiling to level 3 (maximum lockdown; opt-in).
    #[arg(long, global = true)]
    tamper_max: bool,

    /// Accelerate screen-time accounting for local dev (e.g. 60 = 1 real sec is 1 min).
    #[arg(long, global = true, default_value_t = 1)]
    time_accel: u32,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Enroll against a server with a one-time token; writes /etc/sentinel/agent.toml.
    Enroll {
        #[arg(long)]
        server: String,
        #[arg(long)]
        token: String,
    },
    /// Run the main loop (WS bus + policy enforcement).
    Run,
    /// Install and enable the hardened systemd unit + watchdog + polkit rule.
    InstallService,
    /// Show enrollment / service status.
    Status,
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("sentinel_agent=info,info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let ctx = AgentCtx::new(cli.dry_run, cli.tamper_max, cli.time_accel);

    if cli.dry_run {
        tracing::info!("DRY-RUN: no host state will be modified");
    }
    if !ctx.is_root && !cli.dry_run {
        tracing::warn!(
            "not running as root; enforcing subcommands will refuse (use --dry-run to simulate)"
        );
    }

    match cli.cmd {
        Cmd::Enroll { server, token } => enroll::run(&server, &token).await,
        Cmd::Run => {
            let cfg = config::AgentConfig::load()
                .map_err(|e| anyhow::anyhow!("not enrolled? {e} (run `enroll` first)"))?;
            runner::run(ctx, cfg).await
        }
        Cmd::InstallService => service::install_service(ctx),
        Cmd::Status => service::status(),
    }
}
