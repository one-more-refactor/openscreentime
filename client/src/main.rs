//! openscreentime — the Linux client for the OpenScreenTime zero-trust device management
//! platform. Single binary; subcommands: enroll, run, install-service, status.
//!
//! Global safety flags (honored everywhere):
//!   --dry-run       log actions instead of executing them (safe as non-root)
//!   --tamper-max    raise the tamper ceiling to level 3 (opt-in, TAMPER.md)

mod attrib;
mod childcli;
mod client;
mod config;
mod earn;
mod enforce;
mod enroll;
#[cfg(feature = "gui")]
mod intro;
mod lockout;
mod login;
mod loginbroker;
mod pam;
mod parent;
mod parentcode;
mod paths;
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
    name = "openscreentime",
    bin_name = "ost",
    version,
    about = "OpenScreenTime — screen time for the whole family",
    long_about = "OpenScreenTime keeps track of screen time on this computer.\n\n\
                  Everyday commands need no special permissions:\n  \
                  ost time     how much is left today\n  \
                  ost ask      ask a parent for more\n  \
                  ost login    open the console, already signed in\n\n\
                  Every read command also takes --json.",
    after_help = "Setup and recovery need root: enroll, run, install-service, unlock."
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
    /// Enroll against a server with a one-time token; writes /etc/openscreentime/agent.toml.
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
    Status {
        #[arg(long)]
        json: bool,
    },
    /// How much screen time you have left today. Safe to run as yourself.
    Time {
        /// Machine-readable output, for scripts, status bars and assistants.
        #[arg(long)]
        json: bool,
    },
    /// Ask a parent for more time. Safe to run as yourself.
    Ask {
        #[arg(long)]
        json: bool,
    },
    /// Open the console in a browser, already signed in.
    ///
    /// Uses this computer's own enrollment as proof of identity: no password,
    /// no passkey prompt. The session can read everything; changing anything
    /// still asks for a second factor.
    Login {
        /// Print the sign-in URL instead of opening a browser (headless boxes,
        /// or opening it on another machine). stdout is the URL and nothing else.
        #[arg(long)]
        print_url: bool,
        #[arg(long)]
        json: bool,
    },
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
    /// Parent recovery: verify the unlock code (read it off the OpenScreenTime
    /// console, or use a recovery code) and suspend enforcement for a while
    /// (nft table + resolv.conf pin torn down, users un-frozen). Works
    /// offline. Requires root.
    Unlock {
        /// Optional. Prefer omitting it: anything passed here is visible in
        /// /proc/<pid>/cmdline to every local user, including the person this
        /// device constrains. Omit it and the code is read from the terminal.
        #[arg(long)]
        code: Option<String>,
        /// Old name for --code.
        #[arg(long, hide = true)]
        pin: Option<String>,
        /// How long to suspend enforcement for, in minutes.
        #[arg(long, default_value_t = 60)]
        minutes: u64,
    },
    /// Remove the systemd units, the sudo/PAM unlock-code hook and the
    /// `ost-managed` group. Leaves the enrollment config in place. Requires root.
    Uninstall,
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("openscreentime=info,info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

/// Read the unlock code from the terminal without echoing it and without ever
/// placing it on a command line. No new dependency: raw-mode toggling via
/// `stty` is enough for a one-shot prompt, and falls back to a plain read if
/// that fails.
fn rpassword_prompt() -> anyhow::Result<String> {
    use std::io::{BufRead, Write};
    eprint!("Unlock code (from the OpenScreenTime console, or a recovery code): ");
    std::io::stderr().flush().ok();
    let echo_off = std::process::Command::new("stty")
        .arg("-echo")
        .stdin(std::process::Stdio::inherit())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if echo_off {
        let _ = std::process::Command::new("stty")
            .arg("echo")
            .stdin(std::process::Stdio::inherit())
            .status();
        eprintln!();
    }
    read?;
    Ok(line.trim().to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();

    // PAM helper (`pam_exec … openscreentime pam-auth`): runs inside sudo's
    // PAM conversation, must stay quiet on stderr (it's the user's terminal)
    // and must never parse the rest of argv like a normal subcommand.
    if raw_args.get(1).map(String::as_str) == Some("pam-auth") {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("error"))
            .with_target(false)
            .init();
        return pam::run().await;
    }

    init_tracing();

    // Hidden internal helper spawned by `unlock` to auto-resume enforcement
    // after the suspend window elapses. Not a real subcommand (kept out of
    // --help / clap's Cmd enum) since it's an implementation detail, not
    // something an operator should invoke directly.
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
    let is_user_cmd = matches!(
        cli.cmd,
        Cmd::Tray
            | Cmd::Pair { .. }
            | Cmd::Time { .. }
            | Cmd::Ask { .. }
            | Cmd::Login { .. }
            | Cmd::Status { .. }
    );
    #[cfg(not(feature = "tray"))]
    let is_user_cmd = matches!(
        cli.cmd,
        Cmd::Pair { .. }
            | Cmd::Time { .. }
            | Cmd::Ask { .. }
            | Cmd::Login { .. }
            | Cmd::Status { .. }
    );
    if !ctx.is_root && !cli.dry_run && !is_user_cmd {
        tracing::warn!(
            "not running as root; enforcing subcommands will refuse (use --dry-run to simulate)"
        );
    }

    match cli.cmd {
        Cmd::Enroll { server, token } => enroll::run(&server, &token).await,
        Cmd::Run => {
            // Before any state is read: adopt whatever the previous product
            // name left behind, so an upgrade doesn't start the day with an
            // empty usage ledger (every child's spent time silently back to 0).
            paths::migrate_state_dir();
            let cfg = config::AgentConfig::load()
                .map_err(|e| anyhow::anyhow!("not enrolled? {e} (run `enroll` first)"))?;
            runner::run(ctx, cfg).await
        }
        Cmd::InstallService => service::install_service(ctx),
        Cmd::Status { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&service::status_json())?);
                Ok(())
            } else {
                service::status()
            }
        }
        Cmd::Time { json } => childcli::time(json),
        Cmd::Ask { json } => childcli::ask(json),
        Cmd::Login { print_url, json } => login::run(print_url, json).await,
        Cmd::Pair { server, token } => parent::pair(&server, &token),
        #[cfg(feature = "tray")]
        Cmd::Tray => tray::run(),
        Cmd::Unlock { code, pin, minutes } => {
            let code = match code.or(pin) {
                Some(p) => {
                    eprintln!(
                        "warning: --code on the command line is readable by any local user \
                         via /proc; next time omit it and type it when asked."
                    );
                    p
                }
                None => rpassword_prompt()?,
            };
            unlock::run(&ctx, &code, minutes).await
        }
        Cmd::Uninstall => service::uninstall(ctx),
    }
}
