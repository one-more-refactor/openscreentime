//! sentinel-agent — the Linux client for the Sentinel zero-trust device management
//! platform. Single binary; subcommands: enroll, run, install-service, status.
//!
//! Global safety flags (honored everywhere):
//!   --dry-run       log actions instead of executing them (safe as non-root)
//!   --tamper-max    raise the tamper ceiling to level 3 (opt-in, TAMPER.md)

mod childcli;
mod client;
mod config;
mod discovery;
mod enforce;
mod enroll;
mod gamify;
#[cfg(feature = "gui")]
mod intro;
mod lockout;
mod parent;
mod pin;
mod policy;
mod protocol;
mod runner;
mod service;
mod sysusers;
mod tamper;
#[cfg(feature = "tray")]
mod tray;
mod unlock;
mod update;
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
    /// How much screen time you have left today. Safe to run as yourself.
    Time,
    /// Ask a parent for more time. Safe to run as yourself.
    Ask,
    /// Pair this machine as a parent companion: store a scoped parent access
    /// token (minted in the web console → Settings → Parent access) so the tray
    /// can show + approve time requests. Runs as the desktop user (no root).
    Pair {
        #[arg(long)]
        server: String,
        #[arg(long)]
        token: String,
    },
    /// Per-user system tray companion: time left, connection state, and
    /// remote-shell transparency. With a parent pairing (`pair`), also shows and
    /// approves time requests. Runs as the desktop user (no root).
    #[cfg(feature = "tray")]
    Tray,
    /// Parent-PIN recovery: verify the PIN and suspend enforcement for a while
    /// (nft table + resolv.conf pin torn down, users un-frozen). Requires root.
    Unlock {
        #[arg(long)]
        pin: String,
        /// How long to suspend enforcement for, in minutes.
        #[arg(long, default_value_t = 60)]
        minutes: u64,
    },
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

    // Hidden internal helper spawned by `unlock` to auto-resume enforcement
    // after the suspend window elapses. Not a real subcommand (kept out of
    // --help / clap's Cmd enum) since it's an implementation detail, not
    // something an operator should invoke directly.
    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.get(1).map(String::as_str) == Some("__resume-enforcement") {
        let secs: u64 = raw_args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3600);
        return unlock::resume_after(secs);
    }

    // Hidden GUI presenter subprocess (spawned detached by the runner so the
    // blocking egui event loop never stalls the enforcement tick). Reads the
    // root-only staged LockSpec file whose path is the argument (never the spec
    // itself — it carries the parent-PIN hash, which must not sit on argv), shows
    // the overlay, and writes an unlock grant on a verified dismissal.
    if raw_args.get(1).map(String::as_str) == Some("__lockout") {
        #[cfg(feature = "gui")]
        {
            let spec_path = raw_args.get(2).map(String::as_str).unwrap_or("");
            return lockout::gui::run_from_spec_file(spec_path);
        }
        #[cfg(not(feature = "gui"))]
        {
            anyhow::bail!("__lockout requires a build with --features gui");
        }
    }

    // Hidden first-run intro subprocess (spawned detached by the tray on first
    // launch). Shows the skippable child-facing cards, then marks itself seen.
    if raw_args.get(1).map(String::as_str) == Some("__intro") {
        #[cfg(feature = "gui")]
        {
            return intro::run();
        }
        #[cfg(not(feature = "gui"))]
        {
            anyhow::bail!("__intro requires a build with --features gui");
        }
    }

    let cli = Cli::parse();
    let ctx = AgentCtx::new(cli.dry_run, cli.tamper_max, cli.time_accel);

    if cli.dry_run {
        tracing::info!("DRY-RUN: no host state will be modified");
    }
    // The tray companion and `pair` run as the desktop user on purpose — no root nag.
    #[cfg(feature = "tray")]
    let is_user_cmd = matches!(cli.cmd, Cmd::Tray | Cmd::Pair { .. } | Cmd::Time | Cmd::Ask);
    #[cfg(not(feature = "tray"))]
    let is_user_cmd = matches!(cli.cmd, Cmd::Pair { .. } | Cmd::Time | Cmd::Ask);
    if !ctx.is_root && !cli.dry_run && !is_user_cmd {
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
        Cmd::Time => childcli::time(),
        Cmd::Ask => childcli::ask(),
        Cmd::Pair { server, token } => parent::pair(&server, &token),
        #[cfg(feature = "tray")]
        Cmd::Tray => tray::run(),
        Cmd::Unlock { pin, minutes } => unlock::run(&ctx, &pin, minutes),
    }
}
