//! `enroll` subcommand: report identity + OS users to the server, receive
//! `device_id` + `device_token`, and persist them to the root-owned config.

use crate::client::{self, EnrollRequest};
use crate::config::AgentConfig;
use crate::sysusers;
use anyhow::Result;

/// A plaintext `server_url` makes the self-updater's sha256 check decorative:
/// an on-path attacker (open Wi-Fi, LAN ARP-spoof) controls both the manifest
/// and the bytes it hashes, so the check always "passes". Refuse `http://`
/// except to loopback/`.local`, where it's a legitimate dev/LAN setup.
fn ensure_secure_server(server: &str) -> Result<()> {
    let lower = server.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = lower.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        let local = host == "localhost"
            || host == "127.0.0.1"
            || host == "::1"
            || host == "[::1]"
            || host.ends_with(".local");
        if local {
            tracing::warn!("enrolling over plaintext http:// to {host} (local only)");
            return Ok(());
        }
    }
    anyhow::bail!(
        "refusing to enroll against a non-https server URL ({server}): plaintext \
         transport defeats update verification. Use https://, or a loopback/.local \
         host for local testing."
    )
}

/// Where `enroll` reads the one-time token from, most private first:
/// `OST_TOKEN` in the environment (what `install.sh` uses — an environment
/// is readable only by its owner and root, where argv is in everyone's `ps`
/// and in shell history), `--token -` for one line on stdin, and `--token
/// <TOKEN>` as a last resort.
pub fn resolve_token(
    arg: Option<&str>,
    env: Option<String>,
    stdin: &mut dyn std::io::BufRead,
) -> Result<String> {
    let token = match arg {
        Some("-") => {
            let mut line = String::new();
            stdin.read_line(&mut line)?;
            line
        }
        Some(t) => t.to_string(),
        None => env.unwrap_or_default(),
    };
    let token = token.trim().to_string();
    if token.is_empty() {
        anyhow::bail!(
            "an enroll token is required: OST_TOKEN=<token> in the environment, or \
             --token - to read it from stdin"
        );
    }
    Ok(token)
}

/// The login the install ran from: `sudo` records it in `SUDO_USER`; a root
/// shell reached through `su` still carries the login uid in
/// `/proc/self/loginuid`. On "my computer" that login is the parent's own.
fn installer() -> Option<String> {
    if let Ok(u) = std::env::var("SUDO_USER") {
        if !u.is_empty() && u != "root" {
            return Some(u);
        }
    }
    let uid: u32 = std::fs::read_to_string("/proc/self/loginuid")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if uid == 0 || uid == u32::MAX {
        return None;
    }
    users::get_user_by_uid(uid).map(|u| u.name().to_string_lossy().into_owned())
}

/// Parse the answer to "which login is Mia's?": a number from the list, or
/// nothing (0, blank, nonsense) = "none of these".
fn pick(answer: &str, logins: &[String]) -> Option<String> {
    let n: usize = answer.trim().parse().ok()?;
    logins.get(n.checked_sub(1)?).cloned()
}

/// Ask the person at the keyboard which login belongs to whoever this
/// computer is for — only when it matters (more than one login) and when
/// there is a terminal to ask on. Reads `/dev/tty`, never stdin: `install.sh`
/// is piped into `sh`, so stdin is the script.
fn ask_owner_login(
    preview: &client::EnrollPreview,
    logins: &[String],
    installer: Option<&str>,
) -> Option<String> {
    use std::io::{BufRead, Write};
    if logins.len() < 2 {
        return None;
    }
    let owner = preview.owner.as_deref()?;
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let mut out = &tty;
    let whose = if preview.owner_is_parent {
        "yours".to_string()
    } else {
        format!("{owner}'s")
    };
    let _ = writeln!(
        out,
        "\nThis computer is being set up for {owner}. Which login on it is {whose}?"
    );
    for (i, l) in logins.iter().enumerate() {
        let now = if Some(l.as_str()) == installer {
            "   (the one you're using now)"
        } else {
            ""
        };
        let _ = writeln!(out, "  {}) {l}{now}", i + 1);
    }
    let _ = writeln!(
        out,
        "  0) none of these — every login stays a person of its own"
    );
    let _ = write!(out, "Number: ");
    let _ = out.flush();
    let mut line = String::new();
    std::io::BufReader::new(&tty).read_line(&mut line).ok()?;
    pick(&line, logins)
}

