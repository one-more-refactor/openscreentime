//! DNS enforcement: a zero-trust default-deny resolver.
//!
//! Strategy (TAMPER.md): run/configure a local resolver that answers only
//! allowlisted names (wildcards supported) and forwards them to the filtered
//! `upstream`; everything else → NXDOMAIN. `/etc/resolv.conf` is pinned to the
//! local resolver and guarded (immutable bit) so a managed user can't repoint it.
//!
//! The skeleton emits a `dnsmasq` config that realizes this policy and pins
//! resolv.conf. On non-dnsmasq systems the same config is logged (dry-run) and the
//! README documents the `systemd-resolved` drop-in equivalent.
//!
//! Every way this can fail on a real host is reported as a [`DnsGap`] rather than
//! swallowed: a device that cannot enforce DNS must never look like one that can.

use crate::policy::{DnsPolicy, NetworkLockdown};
use crate::util::Exec;
use anyhow::Result;

const DNSMASQ_CONF: &str = "/etc/openscreentime/dnsmasq.d/openscreentime.conf";
const OST_CONF_DIR: &str = "/etc/openscreentime/dnsmasq.d";
const RESOLV_CONF: &str = "/etc/resolv.conf";
const LOCAL_RESOLVER: &str = "127.0.0.1";

/// Distro directories dnsmasq already reads on startup. We drop a one-line
/// `conf-dir=` stub into the first one that exists, because nothing makes
/// dnsmasq read [`OST_CONF_DIR`] on its own — a stock Debian
/// `/etc/dnsmasq.conf` has no active directives at all.
const DISTRO_CONF_DIRS: &[&str] = &["/etc/dnsmasq.d", "/usr/local/etc/dnsmasq.d"];

/// Filename for that stub. `00-` so it is parsed before anything else in the
/// directory, since ordering decides who wins on conflicting options.
const INCLUDE_STUB: &str = "00-openscreentime.conf";

/// A reason DNS enforcement is not actually in force on this host.
///
/// The agent cannot repair any of these by itself, and each one means the
/// allowlist is not doing what the console says it is doing. They are surfaced
/// as `critical` events instead of being logged and forgotten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsGap {
    /// dnsmasq is not installed at all: nothing can serve the allowlist.
    ResolverMissing,
    /// No local resolver is listening, so the rendered ruleset is inert.
    NoLocalResolver,
    /// `/etc/resolv.conf` was a symlink — another service (systemd-resolved's
    /// stub, `resolvconf`) owns the path and will rewrite it.
    ResolvConfNotAFile,
    /// The immutable bit could not be set: a managed user can repoint DNS.
    ResolvConfNotLocked,
    /// The rendered ruleset was written, but no directory dnsmasq actually
    /// reads could be found to include it from — so dnsmasq is running with
    /// its stock config and the policy is a file nobody parses.
    PolicyNotLoaded,
}

impl DnsGap {
    /// Stable machine-readable identifier (event payload `kind`).
    pub fn kind(self) -> &'static str {
        match self {
            DnsGap::ResolverMissing => "dns_resolver_missing",
            DnsGap::NoLocalResolver => "dns_no_local_resolver",
            DnsGap::ResolvConfNotAFile => "dns_resolv_conf_not_a_file",
            DnsGap::ResolvConfNotLocked => "dns_resolv_conf_not_locked",
            DnsGap::PolicyNotLoaded => "dns_policy_not_loaded",
        }
    }

    /// Operator-facing explanation, in the terms the parent/admin needs.
    pub fn explain(self) -> &'static str {
        match self {
            DnsGap::ResolverMissing => {
                "dnsmasq is not installed, so websites are not filtered on this \
                 computer. It stays online with its own DNS, and screen time \
                 still works. Re-run the install command (it installs dnsmasq \
                 and nftables), or install dnsmasq yourself."
            }
            DnsGap::NoLocalResolver => {
                "no local resolver is listening on 127.0.0.1 — dnsmasq failed to \
                 start, so the DNS allowlist is not filtering anything. This \
                 computer keeps its own DNS meanwhile. Check \
                 `systemctl status dnsmasq` on it."
            }
            DnsGap::ResolvConfNotAFile => {
                "/etc/resolv.conf was a symlink owned by another service \
                 (systemd-resolved or resolvconf). It has been replaced with a \
                 real file; disable that service or it will fight the pin on \
                 every network change."
            }
            DnsGap::ResolvConfNotLocked => {
                "the immutable bit could not be set on /etc/resolv.conf — the \
                 filesystem does not support it. A managed user can repoint DNS \
                 and only the 10-second drift check will pull it back."
            }
            DnsGap::PolicyNotLoaded => {
                "the DNS ruleset was written but dnsmasq never reads it: no \
                 dnsmasq config directory was found to include it from, so \
                 dnsmasq is serving its stock config and NOTHING is filtered. \
                 Add `conf-dir=/etc/openscreentime/dnsmasq.d` to this host's \
                 dnsmasq.conf."
            }
        }
    }
}

