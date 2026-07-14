//! Remote shell over the server-brokered reverse tunnel (CONTRACT-PROD.md §3).
//!
//! The agent NEVER opens an inbound listener — it only ever dials OUT, preserving
//! the firewall's default-deny inbound stance. On an `ssh_open` command we allocate
//! a real PTY, spawn `/bin/bash -l` on the slave, and bridge master ↔ WS as base64
//! `ssh_data` frames over the existing WS bus; the server bridges those frames to an
//! admin terminal. `ssh_resize` is applied via `TIOCSWINSZ`; on child exit we send
//! `ssh_closed` with the exit code.
//!
//! The production alternative (`ssh -R` from the agent to the broker's embedded
//! sshd) is documented in the README/`production_reverse_tunnel_hint` below; this
//! PTY bridge is the shape actually wired end to end per the contract.

use crate::protocol::AgentFrame;
use anyhow::{Context, Result};
use base64::Engine;
use nix::pty::openpty;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// A live remote-shell session, PTY-backed and bridged over the WS bus.
pub struct SshSession {
    pub session_id: String,
    /// Keystrokes from the server, forwarded to a blocking writer thread.
    input_tx: std_mpsc::Sender<Vec<u8>>,
    /// Kept open for `TIOCSWINSZ` resize ioctls and closed when the session drops.
    master: OwnedFd,
    /// The shell's pid, signaled on explicit `ssh_close`.
    pid: Pid,
}