pub async fn run(server: &str, token: &str) -> Result<()> {
    ensure_secure_server(server)?;
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let os_users = sysusers::login_users();
    tracing::info!(
        "enrolling {} against {} with {} OS user(s)",
        hostname,
        server,
        os_users.len()
    );

    // Whose computer is this, and which login is theirs? Only that login is
    // linked to them; every other login is its own person (docs/AUTH.md).
    let installer = installer();
    let logins: Vec<String> = os_users.iter().map(|u| u.username.clone()).collect();
    let owner_login = match client::enroll_preview(server, token).await {
        Ok(preview) => ask_owner_login(&preview, &logins, installer.as_deref()),
        Err(e) => {
            tracing::debug!("no enroll preview ({e}); not asking whose login is whose");
            None
        }
    };

    let req = EnrollRequest {
        enroll_token: token.to_string(),
        hostname,
        os: "linux".to_string(),
        agent_version: client::AGENT_VERSION.to_string(),
        os_users,
        installer,
        owner_login,
    };

    let resp = client::enroll(server, &req).await?;
    tracing::info!("enrolled: device_id={}", resp.device_id);
    if !resp.users.is_empty() {
        println!("Who's who on this computer:");
        for u in &resp.users {
            let role = if u.parent { " (parent)" } else { "" };
            println!("  {} → {}{role}", u.os_username, u.person);
        }
        println!("  Wrong? Change it in the console, under Devices.");
        println!();
    }

    let cfg = AgentConfig {
        server_url: server.trim_end_matches('/').to_string(),
        device_id: resp.device_id,
        device_token: resp.device_token,
        poll_interval_secs: resp.poll_interval_secs,
        tamper_level: 1,
        auto_update: true,
    };
    cfg.save()?;
    tracing::info!("wrote {} (0600)", crate::config::CONFIG_PATH);
    println!("Enrolled. Config written to {}", crate::config::CONFIG_PATH);
    // Nothing to write down: the keys to this computer live in the console.
    // The unlock code (6 digits, changes every 30 s) and the one-time recovery
    // codes are read there after a step-up, and verified here offline.
    println!();
    println!("  Unlock code: open the OpenScreenTime console → this computer → Unlock code.");
    println!("  It opens the lock screen, `sudo`, and `sudo ost unlock` — no internet needed.");
    println!("  Recovery codes (for when your phone is not around) are generated in the");
    println!("  same place; generate a set now and keep it somewhere safe.");
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ensure_secure_server, pick, resolve_token};

    #[test]
    fn the_token_comes_from_the_environment_or_stdin() {
        let mut none: &[u8] = b"";
        // install.sh: OST_TOKEN only, nothing in argv.
        let t = resolve_token(None, Some("env-tok\n".into()), &mut none).unwrap();
        assert_eq!(t, "env-tok");
        // `--token -`: one line on stdin, even with OST_TOKEN set.
        let mut stdin: &[u8] = b"  stdin-tok \nrest\n";
        let t = resolve_token(Some("-"), Some("env-tok".into()), &mut stdin).unwrap();
        assert_eq!(t, "stdin-tok");
        // `--token x` still works.
        assert_eq!(
            resolve_token(Some("argv-tok"), None, &mut none).unwrap(),
            "argv-tok"
        );
        // Nothing anywhere, or only blanks: a plain error, never an empty token.
        assert!(resolve_token(None, None, &mut none).is_err());
        assert!(resolve_token(None, Some("  ".into()), &mut none).is_err());
        let mut blank: &[u8] = b"\n";
        assert!(resolve_token(Some("-"), None, &mut blank).is_err());
    }

    #[test]
    fn the_answer_picks_a_login_or_none() {
        let l = vec!["dad".to_string(), "mia".to_string()];
        assert_eq!(pick("2\n", &l).as_deref(), Some("mia"));
        assert_eq!(pick(" 1 ", &l).as_deref(), Some("dad"));
        assert_eq!(pick("0", &l), None);
        assert_eq!(pick("", &l), None);
        assert_eq!(pick("3", &l), None);
        assert_eq!(pick("mia", &l), None);
    }

    #[test]
    fn https_is_accepted() {
        assert!(ensure_secure_server("https://ost.example.com").is_ok());
    }

    #[test]
    fn loopback_http_is_allowed_for_dev() {
        assert!(ensure_secure_server("http://localhost:8080").is_ok());
        assert!(ensure_secure_server("http://127.0.0.1:8080").is_ok());
        assert!(ensure_secure_server("http://box.local").is_ok());
    }

    #[test]
    fn public_http_is_rejected() {
        assert!(ensure_secure_server("http://ost.example.com").is_err());
        assert!(ensure_secure_server("http://203.0.113.7:8080").is_err());
    }
}
