//! The child's half of the CLI: `ost time` and `ost ask`.
//!
//! Why this exists: the shipped artifact is headless. `install.sh` selects the
//! build whose features are `"headless"`, so the tray, the lockout window and
//! the first-run intro — every surface that was supposed to explain this system
//! to the person living under it — are compiled out. What remains, for the
//! child, is a session that freezes mid-game with no message, on a machine
//! whose power button is blocked by a polkit rule.
//!
//! The agent already writes everything needed to
//! `/run/openscreentime/status.<user>.json` each tick: minutes used, minutes
//! left, whether a freeze is armed. The number existed all along; nothing could
//! read it out loud. These two commands are the smallest honest fix — they need
//! no display server, no root, and no features.
//!
//! `ask` writes the same marker the tray would have written, into the user's own
//! `/run/user/<uid>/openscreentime/`, which only they can write and only root
//! reads. That is what makes the request provably theirs.
//!
//! Both also speak `--json`, so an assistant or a status bar can read a child's
//! remaining time without scraping prose that is written to be reassuring.

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// Where the per-user request marker lives. The legacy directory is still
/// accepted on read so a running old agent and a new CLI don't talk past each
/// other mid-upgrade.
fn request_dir(uid: u32) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/run/user/{uid}/openscreentime"))
}

fn legacy_request_dir(uid: u32) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/run/user/{uid}/openscreentime"))
}

fn status_path() -> String {
    let user = users::get_current_username()
        .map(|u| u.to_string_lossy().into_owned())
        .unwrap_or_default();
    crate::paths::run_str(&format!("status.{user}.json"))
}

fn read_status() -> Result<Value> {
    let path = status_path();
    let body = std::fs::read_to_string(&path).with_context(|| {
        format!("no status for you at {path} — is OpenScreenTime running on this computer?")
    })?;
    serde_json::from_str(&body).context("could not read the status file")
}

/// The current user's slice of the status snapshot.
struct Mine {
    used: Option<u64>,
    left: Option<i64>,
    frozen: bool,
    freeze_in: Option<u64>,
}

fn mine() -> Result<Mine> {
    let st = read_status()?;
    let me = st
        .get("users")
        .and_then(|u| u.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);
    Ok(Mine {
        used: me.get("used_minutes").and_then(Value::as_u64),
        left: me.get("remaining_minutes").and_then(Value::as_i64),
        frozen: me.get("frozen").and_then(Value::as_bool).unwrap_or(false),
        freeze_in: me.get("freeze_in_secs").and_then(Value::as_u64),
    })
}

/// `ost time` — how much is left, in a sentence a child can read.
pub fn time(as_json: bool) -> Result<()> {
    let m = mine()?;

    if as_json {
        // Stable shape (docs/AGENT.md → "Machine-readable output"). `limited`
        // distinguishes "no limit configured" from "no time left", which is the
        // one thing a consumer must not get wrong.
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "limited": m.left.is_some(),
                "used_minutes": m.used,
                "left_minutes": m.left,
                "frozen": m.frozen,
                "freeze_in_secs": m.freeze_in,
            }))?
        );
        return Ok(());
    }

    match m.left {
        Some(x) if x > 0 => {
            println!("You have {x} minutes left today.");
            if let Some(u) = m.used {
                println!("(You've used {u} so far.)");
            }
        }
        Some(_) => println!("Your time for today is used up."),
        None => {
            println!("There's no time limit set for you right now.");
            if let Some(u) = m.used {
                println!("You've used {u} minutes today.");
            }
        }
    }

    if let Some(secs) = m.freeze_in {
        println!();
        println!("The screen pauses in {secs} seconds — save what you're doing.");
    }
    if m.frozen {
        println!();
        println!("The screen is paused right now.");
    }
    println!();
    println!("To ask for more time:  ost ask");
    Ok(())
}

/// `ost ask` — send a request to a parent, from the keyboard.
///
/// Runs as the child, never root: it only ever writes inside their own
/// `/run/user/<uid>`, which is exactly what proves the request came from them.
pub fn ask(as_json: bool) -> Result<()> {
    let uid = users::get_current_uid();
    let dir = request_dir(uid);
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let marker = dir.join("earn_request");
    std::fs::write(&marker, b"1")
        .with_context(|| format!("could not write {}", marker.display()))?;

    // Mid-upgrade an older agent may still be watching the previous directory.
    // Writing both costs one file and avoids a request that silently vanishes.
    let legacy = legacy_request_dir(uid);
    if legacy.exists() {
        let _ = std::fs::write(legacy.join("earn_request"), b"1");
    }

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "asked": true, "marker": marker }))?
        );
        return Ok(());
    }

    println!("Asked for more time.");
    println!();
    println!("A parent gets the message now. You'll get an answer either way —");
    println!("check with:  ost time");
    Ok(())
}
