//! Sign-in codes on this computer — door one of docs/AUTH.md.
//!
//! Someone types their name on the sign-in page; the server sends a
//! `login_code` command to the computers that person uses:
//! `{request_id, name, os_users, code, purpose, site, expires_in_secs}`.
//! The person reads the code here and types it into the browser.
//!
//! Where the code goes — and where it must never go:
//!
//! * The root agent keeps it in memory and publishes it only in the private
//!   status file of each OS login it's for (`status.<user>.json`, 0600, owned
//!   by that user). Never the shared `status.json`, never argv, never `wall`.
//! * Readers, all running as that user: the **app window** (a card at the top;
//!   the agent opens the window if it isn't open), **one desktop
//!   notification** (from whichever of the app or the tray sees it first), and
//!   `ost code` in a terminal — the only surface a headless build has.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One code, as published to its person's status file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginCode {
    pub id: String,
    /// Whose sign-in it is (their name on the household).
    #[serde(default)]
    pub name: String,
    pub code: String,
    /// "login" (a browser signing in) or "confirm" (a signed-in console
    /// confirming it's you).
    #[serde(default)]
    pub purpose: String,
    /// The console's address — the only place this code should be typed.
    #[serde(default)]
    pub site: String,
    /// RFC 3339.
    pub expires_at: String,
}

impl LoginCode {
    /// Parse the server's `login_code` command payload into the code and the
    /// OS logins it is for. `None` if anything is missing or malformed.
    pub fn from_command(
        payload: &Value,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<(LoginCode, Vec<String>)> {
        let s = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or("").trim();
        let id = s("request_id").to_string();
        let code = s("code").to_string();
        // The id names a marker file, so it must be an opaque token.
        let id_ok = !id.is_empty()
            && id.len() <= 64
            && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !id_ok || code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let users: Vec<String> = payload
            .get("os_users")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if users.is_empty() {
            return None;
        }
        let secs = payload
            .get("expires_in_secs")
            .and_then(Value::as_i64)
            .unwrap_or(300)
            .clamp(1, 900);
        Some((
            LoginCode {
                id,
                name: s("name").to_string(),
                code,
                purpose: s("purpose").to_string(),
                site: s("site").to_string(),
                expires_at: (now + chrono::Duration::seconds(secs)).to_rfc3339(),
            },
            users,
        ))
    }

    pub fn expires(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::parse_from_rfc3339(&self.expires_at)
            .ok()
            .map(|t| t.with_timezone(&chrono::Utc))
    }

    pub fn is_live(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.expires().is_some_and(|t| t > now)
    }

    /// "123 456" — easier to read off a screen and type.
    pub fn spaced(&self) -> String {
        format!(
            "{} {}",
            &self.code[..3.min(self.code.len())],
            &self.code[3.min(self.code.len())..]
        )
    }

    /// Whole minutes left, rounded up (never shows "0 minutes").
    pub fn minutes_left(&self, now: chrono::DateTime<chrono::Utc>) -> i64 {
        self.expires()
            .map(|t| ((t - now).num_seconds().max(0) + 59) / 60)
            .unwrap_or(0)
            .max(1)
    }

    pub fn headline(&self) -> &'static str {
        if self.purpose == "confirm" {
            "Your confirm code"
        } else {
            "Your sign-in code"
        }
    }

    /// One plain sentence: where to type it, and what to do if you didn't ask.
    pub fn instructions(&self) -> String {
        let site = host_of(&self.site);
        let what = if self.purpose == "confirm" {
            "to confirm it's you".to_string()
        } else if self.name.is_empty() {
            "to sign in".to_string()
        } else {
            format!("to sign in as {}", self.name)
        };
        if site.is_empty() {
            format!("Type it into the OpenScreenTime page {what}. Didn't ask? Ignore it.")
        } else {
            format!("Type it into {site} {what}. Didn't ask? Ignore it.")
        }
    }
}

/// `https://ost.example.com/` → `ost.example.com` (what a person recognises).
fn host_of(site: &str) -> String {
    let rest = site
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    rest.split('/').next().unwrap_or("").to_string()
}

