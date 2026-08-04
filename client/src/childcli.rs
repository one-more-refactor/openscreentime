//! The child's half of the CLI: `sentinel-agent time` and `sentinel-agent ask`.
//!
//! Why this exists: the shipped artifact is headless. `install.sh` selects the
//! build whose features are `"headless"`, so the tray, the lockout window and
//! the first-run intro — every surface that was supposed to explain this system
//! to the person living under it — are compiled out. What remains, for the
//! child, is a session that freezes mid-game with no message, on a machine
//! whose power button is blocked by a polkit rule.
//!
//! The agent already writes everything needed to `/run/sentinel/status.<user>.json`
//! each tick: minutes used, minutes left, whether a freeze is armed. The number
//! existed all along; nothing could read it out loud. These two commands are the
//! smallest honest fix — they need no display server, no root, and no features.
//!
//! `ask` writes the same marker the tray would have written, into the user's own
//! `/run/user/<uid>/sentinel/`, which only they can write and only root reads.
//! That is what makes the request provably theirs.

use anyhow::{Context, Result};
use serde_json::Value;

fn status_path() -> String {
    let user = users::get_current_username()
        .map(|u| u.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("/run/sentinel/status.{user}.json")
}

fn read_status() -> Result<Value> {
    let path = status_path();
    let body = std::fs::read_to_string(&path).with_context(|| {
        format!("no status for you at {path} — is the Sentinel agent running on this computer?")
    })?;
    serde_json::from_str(&body).context("could not read the status file")
}

/// `sentinel-agent time` — how much is left, in a sentence a child can read.
pub fn time() -> Result<()> {
    let st = read_status()?;
    let me = st
        .get("users")
        .and_then(|u| u.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);

    let used = me.get("used_minutes").and_then(Value::as_u64);
    let left = me.get("remaining_minutes").and_then(Value::as_i64);
    let frozen = me.get("frozen").and_then(Value::as_bool).unwrap_or(false);
    let freeze_in = me.get("freeze_in_secs").and_then(Value::as_u64);

    match left {
        Some(m) if m > 0 => {
            println!("You have {m} minutes left today.");
            if let Some(u) = used {
                println!("(You've used {u} so far.)");
            }
        }
        Some(_) => println!("Your time for today is used up."),
        None => {
            println!("There's no time limit set for you right now.");
            if let Some(u) = used {
                println!("You've used {u} minutes today.");
            }
        }
    }

    if let Some(secs) = freeze_in {
        println!();
        println!("The screen pauses in {secs} seconds — save what you're doing.");
    }
    if frozen {
        println!();
        println!("The screen is paused right now.");
    }
    println!();
    println!("To ask for more time:  sentinel-agent ask");
    Ok(())
}

/// `sentinel-agent ask` — send a request to a parent, from the keyboard.
///
/// Runs as the child, never root: it only ever writes inside their own
/// `/run/user/<uid>`, which is exactly what proves the request came from them.
pub fn ask() -> Result<()> {
    let uid = users::get_current_uid();
    let dir = std::path::PathBuf::from(format!("/run/user/{uid}/sentinel"));
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let marker = dir.join("earn_request");
    std::fs::write(&marker, b"1")
        .with_context(|| format!("could not write {}", marker.display()))?;

    println!("Asked for more time.");
    println!();
    println!("A parent gets the message now. You'll get an answer either way —");
    println!("check with:  sentinel-agent time");
    Ok(())
}