/// Make dnsmasq actually read [`OST_CONF_DIR`].
///
/// Writing the ruleset is not enough: dnsmasq only parses `/etc/dnsmasq.conf`
/// plus whatever `conf-dir` it was given, and a stock Debian install has no
/// active directives whatsoever. Without this stub the agent writes a policy,
/// restarts dnsmasq successfully, sees it `active`, and reports a fully
/// enforcing device while dnsmasq serves its default forward-everything
/// config — the exact silent-green failure this module exists to prevent.
fn ensure_include(exec: &Exec) -> Option<DnsGap> {
    let dir = DISTRO_CONF_DIRS
        .iter()
        .find(|d| std::path::Path::new(d).is_dir());

    // Under --dry-run nothing exists to probe; log the intent, claim no gap.
    let Some(dir) = dir else {
        if exec.dry_run() {
            return None;
        }
        return Some(DnsGap::PolicyNotLoaded);
    };

    let stub = format!("{dir}/{INCLUDE_STUB}");
    let body = include_stub_body();
    // Already there (the installer seeds it): nothing to write. The agent's
    // sandbox can only write /etc/dnsmasq.d if it existed when the agent
    // started, so a correct stub must not turn into a "could not write" gap.
    if exec.read_file(&stub).as_deref() == Some(body.as_str()) {
        return None;
    }
    if let Err(e) = exec.write_file(&stub, &body) {
        tracing::error!("could not write dnsmasq include stub {stub}: {e}");
        return Some(DnsGap::PolicyNotLoaded);
    }
    None
}

fn include_stub_body() -> String {
    format!("# Managed by openscreentime — do not edit.\nconf-dir={OST_CONF_DIR}\n")
}

/// What dnsmasq serves before the agent's first policy: answer on 127.0.0.1
/// only (never the wildcard, which collides with systemd-resolved's stub on
/// 127.0.0.53 and makes the package's own start fail), forwarding to whatever
/// this computer used before. The first policy apply replaces it.
const BOOTSTRAP_CONF: &str = "# Managed by openscreentime — replaced by the first policy.\n\
listen-address=127.0.0.1\nbind-interfaces\n";

/// Before dnsmasq is installed: make the package's first start one that works
/// and reads our directory. Idempotent; never overwrites a ruleset.
pub fn preseed(exec: &Exec) -> Result<()> {
    let dir = DISTRO_CONF_DIRS[0];
    exec.write_file(&format!("{dir}/{INCLUDE_STUB}"), &include_stub_body())?;
    if exec.read_file(DNSMASQ_CONF).is_none() {
        exec.write_file(DNSMASQ_CONF, BOOTSTRAP_CONF)?;
    }
    Ok(())
}

/// Is a local resolver actually answering? A rendered allowlist that nothing
/// serves is worse than no allowlist, because the console reports it as applied.
fn local_resolver_running(exec: &Exec) -> bool {
    exec.probe("systemctl", &["is-active", "dnsmasq"]).trim() == "active"
}

