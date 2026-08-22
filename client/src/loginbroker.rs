//! The login broker — how `ost login` works for a user who is not root.
//!
//! The device token lives in a root-only config, so the desktop user cannot
//! mint a voucher themselves. Instead they drop a request file in
//! `/run/openscreentime/login/` (sticky, world-writable, like /tmp) and the
//! root agent — which is already running and holds the token — answers with a
//! one-time voucher URL in a file only that user can read. No socket, no
//! setuid, no daemon API: two files and a rename.
//!
//! Spoofing is closed by ownership: the agent only honours `<user>.req` when
//! the file is owned by the uid of `<user>`, and the answer `<user>.url` is
//! created `0600` and chowned to that uid before it is renamed into place. A
//! voucher is bound server-side to the account behind that OS user, so even a
//! stolen URL only ever signs in as the person whose login requested it.

use crate::client::ServerClient;
use crate::config::AgentConfig;
use anyhow::{Context, Result};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn dir() -> PathBuf {
    crate::paths::run("login")
}

/// Called by the root agent at startup: make the drop-box.
pub fn ensure_dir() {
    let d = dir();
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o1733));
}

/// The user-side half: ask the agent for a sign-in URL and wait for it.
pub async fn request_url(user: &str) -> Result<String> {
    let d = dir();
    if !d.is_dir() {
        anyhow::bail!(
            "the OpenScreenTime agent isn't running on this computer (no {}), so it can't sign you in",
            d.display()
        );
    }
    let req = d.join(format!("{user}.req"));
    let ans = d.join(format!("{user}.url"));
    let _ = std::fs::remove_file(&ans);
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&req)
        .with_context(|| format!("could not write {}", req.display()))?;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Ok(url) = std::fs::read_to_string(&ans) {
            let _ = std::fs::remove_file(&ans);
            let url = url.trim().to_string();
            if url.starts_with("error:") {
                anyhow::bail!("{}", url.trim_start_matches("error:").trim());
            }
            return Ok(url);
        }
    }
    let _ = std::fs::remove_file(&req);
    anyhow::bail!("the agent didn't answer in time — is it running? (`systemctl status openscreentime-agent`)")
}

/// The agent-side half: a small loop that answers requests. Runs as root.
pub async fn serve(cfg: AgentConfig, client: ServerClient, dry_run: bool) {
    if dry_run {
        return;
    }
    ensure_dir();
    let d = dir();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(user) = name.strip_suffix(".req") else {
                continue;
            };
            let path = e.path();
            let answer = answer_one(&cfg, &client, &d, user, &path).await;
            let _ = std::fs::remove_file(&path);
            if let Err(err) = answer {
                tracing::warn!("login request from {user}: {err:#}");
            }
        }
    }
}

async fn answer_one(
    cfg: &AgentConfig,
    client: &ServerClient,
    d: &Path,
    user: &str,
    req: &Path,
) -> Result<()> {
    // Only real local users, and only their own request.
    let pw = users::get_user_by_name(user).ok_or_else(|| anyhow::anyhow!("no such OS user"))?;
    let meta = std::fs::metadata(req)?;
    if meta.uid() != pw.uid() {
        anyhow::bail!(
            "request file owned by uid {} but {user} is uid {}",
            meta.uid(),
            pw.uid()
        );
    }
    let body = match client.mint_voucher(user).await {
        Ok((voucher, _ttl)) => crate::login::console_url(&cfg.server_url, &voucher),
        Err(e) => {
            // The server's reason is the useful part ("no_account" = nobody
            // linked to this login yet).
            format!("error: could not get a sign-in voucher ({e:#})")
        }
    };
    write_private(d, user, pw.uid(), &body)?;
    tracing::info!("login voucher issued for {user}");
    Ok(())
}

fn write_private(d: &Path, user: &str, uid: u32, body: &str) -> Result<()> {
    use std::io::Write;
    let tmp = d.join(format!("{user}.url.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(body.as_bytes())?;
    drop(f);
    std::os::unix::fs::chown(&tmp, Some(uid), None)?;
    std::fs::rename(&tmp, d.join(format!("{user}.url")))?;
    Ok(())
}
