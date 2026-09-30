//! The graphical lock's line to the agent: a root-owned Unix socket that only
//! the `ost-lock` user may talk to.
//!
//! The lock UI runs unprivileged and holds no secret. It sends what was typed;
//! the agent checks the caller's `SO_PEERCRED` uid is `ost-lock`, verifies the
//! code itself (offline, `parentcode`) and, if it is right, thaws and takes the
//! lock down. One JSON request per connection, one JSON reply, bounded.
//!
//! This replaces the old root-trusted `unlock_grant.<user>` file drop (the
//! verdict used to come *from* the drawing process, running as root) and the
//! `unlock_pin.<user>` drop nothing ever wrote.

use super::{current_face, mark_gui_seen, Face, LockEvent, LockTx, SharedRef};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

/// Longest request line accepted.
pub const MAX_LINE: u64 = 4096;

pub fn path() -> PathBuf {
    crate::paths::run("lock.sock")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// What should I show?
    Face,
    /// Someone typed a code.
    Code { code: String },
    /// "Ask for more time".
    Ask,
    /// "Give me 15 more minutes" — only for someone who set their own limits;
    /// the agent checks that, the wait and today's count, not the lock.
    Snooze,
    /// "Switch user": someone else wants the computer.
    SwitchUser,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub ok: bool,
    pub message: String,
    /// "Not yet — in N s …" ([`Outcome::too_soon`]): its number is the wait
    /// at the press, and the wait counts down on screen, so a lock says it
    /// with the count as it stands (`super::shown_message`), never frozen.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub too_soon: bool,
}

impl Outcome {
    pub fn yes(m: &str) -> Self {
        Outcome {
            ok: true,
            message: m.into(),
            too_soon: false,
        }
    }
    pub fn no(m: &str) -> Self {
        Outcome {
            ok: false,
            message: m.into(),
            too_soon: false,
        }
    }
    /// "Give me 15 more minutes" pressed `secs` before the wait is over.
    pub fn too_soon(secs: u64) -> Self {
        Outcome {
            too_soon: true,
            ..Outcome::no(&super::too_soon_words(secs))
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// The face to show now; `None` once the lock is down.
    #[serde(default)]
    pub face: Option<Face>,
    #[serde(default)]
    pub result: Option<Outcome>,
}

/// A request only the runner can answer, with where to send the answer.
pub struct Pending {
    pub req: Request,
    pub reply: oneshot::Sender<Reply>,
}

/// Only the lock user may speak here. Everyone else — including the person
/// the lock is for — is refused before a byte is read.
pub fn authorized(peer_uid: u32, lock_uid: u32) -> bool {
    peer_uid == lock_uid && lock_uid != 0
}

/// Bind the socket: root-owned, group `ost-lock`, `0660`.
pub fn bind(at: &std::path::Path, group: Option<u32>) -> std::io::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(at);
    let l = UnixListener::bind(at)?;
    if let Some(g) = group {
        std::os::unix::fs::chown(at, Some(0), Some(g))?;
    }
    std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o660))?;
    Ok(l)
}

/// Serve lock UIs until the listener fails. `face` requests are answered from
/// the shared state straight away; codes and asks go to the runner.
pub async fn serve(listener: UnixListener, lock_uid: u32, shared: SharedRef, tx: LockTx) {
    loop {
        let stream = match listener.accept().await {
            Ok((s, _)) => s,
            Err(e) => {
                tracing::warn!("lock socket accept failed: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let peer = stream.peer_cred().map(|c| c.uid()).ok();
        if !peer.is_some_and(|uid| authorized(uid, lock_uid)) {
            tracing::warn!("lock socket: refused a connection from uid {peer:?}");
            continue;
        }
        let shared = shared.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, shared, tx).await {
                tracing::debug!("lock socket request failed: {e}");
            }
        });
    }
}

