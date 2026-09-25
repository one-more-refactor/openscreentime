//! Shared helpers: the dry-run-aware external-command executor. Every enforcement
//! module shells out through `Exec` so that `--dry-run` is honored in exactly one
//! place (TAMPER.md enforcement primitives are all external tools: nft, resolvectl,
//! loginctl, systemctl, ...).

use crate::config::AgentCtx;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Exec {
    ctx: Arc<AgentCtx>,
    /// A simulated machine, for tests: see [`Exec::simulated`].
    sim: Option<Arc<Sim>>,
}

/// A pretend machine behind a dry-run `Exec`: it records what the agent would
/// have done, answers probes from a table, and can lack programs — so "nft is
/// not installed" or "dnsmasq isn't running" can be tested without a VM.
#[derive(Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Sim {
    log: Mutex<Vec<String>>,
    missing: HashSet<String>,
    /// Installed, but fails when run (`nft` refusing a ruleset); a
    /// `write:<path>` entry makes writing that file fail.
    failing: HashSet<String>,
    probes: HashMap<String, String>,
}

/// Where a service's `PATH` usually reaches; used when `PATH` itself is unset.
const STANDARD_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// The absolute path of `program` on this machine: along `PATH`, then along
/// the standard directories (the sbin ones included — `runuser`, `nft` and
/// `useradd` live there on Debian, and a minimal `PATH` would miss them).
pub fn find_program(program: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if program.contains('/') {
        return Some(std::path::PathBuf::from(program));
    }
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':')
        .chain(STANDARD_PATH.split(':'))
        .filter(|d| !d.is_empty())
        .map(|d| std::path::Path::new(d).join(program))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

impl Exec {
    pub fn new(ctx: Arc<AgentCtx>) -> Self {
        Exec { ctx, sim: None }
    }

    /// A dry-run `Exec` over a pretend machine that lacks `missing` programs
    /// and answers `probes` (`("systemctl is-active dnsmasq", "active")`).
    /// Unlisted probes answer "" (the program ran, said nothing). Tests only.
    #[cfg(test)]
    pub fn simulated(missing: &[&str], probes: &[(&str, &str)]) -> Self {
        Exec {
            ctx: AgentCtx::new(true, false, 1),
            sim: Some(Arc::new(Sim {
                log: Mutex::new(Vec::new()),
                missing: missing.iter().map(|m| m.to_string()).collect(),
                failing: HashSet::new(),
                probes: probes
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            })),
        }
    }

    /// The same pretend machine, where these programs (or `write:<path>`
    /// writes) fail. Tests only.
    #[cfg(test)]
    pub fn failing(self, what: &[&str]) -> Self {
        let sim = self.sim.as_ref().expect("a simulated Exec");
        Exec {
            ctx: self.ctx.clone(),
            sim: Some(Arc::new(Sim {
                log: Mutex::new(Vec::new()),
                missing: sim.missing.clone(),
                failing: what.iter().map(|w| w.to_string()).collect(),
                probes: sim.probes.clone(),
            })),
        }
    }

    /// What the simulated machine was asked to do, in order ("run …",
    /// "write …", "remove …"). Empty for a real `Exec`.
    #[cfg(test)]
    pub fn log(&self) -> Vec<String> {
        self.sim
            .as_ref()
            .map(|s| s.log.lock().unwrap().clone())
            .unwrap_or_default()
    }

    fn record(&self, entry: String) {
        if let Some(sim) = &self.sim {
            sim.log.lock().unwrap().push(entry);
        }
    }

    /// A simulated program that isn't installed fails the way a real one
    /// does: at spawn, with NotFound.
    fn sim_missing(&self, program: &str) -> Result<()> {
        match &self.sim {
            Some(sim) if sim.missing.contains(program) => Err(anyhow::Error::from(
                std::io::Error::from(std::io::ErrorKind::NotFound),
            )
            .context(format!("spawning {program}"))),
            Some(sim) if sim.failing.contains(program) => {
                anyhow::bail!("{program} failed (simulated)")
            }
            _ => Ok(()),
        }
    }

    /// Is `program` installed here? A missing tool is a setup to report once
    /// (`enforcement_degraded`), never a tamper signal to repeat every tick.
    pub fn has(&self, program: &str) -> bool {
        match &self.sim {
            Some(sim) => !sim.missing.contains(program),
            None => find_program(program).is_some(),
        }
    }