/// Build the dnsmasq ruleset that realizes the policy. `sinkhole` is the
/// expanded app/category block list (subdomains included by dnsmasq's
/// `address=/d/` semantics): each name answers 0.0.0.0 / :: — an app that
/// cannot resolve its servers is an app that does not work.
pub fn render_dnsmasq(
    dns: &DnsPolicy,
    lockdown: &NetworkLockdown,
    server_host: Option<&str>,
    sinkhole: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("# Managed by openscreentime — do not edit.\n");
    out.push_str("no-resolv\n"); // never inherit host resolv.conf upstreams
    out.push_str("bogus-priv\n");
    out.push_str("domain-needed\n");
    out.push_str("listen-address=127.0.0.1\n");
    out.push_str("bind-interfaces\n");
    // Where-the-time-goes (CONTRACT-0.6): the extra-format query log is the
    // site-level attribution signal. The agent tails and truncates it
    // (attrib.rs); dnsmasq appends, so truncation under it is safe.
    out.push_str("log-queries=extra\n");
    out.push_str(&format!("log-facility={}\n", crate::attrib::DNSQ_LOG));

    // The upstream is interpolated into `server=` lines and must be a bare IP.
    // A policy field with a newline or a directive would otherwise inject
    // arbitrary dnsmasq config into this root-owned, root-reloaded file. A
    // malformed value falls back to the malware+adult family resolver rather
    // than passing through.
    let upstream = if dns.upstream.parse::<std::net::IpAddr>().is_ok() {
        dns.upstream.as_str()
    } else {
        tracing::warn!(
            "ignoring non-IP DNS upstream {:?}; using 1.1.1.3",
            dns.upstream
        );
        "1.1.1.3"
    };
    // A domain safe to interpolate into a resolver directive — same discipline
    // as the catalog sinkhole. Rejects anything with a control char, slash,
    // whitespace or a config-directive shape.
    let clean_domain = |raw: &str| -> Option<String> {
        let d = raw
            .trim_start_matches("*.")
            .trim_start_matches('*')
            .trim_start_matches('.')
            .trim();
        if d.is_empty()
            || !d
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
        {
            return None;
        }
        Some(d.to_string())
    };

    if dns.is_default_deny() && !dns.allows_everything() {
        // Zero-trust: forward ONLY allowlisted domains to the filtered upstream.
        // dnsmasq with no-resolv and no matching server returns NXDOMAIN/REFUSED
        // for everything else, which is the default-deny behavior we want.
        for domain in &dns.allowlist {
            if let Some(d) = clean_domain(domain) {
                out.push_str(&format!("server=/{d}/{upstream}\n"));
            }
        }
        // Explicit extra blocks (redundant under default-deny, honored anyway).
        for b in &dns.blocklist {
            if let Some(b) = clean_domain(b) {
                out.push_str(&format!("address=/{b}/0.0.0.0\n"));
            }
        }
        // The control server must ALWAYS resolve, or a default-deny policy
        // permanently severs the only channel that could relax it: the agent
        // cannot reach the server, so no new policy can ever arrive, and the
        // console has no way to undo what it just did.
        if let Some(host) = server_host {
            if host.parse::<std::net::IpAddr>().is_err() {
                if let Some(h) = clean_domain(host) {
                    out.push_str(&format!("server=/{h}/{upstream}\n"));
                }
            }
        }
        // Catch-all: anything not matched above is NXDOMAIN.
        out.push_str("address=/#/\n");
    } else {
        // allow_all mode, or allowlist == ["*"] (the `default` profile): forward
        // everything to the filtered upstream. Structurally still zero-trust:
        // firewall ports + safe-search stay on.
        out.push_str(&format!("server={upstream}\n"));
        for b in &dns.blocklist {
            if let Some(b) = clean_domain(b) {
                out.push_str(&format!("address=/{b}/0.0.0.0\n"));
            }
        }
    }

    if !sinkhole.is_empty() {
        out.push_str("# app & category blocks (catalog)\n");
        for d in sinkhole {
            // Defensive: the catalog already cleaned these, but this string is
            // interpolated into a resolver config — never let a stray char through.
            if d.is_empty()
                || !d
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
            {
                continue;
            }
            out.push_str(&format!("address=/{d}/0.0.0.0\naddress=/{d}/::\n"));
        }
    }

    if lockdown.block_tor {
        // block_tor: NXDOMAIN .onion and the Tor Project bootstrap domains, so a
        // managed user can't reach hidden services or download a Tor client.
        out.push_str("# block_tor: Tor hidden services + bootstrap domains\n");
        out.push_str("address=/onion/0.0.0.0\n");
        out.push_str("address=/torproject.org/0.0.0.0\n");
    }

    if dns.safe_search {
        // Force safe-search endpoints for the big providers (CNAME rewrites).
        out.push_str("# safe-search enforced\n");
        out.push_str("cname=www.google.com,forcesafesearch.google.com\n");
        out.push_str("cname=www.youtube.com,restrict.youtube.com\n");
        out.push_str("cname=www.bing.com,strict.bing.com\n");
    }
    out
}

