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
        if let Some(url) = read_own_answer(&ans) {
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

/// Read the agent's answer only if it is really the agent's answer to US.
///
/// The drop-box is world-writable (sticky), so another local user can plant
/// `<us>.url` first — with a phishing link — and we can't even delete it. The
/// agent's real answer is a regular file chowned to our uid with mode 0600;
/// anything else (someone else's file, a group/world-readable one, a symlink:
/// `O_NOFOLLOW`) is ignored, and the agent's rename replaces it.
fn read_own_answer(path: &Path) -> Option<String> {
    use std::io::Read;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = f.metadata().ok()?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let me = unsafe { libc::getuid() };
    if !meta.is_file() || meta.uid() != me || meta.mode() & 0o077 != 0 {
        tracing::warn!(
            "ignoring {}: not a private file of ours (uid {}, mode {:o})",
            path.display(),
            meta.uid(),
            meta.mode() & 0o7777
        );
        return None;
    }
    let mut body = String::new();
    f.take(64 * 1024).read_to_string(&mut body).ok()?;
    Some(body)
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
    // Only real local users, and only their own request. `symlink_metadata`
    // (lstat), NOT `metadata` (stat): the request lives in a world-writable
    // sticky dir, so a child could point `alice.req` at a file alice owns and
    // pass a follow-based uid check to mint a voucher "as alice". lstat checks
    // the link itself, and we reject any symlink outright.
    let pw = users::get_user_by_name(user).ok_or_else(|| anyhow::anyhow!("no such OS user"))?;
    let meta = std::fs::symlink_metadata(req)?;
    if meta.file_type().is_symlink() {
        anyhow::bail!("request file is a symlink — refusing");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_private_answer_of_ours_is_trusted() {
        let d = std::env::temp_dir().join(format!("ost-loginbroker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();

        let ours = d.join("me.url");
        std::fs::write(&ours, "https://ost.example/#v=1").unwrap();
        std::fs::set_permissions(&ours, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            read_own_answer(&ours).as_deref(),
            Some("https://ost.example/#v=1")
        );

        // Planted the way another user would: readable by others.
        std::fs::set_permissions(&ours, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read_own_answer(&ours), None);

        // A symlink to a private file of ours is not followed.
        std::fs::set_permissions(&ours, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = d.join("link.url");
        std::os::unix::fs::symlink(&ours, &link).unwrap();
        assert_eq!(read_own_answer(&link), None);

        assert_eq!(read_own_answer(&d.join("missing.url")), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
