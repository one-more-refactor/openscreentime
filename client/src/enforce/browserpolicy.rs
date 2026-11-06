//! Browser enterprise policies that turn DNS-over-HTTPS off while
//! `block_doh` is on.
//!
//! A browser with DoH on resolves names itself, over HTTPS, and never asks
//! the local dnsmasq — every DNS block and safe-search rewrite is then gone,
//! with no root needed (Firefox: Settings → "DNS over HTTPS"; Chrome: "Use
//! secure DNS"). The firewall drops the big resolvers' well-known addresses
//! and `dns` sinkholes their names, but any other DoH server still works. The
//! browsers' own managed policies close that: they switch DoH off and lock
//! the setting so it can't be turned back on from the browser's settings.
//!
//! - Firefox: `/etc/firefox/policies/policies.json`, key
//!   `policies.DNSOverHTTPS = {"Enabled": false, "Locked": true}`. The file
//!   can hold other policies (a distro's, an admin's): ours is merged in and,
//!   when `block_doh` goes off, removed again only if it is still exactly
//!   ours. A file that isn't valid JSON is left alone.
//! - Chromium family: a file of our own, `openscreentime-doh.json`, with
//!   `{"DnsOverHttpsMode": "off"}` in each browser's managed-policy dir.
//!
//! Not covered (honest limits): Flatpak browsers (their sandbox doesn't see
//! the host's /etc policies), browsers that read policies from elsewhere,
//! a portable browser in the child's home, and non-browser DoH clients.
//!
//! The agent's sandbox (ProtectSystem=strict) can't write these paths, so the
//! agent only *checks* them and, when they differ from what the policy wants,
//! runs `ost __browser-policy on|off` in a transient unit outside the sandbox
//! (the same pattern as `__refresh-units`). `uninstall` / `__retire` remove
//! them directly.

use crate::config::AgentCtx;
use crate::util::Exec;
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const FIREFOX_POLICIES: &str = "/etc/firefox/policies/policies.json";

/// Managed-policy directories of the Chromium family on Linux. NEEDS REVIEW:
/// Chromium (Debian/Arch/Fedora), older Ubuntu/Debian `chromium-browser`,
/// Google Chrome, Brave, Microsoft Edge. Created if missing, so a browser
/// installed later is covered too.
pub const CHROMIUM_POLICY_DIRS: &[&str] = &[
    "/etc/chromium/policies/managed",
    "/etc/chromium-browser/policies/managed",
    "/etc/opt/chrome/policies/managed",
    "/etc/brave/policies/managed",
    "/etc/opt/edge/policies/managed",
];
const CHROMIUM_FILE: &str = "openscreentime-doh.json";
const CHROMIUM_BODY: &str = "{\n  \"DnsOverHttpsMode\": \"off\"\n}\n";

/// The transient unit the agent starts for [`run_helper`].
const HELPER_UNIT: &str = "openscreentime-browser-policy";
/// Never start the helper more often than this for the same wanted state —
/// if it can't write (a read-only /etc), the agent must not spawn a unit on
/// every policy apply.
const RESPAWN_AFTER: Duration = Duration::from_secs(600);
static LAST_SPAWN: Mutex<Option<(bool, Instant)>> = Mutex::new(None);

fn ours() -> Value {
    json!({ "Enabled": false, "Locked": true })
}

/// What to do with one file.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Keep,
    Write(String),
    Remove,
}

/// Firefox's policies.json, from what is there now (`None`: no file) and
/// whether DoH should be off.
fn firefox_plan(existing: Option<&str>, on: bool) -> Action {
    let Some(text) = existing else {
        return if on {
            Action::Write(render(&json!({ "policies": { "DNSOverHTTPS": ours() } })))
        } else {
            Action::Keep
        };
    };
    let Ok(mut root) = serde_json::from_str::<Value>(text) else {
        tracing::warn!("{FIREFOX_POLICIES} is not valid JSON; leaving it alone");
        return Action::Keep;
    };
    let Some(top) = root.as_object_mut() else {
        return Action::Keep;
    };
    let policies = top
        .entry("policies")
        .or_insert_with(|| Value::Object(Default::default()));
    let Some(policies) = policies.as_object_mut() else {
        return Action::Keep;
    };
    let is_ours = policies.get("DNSOverHTTPS") == Some(&ours());
    if on {
        if is_ours {
            return Action::Keep;
        }
        policies.insert("DNSOverHTTPS".into(), ours());
        return Action::Write(render(&root));
    }
    if !is_ours {
        return Action::Keep;
    }
    policies.remove("DNSOverHTTPS");
    let only_empty_policies = policies.is_empty() && top.len() == 1;
    if only_empty_policies {
        Action::Remove
    } else {
        Action::Write(render(&root))
    }
}

fn render(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_default();
    s.push('\n');
    s
}

fn chromium_plan(existing: Option<&str>, on: bool) -> Action {
    match (existing, on) {
        (Some(body), true) if body == CHROMIUM_BODY => Action::Keep,
        (_, true) => Action::Write(CHROMIUM_BODY.to_string()),
        (Some(_), false) => Action::Remove,
        (None, false) => Action::Keep,
    }
}