pub fn render_resolv_conf() -> String {
    format!(
        "# Managed by openscreentime — pinned. Do not edit.\nnameserver {LOCAL_RESOLVER}\noptions edns0 trust-ad\n"
    )
}

/// Apply DNS policy and pin resolv.conf.
///
/// Returns the [`DnsGap`]s that stop this host from actually enforcing the
/// policy. An empty vec means DNS is genuinely in force.
pub fn apply(
    exec: &Exec,
    dns: &DnsPolicy,
    lockdown: &NetworkLockdown,
    server_host: Option<&str>,
    sinkhole: &[String],
) -> Result<Vec<DnsGap>> {
    let mut gaps = Vec::new();
    let conf = render_dnsmasq(dns, lockdown, server_host, sinkhole);
    exec.write_file(DNSMASQ_CONF, &conf)?;

    // Writing the ruleset does not make dnsmasq read it. Drop the include stub
    // BEFORE the restart so the policy goes live on this cycle, not the next.
    if let Some(gap) = ensure_include(exec) {
        gaps.push(gap);
    }

    // dnsmasq is what actually serves the allowlist. There is no equivalent
    // systemd-resolved path implemented, so a failure here is not something a
    // cache flush papers over — it means nothing is filtering.
    // The query log is every user's browsing on this computer. dnsmasq would
    // create it 0644 — world-readable by every sibling; make it root-only
    // first (append-open, never truncate an existing log).
    if !exec.dry_run() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(crate::attrib::DNSQ_LOG);
        let _ = std::fs::set_permissions(
            crate::attrib::DNSQ_LOG,
            std::fs::Permissions::from_mode(0o600),
        );
    }
    let installed = !exec.observes() || exec.has("dnsmasq");
    if !installed {
        gaps.push(DnsGap::ResolverMissing);
    } else if let Err(e) = exec.run("systemctl", &["restart", "dnsmasq"]) {
        tracing::error!("dnsmasq restart failed: {e}");
    }
    let no_resolver = exec.observes() && !local_resolver_running(exec);
    if no_resolver && installed {
        gaps.push(DnsGap::NoLocalResolver);
    }

    // Pin resolv.conf to the local resolver — but ONLY if there is a resolver
    // to point at.
    //
    // Pinning unconditionally is how a device loses DNS completely and then
    // resists being fixed: /etc/resolv.conf says 127.0.0.1, nothing is
    // listening there, the immutable bit makes `sudo nano /etc/resolv.conf`
    // fail with "Operation not permitted", and the tamper loop re-pins every
    // 10 seconds so even a correct `chattr -i` is reverted before the edit
    // lands. The agent then cannot resolve its own server either, so no policy
    // can undo it remotely. That is a bricked machine, caused by the failure
    // handler rather than the failure.
    //
    // A device that cannot filter must stay usable and loudly degraded — the
    // rule this module opens with.
    if no_resolver {
        tracing::error!(
            "not pinning /etc/resolv.conf: no local resolver is listening, so pinning it \
             would leave this device with no working DNS and no way to edit it back"
        );
        // Undo a pin from a previous run that DID have a resolver, otherwise the
        // device stays broken from then on.
        unpin_resolv_conf(exec);
    } else {
        gaps.extend(pin_resolv_conf(exec)?);
    }

    for gap in &gaps {
        tracing::error!("DNS enforcement gap [{}]: {}", gap.kind(), gap.explain());
    }
    tracing::info!(
        "DNS applied: {} allowlist entries, {} sinkholed app domains, upstream {}, {} gap(s)",
        dns.allowlist.len(),
        sinkhole.len(),
        dns.upstream,
        gaps.len()
    );
    Ok(gaps)
}

/// Where resolv.conf's contents from before the pin are kept, to put back
/// when the pin has to go (no resolver any more, retirement, `ost unlock`).
fn pre_pin_path() -> String {
    crate::paths::state("resolv.conf.pre-pin")
        .to_string_lossy()
        .into_owned()
}

/// What resolv.conf looked like before we pinned it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PrePin {
    /// It was a symlink to this (systemd-resolved's stub, resolvconf's file).
    #[serde(default)]
    symlink: Option<String>,
    /// Its contents (read through the symlink, if it was one).
    content: String,
}

/// Is this resolv.conf our pin?
fn is_our_pin(content: &str) -> bool {
    content.contains("Managed by openscreentime")
        && content.lines().any(|l| {
            let mut words = l.split_whitespace();
            words.next() == Some("nameserver") && words.next() == Some(LOCAL_RESOLVER)
        })
}

