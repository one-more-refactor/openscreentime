//! Shared helpers: the dry-run-aware external-command executor. Every enforcement
//! module shells out through `Exec` so that `--dry-run` is honored in exactly one
//! place (TAMPER.md enforcement primitives are all external tools: nft, resolvectl,
//! loginctl, systemctl, ...).

use crate::config::AgentCtx;
use anyhow::{Context, Result};
use std::process::{Command, Stdio};
use std::sync::Arc;

#[derive(Clone)]
pub struct Exec {
    ctx: Arc<AgentCtx>,
}

impl Exec {
    pub fn new(ctx: Arc<AgentCtx>) -> Self {
        Exec { ctx }
    }

    /// Run `program args...`. Under `--dry-run`, logs the command and returns "" Ok.
    pub fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        if self.ctx.dry_run {
            tracing::info!(target: "dry_run", "WOULD RUN: {} {}", program, args.join(" "));
            return Ok(String::new());
        }
        let out = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("spawning {program}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("{} {} failed: {}", program, args.join(" "), stderr.trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// Run feeding `stdin_data` to the process stdin (used for `nft -f -`).
    pub fn run_with_stdin(&self, program: &str, args: &[&str], stdin_data: &str) -> Result<String> {
        use std::io::Write;
        if self.ctx.dry_run {
            tracing::info!(target: "dry_run", "WOULD RUN: {} {} <<EOF\n{}\nEOF", program, args.join(" "), stdin_data);
            return Ok(String::new());
        }
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning {program}"))?;
        child
            .stdin
            .take()
            .context("no stdin")?
            .write_all(stdin_data.as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("{} {} failed: {}", program, args.join(" "), stderr.trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// Read-only probe: returns stdout even on nonzero exit (for `loginctl show-*`).
    /// A spawn failure collapses to `""` — fine for callers where "couldn't ask"
    /// and "asked, got nothing" mean the same thing. Callers that would take
    /// *destructive* action on empty output must use [`Self::try_probe`]:
    /// "the firewall table is gone" and "fork() failed this tick" are not the
    /// same fact, and conflating them once escalated a transient spawn failure
    /// into a whole-device tamper lockdown.
    pub fn probe(&self, program: &str, args: &[&str]) -> String {
        self.try_probe(program, args).unwrap_or_default()
    }

    /// Like [`Self::probe`], but distinguishes "the command could not be run at
    /// all" (`None`) from "the command ran and this is its stdout" (`Some`,
    /// possibly empty, even on nonzero exit).
    pub fn try_probe(&self, program: &str, args: &[&str]) -> Option<String> {
        // Probes read state and are always safe to run, even under --dry-run.
        match Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            Ok(o) => Some(String::from_utf8_lossy(&o.stdout).to_string()),
            Err(e) => {
                tracing::warn!("probe {program} could not run: {e}");
                None
            }
        }
    }

    /// Write a file, honoring dry-run. Used for resolv.conf, dnsmasq confs, polkit rules.
    pub fn write_file(&self, path: &str, contents: &str) -> Result<()> {
        if self.ctx.dry_run {
            tracing::info!(target: "dry_run", "WOULD WRITE {} ({} bytes):\n{}", path, contents.len(), contents);
            return Ok(());
        }
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(path, contents).with_context(|| format!("writing {path}"))?;
        Ok(())
    }

    pub fn dry_run(&self) -> bool {
        self.ctx.dry_run
    }
}