/// The live codes in a user's status snapshot (`login_codes`).
pub fn live_codes(status: &Value, now: chrono::DateTime<chrono::Utc>) -> Vec<LoginCode> {
    status
        .get("login_codes")
        .and_then(|v| serde_json::from_value::<Vec<LoginCode>>(v.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.is_live(now))
        .collect()
}

/// This user's private runtime dir (`/run/user/<uid>/openscreentime`).
#[cfg(feature = "tray")]
fn runtime_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(format!(
        "/run/user/{}/openscreentime",
        users::get_current_uid()
    ))
}

/// True exactly once per code per user, across the app window and the tray,
/// so a code rings once however many surfaces are running.
#[cfg(feature = "tray")]
pub fn first_sighting(c: &LoginCode) -> bool {
    let dir = runtime_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return true;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(format!("code-{}.seen", c.id)))
        .is_ok()
}

/// One desktop notification carrying the code, sent from the user's own
/// session (in-process, so the code never touches a command line).
#[cfg(feature = "tray")]
pub fn notify(c: &LoginCode) {
    let mut n = notify_rust::Notification::new();
    n.appname("OpenScreenTime")
        .summary(&format!("{}: {}", c.headline(), c.spaced()))
        .body(&c.instructions())
        .icon("openscreentime")
        .hint(notify_rust::Hint::DesktopEntry("openscreentime".into()))
        .urgency(notify_rust::Urgency::Critical)
        .timeout(notify_rust::Timeout::Milliseconds(
            (c.minutes_left(chrono::Utc::now()) * 60_000).clamp(10_000, 300_000) as u32,
        ));
    if let Err(e) = n.show() {
        tracing::debug!("could not show the sign-in code notification: {e}");
    }
}

/// Open the app window in `user`'s graphical session, as that user, so the
/// code shows up even where there is no tray (GNOME) and nobody has the
/// window open. A second copy exits at once if one is already running
/// (`app::run` holds a lock); the running one sees the code and comes forward.
/// No graphical session → nothing to do (`ost code` still works).
#[cfg(feature = "gui")]
pub fn open_app_for(user: &str) {
    if let Err(e) = spawn_in_session(user, &["app"]) {
        tracing::warn!("could not open the app window for {user}: {e}");
    }
}

/// `runuser` as an absolute path. It lives in /usr/sbin on Debian, and the
/// environment handed to the child below has a user's PATH, which has no
/// sbin — so a bare "runuser" was not found, and the window never opened.
fn runuser() -> std::io::Result<std::path::PathBuf> {
    crate::util::find_program("runuser").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "runuser is not installed")
    })
}