/// Pin & guard /etc/resolv.conf (removing any prior immutable bit first).
///
/// Callers pin only while a local resolver is running. What was there before
/// is kept (`resolv.conf.pre-pin`), so taking the pin away gives the computer
/// its own DNS back at once instead of a file pointing at nothing.
pub fn pin_resolv_conf(exec: &Exec) -> Result<Vec<DnsGap>> {
    let mut gaps = Vec::new();

    let current = exec.read_file(RESOLV_CONF).unwrap_or_default();
    if !current.trim().is_empty() && !is_our_pin(&current) {
        let symlink = if exec.dry_run() {
            None
        } else {
            std::fs::read_link(RESOLV_CONF)
                .ok()
                .map(|t| t.to_string_lossy().into_owned())
        };
        let saved = PrePin {
            symlink,
            content: current,
        };
        if let Ok(json) = serde_json::to_string(&saved) {
            if let Err(e) = exec.write_file(&pre_pin_path(), &json) {
                tracing::warn!("could not keep resolv.conf's previous contents: {e}");
            }
        }
    }

    // If the path is a symlink, another service owns it. Writing through the
    // link lands in *that* service's file — typically on tmpfs, where the
    // immutable bit does not exist — so `chattr +i` silently no-ops and the
    // owner rewrites our nameserver on the next network change. Replace the
    // link with a real file we control.
    if !exec.dry_run() {
        if let Ok(md) = std::fs::symlink_metadata(RESOLV_CONF) {
            if md.file_type().is_symlink() {
                gaps.push(DnsGap::ResolvConfNotAFile);
                let _ = std::fs::remove_file(RESOLV_CONF);
            }
        }
    }

    let _ = exec.run("chattr", &["-i", RESOLV_CONF]); // ignore if not set / unsupported fs
    exec.write_file(RESOLV_CONF, &render_resolv_conf())?;
    if let Err(e) = exec.run("chattr", &["+i", RESOLV_CONF]) {
        tracing::error!("could not set immutable bit on resolv.conf: {e}");
        gaps.push(DnsGap::ResolvConfNotLocked);
    }
    Ok(gaps)
}

/// Used when nothing better is left: the family resolvers the policy
/// defaults to (malware + adult filtered), so the computer stays online.
const FALLBACK_RESOLV: &str =
    "# Written by openscreentime: its local resolver is not running, and\n\
# nothing else gave this computer a resolver. Your network manager\n\
# replaces this on the next connection.\n\
nameserver 1.1.1.3\nnameserver 1.0.0.3\n";

/// Take our pin off /etc/resolv.conf and give the computer working DNS back:
/// what was there before the pin, else whatever NetworkManager or
/// systemd-resolved says, else the family resolvers. Never leaves the file
/// pointing at a resolver that isn't running. Does nothing to a resolv.conf
/// that isn't our pin. Returns whether it changed anything.
pub fn unpin_resolv_conf(exec: &Exec) -> bool {
    let current = exec.read_file(RESOLV_CONF).unwrap_or_default();
    if !is_our_pin(&current) {
        return false;
    }
    let _ = exec.run("chattr", &["-i", RESOLV_CONF]);
    let saved = exec
        .read_file(&pre_pin_path())
        .and_then(|s| serde_json::from_str::<PrePin>(&s).ok())
        .filter(|p| !is_our_pin(&p.content));
    if let Some(saved) = saved {
        // A symlink comes back as the symlink where that's possible (outside
        // the agent's sandbox); otherwise its contents do.
        let relinked = saved.symlink.as_deref().is_some_and(|target| {
            !exec.dry_run()
                && std::path::Path::new(target).exists()
                && std::fs::remove_file(RESOLV_CONF).is_ok()
                && std::os::unix::fs::symlink(target, RESOLV_CONF).is_ok()
        });
        if relinked || exec.write_file(RESOLV_CONF, &saved.content).is_ok() {
            let _ = exec.remove_file(&pre_pin_path());
            tracing::warn!("resolv.conf un-pinned: put back what this computer used before");
            return true;
        }
    }
    // Nothing kept (pinned by an older agent): ask NetworkManager to write it.
    if exec.has("nmcli") {
        let _ = exec.run("nmcli", &["general", "reload", "dns-rc"]);
        let now = exec.read_file(RESOLV_CONF).unwrap_or_default();
        if !now.trim().is_empty() && !is_our_pin(&now) {
            tracing::warn!("resolv.conf un-pinned: NetworkManager rewrote it");
            return true;
        }
    }
    // systemd-resolved keeps the real upstreams here.
    let body = exec
        .read_file("/run/systemd/resolve/resolv.conf")
        .filter(|b| b.contains("nameserver"))
        .unwrap_or_else(|| FALLBACK_RESOLV.to_string());
    if let Err(e) = exec.write_file(RESOLV_CONF, &body) {
        tracing::error!("could not un-pin resolv.conf: {e}");
        return false;
    }
    tracing::warn!("resolv.conf un-pinned");
    true
}