/// Every file and what it needs, read from this computer.
fn plan(on: bool) -> Vec<(String, Action)> {
    let read = |p: &str| std::fs::read_to_string(p).ok();
    let mut out = vec![(
        FIREFOX_POLICIES.to_string(),
        firefox_plan(read(FIREFOX_POLICIES).as_deref(), on),
    )];
    for dir in CHROMIUM_POLICY_DIRS {
        let path = format!("{dir}/{CHROMIUM_FILE}");
        let action = chromium_plan(read(&path).as_deref(), on);
        out.push((path, action));
    }
    out
}

/// Do the files on this computer say what `on` wants?
pub fn in_place(on: bool) -> bool {
    plan(on).iter().all(|(_, a)| *a == Action::Keep)
}

/// Write / remove the policies. Needs to run outside the agent's sandbox.
/// Best-effort per file: one that can't be written is logged, not fatal.
pub fn apply(exec: &Exec, on: bool) {
    for (path, action) in plan(on) {
        let res = match action {
            Action::Keep => continue,
            Action::Write(body) => exec.write_file(&path, &body),
            Action::Remove => exec.remove_file(&path).map(|_| ()),
        };
        match res {
            Ok(()) => tracing::info!(
                "browser DoH policy {} at {path}",
                if on { "set" } else { "removed" }
            ),
            Err(e) => tracing::warn!("could not update browser policy {path}: {e:#}"),
        }
    }
}

/// From the agent, on every network apply: if the browser policies don't
/// match `block_doh`, set them through the helper (outside the sandbox).
/// Never touches anything in dry-run.
pub fn sync(exec: &Exec, on: bool) {
    if exec.dry_run() || !crate::config::is_root() || in_place(on) {
        return;
    }
    {
        let mut last = LAST_SPAWN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((want, at)) = *last {
            if want == on && at.elapsed() < RESPAWN_AFTER {
                return;
            }
        }
        *last = Some((on, Instant::now()));
    }
    let arg = if on { "on" } else { "off" };
    let unit = format!("--unit={HELPER_UNIT}");
    match exec.run(
        "systemd-run",
        &[
            "--quiet",
            "--collect",
            &unit,
            crate::service::BIN_TARGET,
            "__browser-policy",
            arg,
        ],
    ) {
        Ok(_) => tracing::info!("updating the browsers' DNS-over-HTTPS policy ({arg})"),
        Err(e) => tracing::warn!("could not start the browser-policy helper: {e}"),
    }
}

/// `ost __browser-policy on|off` (hidden): as root, outside the sandbox.
pub fn run_helper(arg: Option<&str>) -> Result<()> {
    let on = match arg {
        Some("on") => true,
        Some("off") => false,
        other => anyhow::bail!("__browser-policy wants on|off, got {other:?}"),
    };
    apply(&Exec::new(AgentCtx::new(false, false, 1)), on);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firefox_policy_is_created_merged_and_removed_only_if_ours() {
        // Nothing there: created.
        let Action::Write(fresh) = firefox_plan(None, true) else {
            panic!("expected a write")
        };
        let v: Value = serde_json::from_str(&fresh).unwrap();
        assert_eq!(v["policies"]["DNSOverHTTPS"]["Enabled"], json!(false));
        assert_eq!(v["policies"]["DNSOverHTTPS"]["Locked"], json!(true));
        // Already ours: nothing to do; off removes the whole file we made.
        assert_eq!(firefox_plan(Some(fresh.as_str()), true), Action::Keep);
        assert_eq!(firefox_plan(Some(fresh.as_str()), false), Action::Remove);
        assert_eq!(firefox_plan(None, false), Action::Keep);

        // Someone else's policies are kept around ours.
        let theirs = r#"{"policies":{"DisableTelemetry":true}}"#;
        let Action::Write(merged) = firefox_plan(Some(theirs), true) else {
            panic!("expected a write")
        };
        let v: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["policies"]["DisableTelemetry"], json!(true));
        assert_eq!(v["policies"]["DNSOverHTTPS"], ours());
        let Action::Write(back) = firefox_plan(Some(merged.as_str()), false) else {
            panic!("expected a write")
        };
        let v: Value = serde_json::from_str(&back).unwrap();
        assert_eq!(v["policies"]["DisableTelemetry"], json!(true));
        assert!(v["policies"].get("DNSOverHTTPS").is_none());

        // A DoH setting that isn't ours is replaced while on, kept when off.
        let custom = r#"{"policies":{"DNSOverHTTPS":{"Enabled":true}}}"#;
        assert!(matches!(firefox_plan(Some(custom), true), Action::Write(_)));
        assert_eq!(firefox_plan(Some(custom), false), Action::Keep);

        // Not JSON, or not the shape we know: hands off.
        assert_eq!(firefox_plan(Some("{not json"), true), Action::Keep);
        assert_eq!(firefox_plan(Some("[1,2]"), true), Action::Keep);
        assert_eq!(firefox_plan(Some(r#"{"policies":[]}"#), true), Action::Keep);
    }

    #[test]
    fn chromium_policy_is_our_own_file() {
        assert_eq!(
            chromium_plan(None, true),
            Action::Write(CHROMIUM_BODY.to_string())
        );
        assert_eq!(chromium_plan(Some(CHROMIUM_BODY), true), Action::Keep);
        assert!(matches!(chromium_plan(Some("{}"), true), Action::Write(_)));
        assert_eq!(chromium_plan(Some(CHROMIUM_BODY), false), Action::Remove);
        assert_eq!(chromium_plan(None, false), Action::Keep);
        let v: Value = serde_json::from_str(CHROMIUM_BODY).unwrap();
        assert_eq!(v["DnsOverHttpsMode"], json!("off"));
    }
}