    /// Run `program args...`. Under `--dry-run`, logs the command and returns "" Ok.
    pub fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        if self.ctx.dry_run {
            self.sim_missing(program)?;
            self.record(format!("run {} {}", program, args.join(" ")));
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
            self.sim_missing(program)?;
            self.record(format!("run {} {}", program, args.join(" ")));
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
        if let Some(sim) = &self.sim {
            if sim.missing.contains(program) {
                return None;
            }
            let key = format!("{} {}", program, args.join(" "));
            return Some(sim.probes.get(&key).cloned().unwrap_or_default());
        }
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

    /// Read a file (always — reading is safe under --dry-run too). On a
    /// simulated machine the answer is the probe `read <path>`, absent = None.
    pub fn read_file(&self, path: &str) -> Option<String> {
        if let Some(sim) = &self.sim {
            return sim.probes.get(&format!("read {path}")).cloned();
        }
        std::fs::read_to_string(path).ok()
    }

    /// Write a file, honoring dry-run. Used for resolv.conf, dnsmasq confs, polkit rules.
    pub fn write_file(&self, path: &str, contents: &str) -> Result<()> {
        if self.ctx.dry_run {
            if let Some(sim) = &self.sim {
                if sim.failing.contains(&format!("write:{path}")) {
                    anyhow::bail!("writing {path} failed (simulated)");
                }
            }
            self.record(format!("write {path}"));
            tracing::info!(target: "dry_run", "WOULD WRITE {} ({} bytes):\n{}", path, contents.len(), contents);
            return Ok(());
        }
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(path, contents).with_context(|| format!("writing {path}"))?;
        Ok(())
    }

    /// Remove a file we manage, honoring dry-run. `Ok(true)` if it was there
    /// (or, under dry-run, would have been removed); a missing file is `Ok(false)`.
    pub fn remove_file(&self, path: &str) -> Result<bool> {
        if self.sim.is_some() {
            self.record(format!("remove {path}"));
            return Ok(true);
        }
        if !std::path::Path::new(path).exists() {
            return Ok(false);
        }
        if self.ctx.dry_run {
            tracing::info!(target: "dry_run", "WOULD REMOVE {path}");
            return Ok(true);
        }
        match std::fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("removing {path}")),
        }
    }

    pub fn dry_run(&self) -> bool {
        self.ctx.dry_run
    }

    /// Do probes describe the machine being acted on? Yes for a real run and
    /// for a simulated machine; no for a plain `--dry-run`, which acts on
    /// nothing — there a missing resolver is not a finding.
    pub fn observes(&self) -> bool {
        !self.ctx.dry_run || self.sim.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_file_honors_dry_run_and_tolerates_absence() {
        let path = std::env::temp_dir().join(format!("ost-rm-{}", std::process::id()));
        let p = path.to_str().unwrap();
        std::fs::write(&path, "x").unwrap();
        // Dry-run says it would, and leaves the file where it is.
        let dry = Exec::new(AgentCtx::new(true, false, 1));
        assert!(dry.remove_file(p).unwrap());
        assert!(path.exists());
        // For real: gone, and a second remove is a quiet no-op.
        let real = Exec::new(AgentCtx::new(false, false, 1));
        assert!(real.remove_file(p).unwrap());
        assert!(!path.exists());
        assert!(!real.remove_file(p).unwrap());
    }

    #[test]
    fn a_simulated_machine_records_and_can_lack_programs() {
        let exec = Exec::simulated(&["nft"], &[("systemctl is-active dnsmasq", "active\n")]);
        assert!(!exec.has("nft"));
        assert!(exec.has("chattr"));
        let e = exec.run("nft", &["-f", "-"]).unwrap_err();
        assert_eq!(
            e.root_cause()
                .downcast_ref::<std::io::Error>()
                .map(|e| e.kind()),
            Some(std::io::ErrorKind::NotFound)
        );
        assert_eq!(exec.try_probe("nft", &["list", "tables"]), None);
        assert_eq!(
            exec.probe("systemctl", &["is-active", "dnsmasq"]),
            "active\n"
        );
        assert_eq!(exec.probe("systemctl", &["is-active", "nope"]), "");
        exec.run("chattr", &["+i", "/etc/resolv.conf"]).unwrap();
        exec.write_file("/etc/x", "y").unwrap();
        assert_eq!(
            exec.log(),
            vec!["run chattr +i /etc/resolv.conf", "write /etc/x"]
        );
    }

    #[test]
    fn programs_are_found_in_sbin_too() {
        // `sh` is everywhere; an absolute path is taken as given.
        assert!(find_program("sh").is_some());
        assert!(find_program("definitely-not-a-program-ost").is_none());
        assert_eq!(
            find_program("/usr/sbin/runuser"),
            Some(std::path::PathBuf::from("/usr/sbin/runuser"))
        );
    }
}