/// What the tamper loop's resolv.conf check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reassert {
    /// Pinned to the local resolver, and the resolver is running.
    InForce,
    /// It had drifted and was pinned again; gaps stop the pin from sticking.
    Repinned(Vec<DnsGap>),
    /// No local resolver is running, so nothing was pinned — pinning to a
    /// resolver that isn't there takes the computer offline. `unpinned`: our
    /// earlier pin was taken off just now (the resolver went away).
    NoResolver { unpinned: bool },
}

/// Re-assert resolv.conf if it drifted (called by the tamper loop) — only
/// ever toward a resolver that is actually running.
pub fn reassert(exec: &Exec) -> Result<Reassert> {
    if exec.observes() && !local_resolver_running(exec) {
        return Ok(Reassert::NoResolver {
            unpinned: unpin_resolv_conf(exec),
        });
    }
    let current = exec.read_file(RESOLV_CONF).unwrap_or_default();
    if is_our_pin(&current) {
        return Ok(Reassert::InForce);
    }
    tracing::warn!("resolv.conf drifted off local resolver — re-pinning");
    Ok(Reassert::Repinned(pin_resolv_conf(exec)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injection_in_dns_fields_cannot_add_directives() {
        // A newline-bearing blocklist entry and a non-IP upstream must not
        // inject dnsmasq config; the upstream falls back to the family resolver.
        let dns = DnsPolicy {
            mode: "allow_all".into(),
            allowlist: vec!["*".into()],
            blocklist: vec!["evil.test\nconf-file=/tmp/pwn".into(), "ok.example".into()],
            safe_search: false,
            upstream: "1.1.1.2\nlog-facility=/tmp/x".into(),
        };
        let conf = render_dnsmasq(&dns, &NetworkLockdown::default(), None, &[]);
        assert!(!conf.contains("conf-file"), "no injected directive");
        assert!(
            !conf.contains("log-facility=/tmp/x"),
            "no injected upstream directive"
        );
        assert!(
            conf.contains("server=1.1.1.3"),
            "malformed upstream → safe fallback"
        );
        // The clean sibling entry still applies.
        assert!(conf.contains("address=/ok.example/0.0.0.0"));
    }

    #[test]
    fn default_deny_emits_catch_all_nxdomain() {
        let dns = DnsPolicy {
            mode: "default_deny".into(),
            allowlist: vec!["wikipedia.org".into(), "*.edu".into()],
            blocklist: vec![],
            safe_search: true,
            upstream: "1.1.1.2".into(),
        };
        let conf = render_dnsmasq(&dns, &NetworkLockdown::default(), None, &[]);
        assert!(conf.contains("server=/wikipedia.org/1.1.1.2"));
        assert!(conf.contains("server=/edu/1.1.1.2"));
        assert!(conf.contains("address=/#/"));
        assert!(conf.contains("forcesafesearch.google.com"));
    }

    #[test]
    fn wildcard_star_forwards_everything() {
        let dns = DnsPolicy {
            mode: "default_deny".into(),
            allowlist: vec!["*".into()],
            blocklist: vec![],
            safe_search: false,
            upstream: "1.1.1.2".into(),
        };
        let conf = render_dnsmasq(&dns, &NetworkLockdown::default(), None, &[]);
        assert!(conf.contains("server=1.1.1.2"));
        assert!(!conf.contains("address=/#/"));
    }

    /// App/category blocks become v4 + v6 sinkholes, and nothing that is not a
    /// hostname ever reaches the resolver config.
    #[test]
    fn blocks_are_sinkholed_for_v4_and_v6() {
        let dns = DnsPolicy {
            mode: "allow_all".into(),
            allowlist: vec!["*".into()],
            blocklist: vec![],
            safe_search: false,
            upstream: "1.1.1.3".into(),
        };
        let blocks = crate::policy::AppBlocks {
            apps: vec!["youtube".into()],
            categories: vec!["adult".into()],
            custom_domains: vec!["example.org".into()],
        };
        let sinkhole = openscreentime_policy::catalog::expand(&blocks).domains;
        let conf = render_dnsmasq(&dns, &NetworkLockdown::default(), None, &sinkhole);
        assert!(conf.contains("address=/youtube.com/0.0.0.0\naddress=/youtube.com/::\n"));
        assert!(conf.contains("address=/googlevideo.com/0.0.0.0"));
        assert!(conf.contains("address=/pornhub.com/::"));
        assert!(conf.contains("address=/example.org/0.0.0.0"));
        // a hostile "domain" never lands in the config
        let bad = vec!["evil.com\nserver=9.9.9.9".to_string()];
        let conf = render_dnsmasq(&dns, &NetworkLockdown::default(), None, &bad);
        assert!(!conf.contains("9.9.9.9"));
    }

    #[test]
    fn block_tor_emits_onion_and_torproject_blocks() {
        let dns = DnsPolicy {
            mode: "default_deny".into(),
            allowlist: vec!["*".into()],
            blocklist: vec![],
            safe_search: false,
            upstream: "1.1.1.2".into(),
        };
        let lockdown = NetworkLockdown {
            block_tor: true,
            ..Default::default()
        };
        let conf = render_dnsmasq(&dns, &lockdown, None, &[]);
        assert!(conf.contains("address=/onion/0.0.0.0"));
        assert!(conf.contains("address=/torproject.org/0.0.0.0"));
    }

    /// The `kind` strings land in stored event payloads and in whatever the
    /// console filters on, so they are API. Pin them.
    #[test]
    fn gap_kinds_are_stable_identifiers() {
        assert_eq!(DnsGap::ResolverMissing.kind(), "dns_resolver_missing");
        assert_eq!(DnsGap::NoLocalResolver.kind(), "dns_no_local_resolver");
        assert_eq!(
            DnsGap::ResolvConfNotAFile.kind(),
            "dns_resolv_conf_not_a_file"
        );
        assert_eq!(
            DnsGap::ResolvConfNotLocked.kind(),
            "dns_resolv_conf_not_locked"
        );
        assert_eq!(DnsGap::PolicyNotLoaded.kind(), "dns_policy_not_loaded");
    }

    /// The include stub is what makes dnsmasq read our ruleset at all, so its
    /// contents are load-bearing: a typo here is a silently unfiltered device.
    #[test]
    fn include_stub_points_at_the_openscreentime_conf_dir() {
        let body = format!("conf-dir={OST_CONF_DIR}\n");
        assert!(body.contains("conf-dir=/etc/openscreentime/dnsmasq.d"));
        // The rendered ruleset must live inside the directory we include.
        assert!(DNSMASQ_CONF.starts_with(OST_CONF_DIR));
        // Sorts first, so a conflicting option later in the dir still wins
        // deliberately rather than by filename accident.
        assert!(INCLUDE_STUB.starts_with("00-"));
    }

    fn pos(log: &[String], entry: &str) -> usize {
        log.iter()
            .position(|l| l == entry)
            .unwrap_or_else(|| panic!("{entry:?} not in {log:#?}"))
    }

    const NM_RESOLV: &str = "# Generated by NetworkManager\nnameserver 192.168.1.1\n";

    /// The acceptance bug: no dnsmasq, and the tamper loop pinned
    /// resolv.conf to 127.0.0.1 anyway — a computer with no DNS at all.
    #[test]
    fn reassert_never_pins_to_a_resolver_that_is_not_running() {
        for missing in [&["dnsmasq"][..], &[][..]] {
            let exec = Exec::simulated(
                missing,
                &[
                    ("systemctl is-active dnsmasq", "inactive\n"),
                    ("read /etc/resolv.conf", NM_RESOLV),
                ],
            );
            let r = reassert(&exec).unwrap();
            assert_eq!(r, Reassert::NoResolver { unpinned: false });
            assert!(
                !exec.log().iter().any(|l| l.contains("resolv.conf")),
                "{:?}",
                exec.log()
            );
        }
    }

    /// The resolver went away under a pin: the pin comes off and the
    /// computer's own resolv.conf comes back — not just the immutable bit.
    #[test]
    fn a_pin_without_a_resolver_is_taken_off_and_the_old_dns_restored() {
        let saved = serde_json::to_string(&PrePin {
            symlink: None,
            content: NM_RESOLV.into(),
        })
        .unwrap();
        let exec = Exec::simulated(
            &[],
            &[
                ("systemctl is-active dnsmasq", "failed\n"),
                ("read /etc/resolv.conf", &render_resolv_conf()),
                ("read /var/lib/openscreentime/resolv.conf.pre-pin", &saved),
            ],
        );
        assert_eq!(
            reassert(&exec).unwrap(),
            Reassert::NoResolver { unpinned: true }
        );
        let log = exec.log();
        assert!(pos(&log, "run chattr -i /etc/resolv.conf") < pos(&log, "write /etc/resolv.conf"));
        pos(&log, "remove /var/lib/openscreentime/resolv.conf.pre-pin");
        assert!(!log.iter().any(|l| l.contains("chattr +i")));
    }

    /// Pinned by an older agent that kept nothing: the family resolvers (or
    /// NetworkManager's own file) rather than a pin to nothing.
    #[test]
    fn an_old_pin_with_nothing_kept_still_gets_working_dns() {
        let exec = Exec::simulated(
            &["nmcli"],
            &[
                ("systemctl is-active dnsmasq", "inactive\n"),
                ("read /etc/resolv.conf", &render_resolv_conf()),
            ],
        );
        assert!(unpin_resolv_conf(&exec));
        pos(&exec.log(), "write /etc/resolv.conf");
        assert!(FALLBACK_RESOLV.contains("nameserver 1.1.1.3"));
        assert!(!is_our_pin(FALLBACK_RESOLV));
    }

    /// With the resolver running, drift is re-pinned — and what was there
    /// is kept first, so the pin can be taken off again cleanly.
    #[test]
    fn drift_is_repinned_only_with_a_running_resolver_and_keeps_the_old_file() {
        let exec = Exec::simulated(
            &[],
            &[
                ("systemctl is-active dnsmasq", "active\n"),
                ("read /etc/resolv.conf", NM_RESOLV),
            ],
        );
        assert_eq!(reassert(&exec).unwrap(), Reassert::Repinned(vec![]));
        let log = exec.log();
        assert!(
            pos(&log, "write /var/lib/openscreentime/resolv.conf.pre-pin")
                < pos(&log, "write /etc/resolv.conf")
        );
        pos(&log, "run chattr +i /etc/resolv.conf");
        // Already pinned and served: nothing to do.
        let exec = Exec::simulated(
            &[],
            &[
                ("systemctl is-active dnsmasq", "active\n"),
                ("read /etc/resolv.conf", &render_resolv_conf()),
            ],
        );
        assert_eq!(reassert(&exec).unwrap(), Reassert::InForce);
        assert!(exec.log().is_empty());
    }

    /// No dnsmasq at all: a gap of its own, no restart attempted, no pin.
    #[test]
    fn apply_without_dnsmasq_reports_it_and_leaves_dns_alone() {
        let exec = Exec::simulated(&["dnsmasq"], &[("read /etc/resolv.conf", NM_RESOLV)]);
        let dns = DnsPolicy {
            mode: "allow_all".into(),
            allowlist: vec!["*".into()],
            blocklist: vec![],
            safe_search: true,
            upstream: "1.1.1.3".into(),
        };
        let gaps = apply(&exec, &dns, &NetworkLockdown::default(), None, &[]).unwrap();
        assert_eq!(gaps, vec![DnsGap::ResolverMissing]);
        let log = exec.log();
        assert!(!log.iter().any(|l| l.contains("restart dnsmasq")));
        assert!(!log
            .iter()
            .any(|l| l == "write /etc/resolv.conf" || l.contains("chattr +i")));
    }

    /// Every gap has to tell an operator what to actually do about it — an
    /// alert nobody can action is the failure mode this whole change exists
    /// to remove.
    #[test]
    fn every_gap_explains_itself() {
        for gap in [
            DnsGap::ResolverMissing,
            DnsGap::NoLocalResolver,
            DnsGap::ResolvConfNotAFile,
            DnsGap::ResolvConfNotLocked,
            DnsGap::PolicyNotLoaded,
        ] {
            assert!(gap.explain().len() > 40, "{} has no guidance", gap.kind());
        }
    }
}
