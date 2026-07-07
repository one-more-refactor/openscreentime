//! Remote shell over the server-brokered reverse tunnel (TAMPER.md → "Remote SSH").
//!
//! The agent NEVER opens an inbound listener — it only ever dials OUT, preserving
//! the firewall's default-deny inbound stance. On an `ssh_open` command we spawn a
//! shell and multiplex its I/O as base64 `ssh_data` frames over the existing WS bus;
//! the server bridges those frames to an admin terminal.
//!
//! Skeleton honesty: this multiplexes a shell over pipes, not a full PTY (no
//! job-control / termios). The production path (`ssh -R` from the agent to the
//! broker's embedded sshd) is documented in the README; this exists to prove the
//! dial-out + WS-bridge shape end to end.

use crate::protocol::AgentFrame;
use anyhow::{Context, Result};
use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// A live remote-shell session bridged over the WS bus.
pub struct SshSession {
    pub session_id: String,
    input_tx: mpsc::Sender<Vec<u8>>,
    child: Child,
}

impl SshSession {
    /// Open a session: spawn the shell and start pumping its output to `out_tx`
    /// as `AgentFrame::SshData` frames. Returns the session handle.
    pub fn open(session_id: String, out_tx: mpsc::Sender<AgentFrame>) -> Result<SshSession> {
        let mut child = Command::new("/bin/bash")
            .arg("-i")
            .env("TERM", "xterm-256color")
            .env("SENTINEL_REMOTE_SHELL", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("spawning remote shell")?;

        let mut stdin = child.stdin.take().context("no stdin")?;
        let mut stdout = child.stdout.take().context("no stdout")?;
        let mut stderr = child.stderr.take().context("no stderr")?;

        let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(64);

        // server → shell: write incoming bytes to the shell's stdin.
        tokio::spawn(async move {
            while let Some(bytes) = input_rx.recv().await {
                if stdin.write_all(&bytes).await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // shell stdout → server
        let sid_out = session_id.clone();
        let tx_out = out_tx.clone();
        tokio::spawn(async move {
            pump(&mut stdout, &sid_out, tx_out).await;
        });

        // shell stderr → server
        let sid_err = session_id.clone();
        let tx_err = out_tx.clone();
        tokio::spawn(async move {
            pump(&mut stderr, &sid_err, tx_err.clone()).await;
            // When stderr closes we consider the session done.
            let _ = tx_err
                .send(AgentFrame::SshClose {
                    session_id: sid_err,
                })
                .await;
        });

        tracing::info!("ssh session {} opened (dial-out shell bridge)", session_id);
        Ok(SshSession {
            session_id,
            input_tx,
            child,
        })
    }

    /// Feed decoded bytes (from a server `ssh_data` frame) to the shell.
    pub async fn feed_b64(&self, data_b64: &str) {
        if let Ok(bytes) = B64.decode(data_b64) {
            let _ = self.input_tx.send(bytes).await;
        }
    }

    pub async fn close(mut self) {
        let _ = self.child.start_kill();
        tracing::info!("ssh session {} closed", self.session_id);
    }
}

async fn pump<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    session_id: &str,
    tx: mpsc::Sender<AgentFrame>,
) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let frame = AgentFrame::SshData {
                    session_id: session_id.to_string(),
                    data_b64: B64.encode(&buf[..n]),
                };
                if tx.send(frame).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// The documented production alternative, surfaced in logs / README.
pub fn production_reverse_tunnel_hint(broker_port: u16) -> String {
    format!(
        "production path: agent runs `ssh -R {broker_port}:localhost:22 broker@server` to the \
         broker's embedded sshd; admin connects via `ssh -p {broker_port} device@broker`. \
         Agent dials OUT only; no inbound listener is opened."
    )
}