/// Run this binary with `args` inside `user`'s graphical session, as that
/// user: their uid, groups and a clean environment pointing at their
/// session (Wayland socket or X display, session bus). `Ok` with nothing
/// started when they have no graphical session.
pub fn spawn_in_session(user: &str, args: &[&str]) -> std::io::Result<()> {
    use std::path::Path;
    let Some(pw) = users::get_user_by_name(user) else {
        return Ok(());
    };
    let uid = pw.uid();
    let runtime = format!("/run/user/{uid}");
    if !Path::new(&runtime).is_dir() {
        return Ok(());
    }
    let home = users::os::unix::UserExt::home_dir(&pw)
        .to_string_lossy()
        .into_owned();
    let mut env: Vec<(String, String)> = vec![
        ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
        ("HOME".into(), home.clone()),
        ("USER".into(), user.into()),
        ("LOGNAME".into(), user.into()),
        ("XDG_RUNTIME_DIR".into(), runtime.clone()),
        (
            "DBUS_SESSION_BUS_ADDRESS".into(),
            format!("unix:path={runtime}/bus"),
        ),
    ];
    let wayland = std::fs::read_dir(&runtime).ok().and_then(|entries| {
        entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("wayland-") && !n.ends_with(".lock"))
            .min()
    });
    if let Some(w) = wayland {
        env.push(("WAYLAND_DISPLAY".into(), w));
    } else if Path::new("/tmp/.X11-unix/X0").exists() {
        env.push(("DISPLAY".into(), ":0".into()));
        let gdm = format!("{runtime}/gdm/Xauthority");
        let xauth = if Path::new(&gdm).exists() {
            gdm
        } else {
            format!("{home}/.Xauthority")
        };
        env.push(("XAUTHORITY".into(), xauth));
    } else {
        return Ok(());
    }
    // The installed binary where there is one: a self-update replaces it
    // under a running agent, whose own path then reads "(deleted)".
    let exe = if Path::new(crate::service::BIN_TARGET).exists() {
        std::path::PathBuf::from(crate::service::BIN_TARGET)
    } else {
        std::env::current_exe()?
    };
    // runuser drops to the user (uid, gid and their groups); the environment
    // is ours, set explicitly, nothing inherited from the root agent.
    let mut cmd = std::process::Command::new(runuser()?);
    cmd.arg("-u")
        .arg(user)
        .arg("--")
        .arg(exe)
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Reap it in the background so it never lingers as a zombie.
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// `ost code` — the current sign-in code, for a terminal (and the only way to
/// see one on a headless build).
pub fn cli(as_json: bool) -> anyhow::Result<()> {
    let user = crate::login::invoking_user();
    let path = crate::paths::run_str(&format!("status.{user}.json"));
    let status: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let now = chrono::Utc::now();
    let codes = live_codes(&status, now);
    if as_json {
        println!("{}", serde_json::to_string_pretty(&codes)?);
        return Ok(());
    }
    if codes.is_empty() {
        println!("No code right now.");
        println!("Type your name on the OpenScreenTime sign-in page; the code shows up here.");
        return Ok(());
    }
    for c in &codes {
        println!("{}: {}", c.headline(), c.spaced());
        println!("{}", c.instructions());
        println!(
            "It works for {} more minute{}.",
            c.minutes_left(now),
            if c.minutes_left(now) == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Debian keeps runuser in /usr/sbin, which a user's PATH (the one the
    /// session child gets) doesn't reach: it is found by absolute path.
    #[test]
    fn runuser_is_found_outside_a_users_path() {
        let p = runuser().expect("util-linux's runuser");
        assert!(p.is_absolute(), "{p:?}");
    }

    fn payload() -> Value {
        json!({
            "request_id": "6f1c2a3b-0000-4000-8000-000000000001",
            "name": "Philip",
            "os_users": ["philip"],
            "code": "042917",
            "purpose": "login",
            "site": "https://ost.example.com",
            "expires_in_secs": 300,
        })
    }

    #[test]
    fn a_command_becomes_a_code_for_exactly_those_logins() {
        let now = chrono::Utc::now();
        let (c, users) = LoginCode::from_command(&payload(), now).unwrap();
        assert_eq!(users, vec!["philip".to_string()]);
        assert_eq!(c.spaced(), "042 917");
        assert!(c.is_live(now));
        assert!(!c.is_live(now + chrono::Duration::seconds(301)));
        assert_eq!(c.minutes_left(now), 5);
        assert_eq!(
            c.instructions(),
            "Type it into ost.example.com to sign in as Philip. Didn't ask? Ignore it."
        );
    }

    #[test]
    fn malformed_commands_are_refused() {
        let now = chrono::Utc::now();
        for (k, v) in [
            ("code", json!("12345")),
            ("code", json!("12a456")),
            ("request_id", json!("../../etc/passwd")),
            ("request_id", json!("")),
            ("os_users", json!([])),
        ] {
            let mut p = payload();
            p[k] = v;
            assert!(LoginCode::from_command(&p, now).is_none(), "{k}");
        }
    }

    #[test]
    fn only_live_codes_are_read_back() {
        let now = chrono::Utc::now();
        let (live, _) = LoginCode::from_command(&payload(), now).unwrap();
        let mut dead = live.clone();
        dead.id = "old".into();
        dead.expires_at = (now - chrono::Duration::seconds(1)).to_rfc3339();
        let status = json!({ "login_codes": [live.clone(), dead] });
        assert_eq!(live_codes(&status, now), vec![live]);
        assert!(live_codes(&json!({}), now).is_empty());
    }

    #[test]
    fn a_confirm_code_says_so() {
        let now = chrono::Utc::now();
        let mut p = payload();
        p["purpose"] = json!("confirm");
        let (c, _) = LoginCode::from_command(&p, now).unwrap();
        assert_eq!(c.headline(), "Your confirm code");
        assert!(c.instructions().contains("to confirm it's you"));
    }
}