async fn handle(stream: UnixStream, shared: SharedRef, tx: LockTx) -> std::io::Result<()> {
    let (r, mut w) = stream.into_split();
    let mut line = String::new();
    let read = tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(r.take(MAX_LINE)).read_line(&mut line),
    )
    .await;
    if !matches!(read, Ok(Ok(n)) if n > 0) {
        return Ok(());
    }
    if mark_gui_seen(&shared) {
        // The text lock is standing in for this one: wake the runner to
        // move the lock here.
        let _ = tx.try_send(LockEvent::GuiUp);
    }
    let reply = match serde_json::from_str::<Request>(line.trim()) {
        Ok(Request::Face) => Reply {
            face: current_face(&shared),
            result: None,
        },
        Ok(req) => {
            let (rtx, rrx) = oneshot::channel();
            let sent = tx
                .send(LockEvent::Request(Pending { req, reply: rtx }))
                .await
                .is_ok();
            let answer = if sent {
                tokio::time::timeout(Duration::from_secs(20), rrx)
                    .await
                    .ok()
                    .and_then(Result::ok)
            } else {
                None
            };
            answer.unwrap_or_else(|| Reply {
                face: current_face(&shared),
                result: Some(Outcome::no(
                    "Can't check that right now — try again in a moment.",
                )),
            })
        }
        Err(_) => Reply {
            face: current_face(&shared),
            result: None,
        },
    };
    let mut out = serde_json::to_vec(&reply).unwrap_or_default();
    out.push(b'\n');
    w.write_all(&out).await?;
    w.shutdown().await
}

/// The lock UI's side (blocking; runs as `ost-lock`): one request, one reply.
#[cfg_attr(not(feature = "gui"), allow(dead_code))] // the graphical lock's client
pub fn call(at: &std::path::Path, req: &Request) -> std::io::Result<Reply> {
    use std::io::{BufRead, Write};
    let mut s = std::os::unix::net::UnixStream::connect(at)?;
    s.set_read_timeout(Some(Duration::from_secs(25)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut body = serde_json::to_vec(req).map_err(std::io::Error::other)?;
    body.push(b'\n');
    s.write_all(&body)?;
    let mut line = String::new();
    std::io::BufReader::new(s).read_line(&mut line)?;
    serde_json::from_str(line.trim()).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{shared, Face};

    fn tmp_sock(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ost-lock-sock-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    #[test]
    fn only_the_lock_user_is_authorized() {
        assert!(authorized(975, 975));
        assert!(!authorized(1000, 975));
        // root never "is" the lock user, even if the lookup went wrong
        assert!(!authorized(0, 0));
    }

    /// End to end over a real socket: the peer uid the kernel reports decides
    /// who is heard. Our own uid stands in for `ost-lock`.
    #[tokio::test]
    async fn peer_credentials_gate_the_socket() {
        let me = users::get_current_uid();
        // Allowed: we are "the lock user".
        let at = tmp_sock("allowed.sock");
        let listener = bind(&at, None).unwrap();
        let sh = shared();
        let mut f = Face::waiting();
        f.title = "Bedtime until 07:00".into();
        crate::lock::with_shared(&sh, |s| s.face = Some(f.clone()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(serve(listener, me, sh.clone(), tx));
        // The runner: answer a code request.
        tokio::spawn(async move {
            while let Some(LockEvent::Request(p)) = rx.recv().await {
                let ok = matches!(&p.req, Request::Code { code } if code == "123456");
                let _ = p.reply.send(Reply {
                    face: None,
                    result: Some(if ok {
                        Outcome::yes("Unlocked")
                    } else {
                        Outcome::no("That code didn't work.")
                    }),
                });
            }
        });
        let at2 = at.clone();
        let face = tokio::task::spawn_blocking(move || call(&at2, &Request::Face))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(face.face.unwrap().title, "Bedtime until 07:00");
        let at2 = at.clone();
        let r = tokio::task::spawn_blocking(move || {
            call(
                &at2,
                &Request::Code {
                    code: "123456".into(),
                },
            )
        })
        .await
        .unwrap()
        .unwrap();
        assert!(r.result.unwrap().ok);
        assert!(crate::lock::with_shared(&sh, |s| s.ui_seen.is_some()));

        // Refused: the lock user is someone else, so our connection is dropped
        // without an answer.
        let at = tmp_sock("refused.sock");
        let listener = bind(&at, None).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let sh = shared();
        tokio::spawn(serve(listener, me.wrapping_add(4242), sh.clone(), tx));
        let r = tokio::task::spawn_blocking(move || call(&at, &Request::Face))
            .await
            .unwrap();
        assert!(r.is_err(), "a stranger must get nothing back");
        assert!(crate::lock::with_shared(&sh, |s| s.ui_seen.is_none()));
    }
}