impl SshSession {
    /// Open a session: allocate a PTY, spawn `/bin/bash -l` on the slave, and start
    /// pumping master output to `out_tx` as `AgentFrame::SshData` frames. Returns
    /// the session handle.
    pub fn open(session_id: String, out_tx: mpsc::Sender<AgentFrame>) -> Result<SshSession> {
        let pty = openpty(None, None).context("openpty")?;
        let master = pty.master;
        let slave = pty.slave;
        let slave_raw: RawFd = slave.as_raw_fd();

        // Independent fds for the child's stdio (dup'd so the child owns its own
        // references; our `slave` is dropped once spawned).
        let child_stdin = dup_fd(slave_raw)?;
        let child_stdout = dup_fd(slave_raw)?;
        let child_stderr = dup_fd(slave_raw)?;

        let mut cmd = Command::new("/bin/bash");
        cmd.arg("-l")
            .env("TERM", "xterm-256color")
            .env("SENTINEL_REMOTE_SHELL", "1")
            .stdin(unsafe { Stdio::from_raw_fd(child_stdin) })
            .stdout(unsafe { Stdio::from_raw_fd(child_stdout) })
            .stderr(unsafe { Stdio::from_raw_fd(child_stderr) });

        // Detach into our own session and make the pty slave our controlling tty
        // (needed for job control / signals to behave like a real terminal).
        unsafe {
            cmd.pre_exec(move || {
                nix::unistd::setsid().map_err(nix_to_io)?;
                let rc = libc::ioctl(slave_raw, libc::TIOCSCTTY as _, 0);
                if rc != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = cmd.spawn().context("spawning /bin/bash -l on pty slave")?;
        // Our copy of the slave is no longer needed; the child holds its own via
        // the dup'd stdio fds above.
        drop(slave);
        let pid = Pid::from_raw(child.id() as i32);

        let master_raw = master.as_raw_fd();
        let reader_fd = dup_fd(master_raw)?;
        let writer_fd = dup_fd(master_raw)?;

        let (input_tx, input_rx) = std_mpsc::channel::<Vec<u8>>();

        // server → shell: a dedicated blocking thread writes keystrokes to the master.
        std::thread::spawn(move || {
            let mut wfile = unsafe { std::fs::File::from_raw_fd(writer_fd) };
            while let Ok(bytes) = input_rx.recv() {
                if wfile.write_all(&bytes).is_err() {
                    break;
                }
            }
        });

        // shell → server: read master output, forward as ssh_data, then reap the
        // child and send ssh_closed with its exit code.
        let sid = session_id.clone();
        std::thread::spawn(move || {
            let mut rfile = unsafe { std::fs::File::from_raw_fd(reader_fd) };
            let mut buf = [0u8; 4096];
            loop {
                match rfile.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let frame = AgentFrame::SshData {
                            session_id: sid.clone(),
                            data_b64: B64.encode(&buf[..n]),
                        };
                        if out_tx.blocking_send(frame).is_err() {
                            break;
                        }
                    }
                }
            }
            let exit_code = child.wait().ok().and_then(|s| s.code());
            let _ = out_tx.blocking_send(AgentFrame::SshClosed {
                session_id: sid,
                exit_code,
            });
        });

        tracing::info!("ssh session {} opened (pty bridge)", session_id);
        Ok(SshSession {
            session_id,
            input_tx,
            master,
            pid,
        })
    }

    /// Feed decoded bytes (from a server `ssh_data` frame) to the shell.
    pub async fn feed_b64(&self, data_b64: &str) {
        if let Ok(bytes) = B64.decode(data_b64) {
            let _ = self.input_tx.send(bytes);
        }
    }

    /// Apply a server `ssh_resize` frame via `TIOCSWINSZ` on the PTY master.
    pub fn resize(&self, cols: u16, rows: u16) {
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let rc = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
        if rc != 0 {
            tracing::warn!(
                "ssh session {} resize failed: {}",
                self.session_id,
                std::io::Error::last_os_error()
            );
        }
    }

    /// Close the session: signal the shell. The reader thread detects the resulting
    /// EOF, reaps the child, and sends `ssh_closed` on its own.
    pub async fn close(self) {
        if let Err(e) = signal::kill(self.pid, Signal::SIGHUP) {
            tracing::debug!(
                "ssh session {} kill failed (already gone?): {e}",
                self.session_id
            );
        }
        tracing::info!("ssh session {} closed", self.session_id);
    }
}

fn dup_fd(fd: RawFd) -> Result<RawFd> {
    nix::unistd::dup(fd).context("dup pty fd")
}

fn nix_to_io(e: nix::Error) -> std::io::Error {
    std::io::Error::from_raw_os_error(e as i32)
}

/// The documented production alternative, surfaced in logs / README.
pub fn production_reverse_tunnel_hint(broker_port: u16) -> String {
    format!(
        "production path: agent runs `ssh -R {broker_port}:localhost:22 broker@server` to the \
         broker's embedded sshd; admin connects via `ssh -p {broker_port} device@broker`. \
         Agent dials OUT only; no inbound listener is opened."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};

    /// End-to-end smoke test of the PTY bridge: open a session, feed it a
    /// command over the base64 `ssh_data` path, and observe the echoed output
    /// come back through `out_tx` — proves the real PTY (not a plain-pipe
    /// shell) actually runs an interactive bash and round-trips bytes.
    #[tokio::test]
    async fn pty_session_echoes_command_output() {
        let (out_tx, mut out_rx) = mpsc::channel(64);
        let session = SshSession::open("test-session".to_string(), out_tx)
            .expect("opening a pty session should succeed in a normal dev/CI sandbox");

        session
            .feed_b64(&B64.encode(b"echo SENTINEL_PTY_OK\n"))
            .await;

        let mut seen = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !seen.contains("SENTINEL_PTY_OK") && std::time::Instant::now() < deadline {
            match timeout(Duration::from_secs(5), out_rx.recv()).await {
                Ok(Some(AgentFrame::SshData { data_b64, .. })) => {
                    if let Ok(bytes) = B64.decode(&data_b64) {
                        seen.push_str(&String::from_utf8_lossy(&bytes));
                    }
                }
                Ok(Some(_)) => {}
                _ => break,
            }
        }
        assert!(
            seen.contains("SENTINEL_PTY_OK"),
            "expected echoed output over the pty bridge, got: {seen:?}"
        );

        // Resize shouldn't error even though nothing reads winsize back here.
        session.resize(120, 40);
        session.close().await;
    }
}
