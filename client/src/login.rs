//! `ost login` — open the console already signed in.
//!
//! The server has been able to mint device vouchers since the auth rework, but
//! nothing on the client ever asked for one, so the feature existed only on
//! paper. This is the missing half.
//!
//! How it works: the agent authenticates with its own device token, receives a
//! one-time voucher good for two minutes, and opens the console with the
//! voucher in the URL **fragment**. The fragment matters — it is never sent to
//! a server, so the voucher cannot land in an access log or a proxy trace on
//! the way in. The console redeems it and strips it from the address bar
//! immediately.
//!
//! What this is not: a way to become a parent. The session a voucher buys can
//! read, and it can step up like any other session, but it never *starts*
//! stepped up — possession of the laptop is not possession of the second
//! factor. Changing anything still needs a code.

use crate::client::ServerClient;
use crate::config::AgentConfig;
use anyhow::{Context, Result};
use serde_json::json;

/// Build the console URL that carries a voucher.
///
/// Fragment, not query string: `#v=…` never leaves the browser.
pub fn console_url(server_url: &str, voucher: &str) -> String {
    format!("{}/#v={}", server_url.trim_end_matches('/'), voucher)
}

/// Best-effort browser open. Never fatal — printing the URL is a perfectly
/// good outcome, and on a headless box it is the only possible one.
fn open_browser(url: &str) -> bool {
    for (bin, args) in [
        ("xdg-open", vec![url]),
        ("gio", vec!["open", url]),
        ("wslview", vec![url]),
    ] {
        let ok = std::process::Command::new(bin)
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
        if ok {
            return true;
        }
    }
    false
}

/// The person at the keyboard: the user who ran `ost login` — through sudo,
/// the one who *called* sudo, not root.
pub fn invoking_user() -> String {
    std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root")
        .or_else(|| std::env::var("USER").ok().filter(|u| !u.is_empty()))
        .or_else(|| users::get_current_username().map(|u| u.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// Whether this looks like a session that can actually show a browser.
fn has_display() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub async fn run(print_url: bool, as_json: bool) -> Result<()> {
    let me = invoking_user();
    let (url, expires_in) = if crate::config::is_root() {
        let cfg = AgentConfig::load().map_err(|e| {
            anyhow::anyhow!("this computer isn't set up yet: {e}\nRun `ost enroll` first.")
        })?;
        let client = ServerClient::new(&cfg.server_url, &cfg.device_token)?;
        let (voucher, expires_in) = client
            .mint_voucher(&me)
            .await
            .context("could not get a sign-in voucher from the server")?;
        (console_url(&cfg.server_url, &voucher), expires_in)
    } else {
        // Not root: the running agent mints it for us (loginbroker.rs).
        (crate::loginbroker::request_url(&me).await?, 120)
    };
    if as_json {
        // The URL carries a live credential; a caller asking for JSON is
        // explicitly asking to handle it, so it is theirs to protect.
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "url": url,
                "expires_in_secs": expires_in,
            }))?
        );
        return Ok(());
    }

    if print_url || !has_display() {
        // stdout is the URL and nothing else, so `ost login --print-url` can be
        // piped straight into a browser on another machine.
        println!("{url}");
        eprintln!("Valid for {expires_in} seconds. Opening it signs you in.");
        return Ok(());
    }

    if open_browser(&url) {
        println!("Opening the console — you'll already be signed in.");
        println!("(The link is good for {expires_in} seconds.)");
    } else {
        println!("Couldn't open a browser. Open this yourself, within {expires_in} seconds:");
        println!();
        println!("{url}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The voucher must ride in the fragment. A query string would be written
    /// to the server's access log the moment the console is opened.
    #[test]
    fn the_voucher_rides_in_the_fragment() {
        let url = console_url("https://ost.example.com", "abc123");
        assert_eq!(url, "https://ost.example.com/#v=abc123");
        assert!(
            !url.contains("?"),
            "a query string would be logged server-side"
        );
    }

    #[test]
    fn a_trailing_slash_does_not_double_up() {
        assert_eq!(
            console_url("https://ost.example.com/", "t"),
            "https://ost.example.com/#v=t"
        );
    }
}
