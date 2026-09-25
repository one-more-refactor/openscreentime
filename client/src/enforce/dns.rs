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

use super::safesearch::{self, SafeSearch};
use crate::policy::{DnsPolicy, NetworkLockdown};
use crate::util::Exec;
use anyhow::Result;

const DNSMASQ_CONF: &str = "/etc/openscreentime/dnsmasq.d/openscreentime.conf";
pub const OST_CONF_DIR: &str = "/etc/openscreentime/dnsmasq.d";
const RESOLV_CONF: &str = "/etc/resolv.conf";
const LOCAL_RESOLVER: &str = "127.0.0.1";

/// The service that serves the website rules. The `dnsmasq` program alone
/// proves nothing: a Debian or Ubuntu desktop carries it in `dnsmasq-base`,
/// a NetworkManager dependency with no service, no config and no
/// `/etc/dnsmasq.d` — so "dnsmasq is installed" there, and nothing filters.
pub const DNSMASQ_UNIT: &str = "dnsmasq.service";

/// Distro directories dnsmasq already reads on startup. We drop a one-line
/// `conf-dir=` stub into the one this dnsmasq reads (see [`include_route`]),
/// because nothing makes dnsmasq read [`OST_CONF_DIR`] on its own — a stock
/// Debian `/etc/dnsmasq.conf` has no active directives at all.
const DISTRO_CONF_DIRS: &[&str] = &["/etc/dnsmasq.d", "/usr/local/etc/dnsmasq.d"];

/// Filename for that stub. `00-` so it is parsed before anything else in the
/// directory, since ordering decides who wins on conflicting options.
const INCLUDE_STUB: &str = "00-openscreentime.conf";

/// dnsmasq's own config file, and Debian's defaults file for it (the Debian
/// and Ubuntu unit runs an init script that adds `-7 $CONFIG_DIR` from there,
/// `/etc/dnsmasq.d` as shipped).
const DNSMASQ_MAIN_CONF: &str = "/etc/dnsmasq.conf";
const DEBIAN_DEFAULTS: &str = "/etc/default/dnsmasq";

/// The two lines [`wire_main_conf`] adds to dnsmasq.conf on a distro whose
/// dnsmasq reads no directory at all (Arch). Removed again, exactly, by
/// [`remove_config`].
const MAIN_CONF_MARK: &str =
    "# Added by OpenScreenTime: its website rules. Removed when it leaves this computer.";

/// Is dnsmasq installed *as a service* here — what serving the website rules
/// needs (see [`DNSMASQ_UNIT`])?
pub fn resolver_installed(exec: &Exec) -> bool {
    exec.has("dnsmasq")
        && exec
            .probe(
                "systemctl",
                &["show", "-p", "LoadState", "--value", DNSMASQ_UNIT],
            )
            .trim()
            != "not-found"
}

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
    /// The ruleset itself could not be written: dnsmasq keeps serving what
    /// it had (the previous rules, or none).
    RulesNotWritten,
    /// `/etc/resolv.conf` could not be pointed at the local resolver, so
    /// programs ask their usual DNS and nothing is filtered.
    ResolvConfNotPinned,
    /// Safe search is on, but a search engine's safe-search front end could
    /// not be looked up: its names pass through (the site works, without
    /// safe search) rather than being pointed at nothing.
    SafeSearchUnavailable,
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
            DnsGap::RulesNotWritten => "dns_rules_not_written",
            DnsGap::ResolvConfNotPinned => "dns_resolv_conf_not_pinned",
            DnsGap::SafeSearchUnavailable => "dns_safesearch_unavailable",
        }
    }

    /// With this gap standing, nothing on this computer resolves names
    /// through a running local resolver — so forcing all DNS there
    /// (`force_dns`) would leave it with no DNS at all.
    pub fn breaks_forced_dns(self) -> bool {
        matches!(
            self,
            DnsGap::ResolverMissing | DnsGap::NoLocalResolver | DnsGap::ResolvConfNotPinned
        )
    }

    /// Operator-facing explanation, in the terms the parent/admin needs.
    pub fn explain(self) -> &'static str {
        match self {
            DnsGap::ResolverMissing => {
                "the dnsmasq service is not installed (a desktop's NetworkManager \
                 only brings the program, not the service), so websites are not \
                 filtered on this computer. It stays online with its own DNS, and \
                 screen time still works. Re-run the install command (it installs \
                 dnsmasq and nftables), or install the dnsmasq package yourself."
            }
            DnsGap::NoLocalResolver => {
                "no local resolver is listening on 127.0.0.1 — dnsmasq failed to \
                 start, so the DNS allowlist is not filtering anything. This \
                 computer keeps its own DNS meanwhile. Check \
                 `systemctl status dnsmasq` on it."
            }
            DnsGap::ResolvConfNotAFile => {
                "/etc/resolv.conf is a symlink owned by another service \
                 (systemd-resolved or resolvconf), which replaces it on every \
                 network change, and the agent can't make it a file from inside \
                 its sandbox — so the filter comes and goes. Re-run the install \
                 command: it makes it a file the filter owns (and puts the link \
                 back when OpenScreenTime leaves)."
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
                 Re-run the install command, or add \
                 `conf-dir=/etc/openscreentime/dnsmasq.d` to this host's \
                 dnsmasq.conf."
            }
            DnsGap::RulesNotWritten => {
                "the website rules could not be written for dnsmasq, so it keeps \
                 serving the rules it had before (or none). Screen time and the \
                 firewall are not affected. The agent's log \
                 (`journalctl -u openscreentime-agent`) says why."
            }
            DnsGap::ResolvConfNotPinned => {
                "/etc/resolv.conf could not be pointed at the local resolver, so \
                 programs on this computer ask their usual DNS and websites are \
                 not filtered. It stays online, and screen time still works. The \
                 agent's log (`journalctl -u openscreentime-agent`) says why."
            }
            DnsGap::SafeSearchUnavailable => {
                "safe search is on, but the address of a search engine's \
                 safe-search front end (forcesafesearch.google.com, \
                 restrict.youtube.com, strict.bing.com or safe.duckduckgo.com) \
                 could not be looked up from the family DNS, so that engine \
                 opens without safe search rather than not at all. Websites \
                 are still filtered. The agent tries again every minute and \
                 whenever the network changes."
            }
        }
    }
}

/// How dnsmasq on this computer comes to read [`OST_CONF_DIR`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum Include {
    /// It reads this distro directory — Debian and Ubuntu's `/etc/dnsmasq.d`
    /// (from `CONFIG_DIR` in /etc/default/dnsmasq), or a `conf-dir=` in
    /// dnsmasq.conf (Fedora) — where a one-line stub points at ours.
    Stub(String),
    /// dnsmasq.conf names our directory itself ([`wire_main_conf`], on a
    /// distro whose dnsmasq reads no directory — Arch).
    MainConf,
    /// Nothing: dnsmasq serves its stock config.
    Nothing,
}

/// The directories named by active `conf-dir=` lines (the part before the
/// first comma, which lists extensions), without a trailing slash.
fn active_conf_dirs(conf: &str) -> Vec<String> {
    conf.lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("conf-dir="))
        .filter_map(|v| v.split(',').next())
        .map(|d| d.trim().trim_end_matches('/').to_string())
        .filter(|d| !d.is_empty())
        .collect()
}

fn include_route(exec: &Exec) -> Include {
    let main = exec.read_file(DNSMASQ_MAIN_CONF).unwrap_or_default();
    let dirs = active_conf_dirs(&main);
    if dirs.iter().any(|d| d == OST_CONF_DIR) {
        return Include::MainConf;
    }
    // Debian/Ubuntu: `CONFIG_DIR=/etc/dnsmasq.d,.dpkg-dist,…`, passed as -7.
    if let Some(dir) = exec.read_file(DEBIAN_DEFAULTS).and_then(|d| {
        d.lines()
            .map(str::trim)
            .filter_map(|l| l.strip_prefix("CONFIG_DIR="))
            .next_back()
            .and_then(|v| v.trim_matches('"').split(',').next().map(str::to_string))
    }) {
        let dir = dir.trim().trim_end_matches('/').to_string();
        if !dir.is_empty() {
            return Include::Stub(dir);
        }
    }
    dirs.into_iter()
        .find(|d| DISTRO_CONF_DIRS.contains(&d.as_str()))
        .map(Include::Stub)
        .unwrap_or(Include::Nothing)
}

/// On a distro whose dnsmasq reads no directory at all (Arch's dnsmasq.conf
/// is all comments), name ours in dnsmasq.conf — two marked lines,
/// [`remove_config`] takes exactly them out again. Outside the agent's
/// sandbox only (install-service): the agent can't write /etc/dnsmasq.conf.
pub fn wire_main_conf(exec: &Exec) -> Result<()> {
    if include_route(exec) != Include::Nothing {
        return Ok(());
    }
    let Some(main) = exec.read_file(DNSMASQ_MAIN_CONF) else {
        return Ok(()); // no dnsmasq.conf: nothing installed to wire
    };
    let mut body = main;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&format!("{MAIN_CONF_MARK}\nconf-dir={OST_CONF_DIR}\n"));
    exec.write_file(DNSMASQ_MAIN_CONF, &body)
}

/// dnsmasq is here but reads nothing of ours, and [`wire_main_conf`] would
/// fix that.
pub fn needs_wiring(exec: &Exec) -> bool {
    resolver_installed(exec)
        && include_route(exec) == Include::Nothing
        && exec.read_file(DNSMASQ_MAIN_CONF).is_some()
}

/// dnsmasq.conf without the lines [`wire_main_conf`] added.
fn unwired(main: &str) -> Option<String> {
    let ours = format!("conf-dir={OST_CONF_DIR}");
    let kept: Vec<&str> = main
        .lines()
        .filter(|l| l.trim() != MAIN_CONF_MARK && l.trim() != ours)
        .collect();
    (kept.len() != main.lines().count()).then(|| {
        let mut s = kept.join("\n");
        if main.ends_with('\n') {
            s.push('\n');
        }
        s
    })
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
    let dir = match include_route(exec) {
        Include::MainConf => return None,
        Include::Stub(dir) => dir,
        // Under --dry-run nothing exists to probe; log the intent, claim no gap.
        Include::Nothing if exec.dry_run() => return None,
        Include::Nothing => return Some(DnsGap::PolicyNotLoaded),
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
/// and reads our directory (Debian and Ubuntu start it at once, next to
/// systemd-resolved). Idempotent; never overwrites a ruleset.
pub fn preseed(exec: &Exec) -> Result<()> {
    let dir = DISTRO_CONF_DIRS[0];
    exec.write_file(&format!("{dir}/{INCLUDE_STUB}"), &include_stub_body())?;
    if exec.read_file(DNSMASQ_CONF).is_none() {
        exec.write_file(DNSMASQ_CONF, BOOTSTRAP_CONF)?;
    }
    Ok(())
}

/// Take the ruleset out of dnsmasq (retirement): remove our include stubs,
/// the lines we added to dnsmasq.conf and the ruleset, and restart dnsmasq
/// only if it is running, so it goes back to its own config. resolv.conf is
/// [`unpin_resolv_conf`]'s job.
pub fn remove_config(exec: &Exec) {
    for dir in DISTRO_CONF_DIRS {
        let _ = exec.remove_file(&format!("{dir}/{INCLUDE_STUB}"));
    }
    if let Some(main) = exec
        .read_file(DNSMASQ_MAIN_CONF)
        .as_deref()
        .and_then(unwired)
    {
        if let Err(e) = exec.write_file(DNSMASQ_MAIN_CONF, &main) {
            tracing::warn!("could not take our lines out of {DNSMASQ_MAIN_CONF}: {e}");
        }
    }
    let _ = exec.remove_file(DNSMASQ_CONF);
    let _ = exec.run("systemctl", &["try-restart", "dnsmasq"]);
}

/// Is a local resolver actually answering? A rendered allowlist that nothing
/// serves is worse than no allowlist, because the console reports it as applied.
fn local_resolver_running(exec: &Exec) -> bool {
    exec.probe("systemctl", &["is-active", "dnsmasq"]).trim() == "active"
}

/// The name the agent's block self-check resolves (`runner::probe_enforcement`).
/// Every ruleset answers it with the block address, so resolving it through
/// the system resolver proves this computer's rules are what answers —
/// without ever looking up a real blocked site (that lookup once read as the
/// child's browsing: "123movies.to is blocked on this computer" to someone
/// who never went there, "123movies.to 37×" to their parent). Attribution
/// and the blocked-site notice never count it.
///
/// `.internal` (reserved for private use, never delegated), not `.invalid`:
/// systemd-resolved answers `.invalid` itself and never asks dnsmasq, so a
/// computer whose resolved forwards to the rules would fail its own test.
pub const SELFTEST_NAME: &str = "selftest.openscreentime.internal";

/// Is `name` the self-check's name (see [`SELFTEST_NAME`])?
pub fn is_selftest(name: &str) -> bool {
    name.trim_end_matches('.')
        .eq_ignore_ascii_case(SELFTEST_NAME)
}

/// A domain safe to interpolate into a resolver directive: rejects anything
/// with a control char, slash, whitespace or a config-directive shape.
/// Wildcard and leading-dot forms (`*.example.org`, `.example.org`) mean the
/// domain and everything under it — what dnsmasq's `address=/d/` does anyway.
fn clean_domain(raw: &str) -> Option<String> {
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
}

/// The domains this computer's own rules answer as blocked — the policy's
/// blocklist, the app / category / focus-hours sinkhole, Tor's when Tor is
/// blocked: what [`render_rules`] writes as `address=/d/0.0.0.0`. Each one
/// blocks itself and every name under it. What the family resolver upstream
/// happens to filter is not in here: that isn't this computer's rule, and
/// it isn't ours to name ("mozilla.org is blocked" was Firefox's background
/// `ads.mozilla.org`, filtered upstream — mozilla.org itself worked).
pub fn block_rules(
    dns: &DnsPolicy,
    lockdown: &NetworkLockdown,
    sinkhole: &[String],
) -> Vec<String> {
    let mut rules: Vec<String> = dns
        .blocklist
        .iter()
        .chain(sinkhole)
        .filter_map(|d| clean_domain(d))
        .map(|d| d.to_ascii_lowercase())
        .collect();
    if lockdown.block_tor {
        rules.push("torproject.org".to_string());
    }
    rules.sort();
    rules.dedup();
    rules
}

/// The rule in `rules` ([`block_rules`]) that blocks `name` — the most
/// specific one: `www.example.org` under the rule `example.org` is
/// `example.org`. A name no rule covers is `None`.
pub fn blocking_rule<'a>(name: &str, rules: &'a [String]) -> Option<&'a str> {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    rules
        .iter()
        .filter(|r| {
            name == **r
                || name
                    .strip_suffix(r.as_str())
                    .is_some_and(|head| head.ends_with('.'))
        })
        .max_by_key(|r| r.len())
        .map(String::as_str)
}

/// The family resolver a malformed upstream falls back to (malware + adult).
const FALLBACK_UPSTREAM: &str = "1.1.1.3";

/// The upstream dnsmasq forwards to — the policy's, when it is an address.
pub fn upstream_ip(dns: &DnsPolicy) -> std::net::IpAddr {
    dns.upstream
        .parse()
        .unwrap_or_else(|_| FALLBACK_UPSTREAM.parse().expect("an address"))
}

/// The ruleset alone (tests).
#[cfg(test)]
pub fn render_dnsmasq(
    dns: &DnsPolicy,
    lockdown: &NetworkLockdown,
    server_host: Option<&str>,
    sinkhole: &[String],
    safe: &SafeSearch,
) -> String {
    render_rules(dns, lockdown, server_host, sinkhole, safe).0
}

/// Build the dnsmasq ruleset that realizes the policy. `sinkhole` is the
/// expanded app/category block list (subdomains included by dnsmasq's
/// `address=/d/` semantics): each name answers 0.0.0.0 / :: — an app that
/// cannot resolve its servers is an app that does not work. `safe`: the
/// safe-search front ends' addresses (`safesearch`), when safe search is on.
/// Also returns the search engines safe search could not be forced on (their
/// front end wasn't resolved: passed through).
fn render_rules(
    dns: &DnsPolicy,
    lockdown: &NetworkLockdown,
    server_host: Option<&str>,
    sinkhole: &[String],
    safe: &SafeSearch,
) -> (String, Vec<&'static str>) {
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
        FALLBACK_UPSTREAM
    };
    // Every domain interpolated below goes through `clean_domain` — the same
    // discipline as the catalog sinkhole.
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

    // The block self-check's own name ([`SELFTEST_NAME`]): answered as
    // blocked by every ruleset, default-deny's catch-all included.
    out.push_str("# the agent's own block self-check (no one visits this name)\n");
    out.push_str(&format!(
        "address=/{SELFTEST_NAME}/0.0.0.0\naddress=/{SELFTEST_NAME}/::\n"
    ));

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

    let mut unavailable = Vec::new();
    if dns.safe_search {
        // The engines' own names answer with their safe-search front end's
        // addresses (never a `cname=` to it: dnsmasq can't follow a CNAME to
        // a name it only learns from upstream — see `safesearch`). A name a
        // rule blocks stays blocked; under default-deny only allowlisted
        // names are answered at all.
        let blocked: Vec<String> = sinkhole
            .iter()
            .cloned()
            .chain(dns.blocklist.iter().filter_map(|b| clean_domain(b)))
            .collect();
        let deny = dns.is_default_deny() && !dns.allows_everything();
        let allow: Vec<String> = dns
            .allowlist
            .iter()
            .filter_map(|a| clean_domain(a))
            .collect();
        let allowed = |h: &str| {
            !blocked.iter().any(|b| safesearch::under(h, b))
                && (!deny || allow.iter().any(|a| safesearch::under(h, a)))
        };
        let (lines, missing) = safesearch::render(safe, &allowed);
        out.push_str("# safe search enforced\n");
        out.push_str(&lines);
        unavailable = missing;
    }
    (out, unavailable)
}

pub fn render_resolv_conf() -> String {
    format!(
        "# Managed by openscreentime — pinned. Do not edit.\nnameserver {LOCAL_RESOLVER}\noptions edns0 trust-ad\n"
    )
}

/// Apply DNS policy and pin resolv.conf.
///
/// Returns the [`DnsGap`]s that stop this host from actually enforcing the
/// policy. An empty vec means DNS is genuinely in force. Never fails: a step
/// that can't be done is a gap, and the steps after it still run.
pub fn apply(
    exec: &Exec,
    dns: &DnsPolicy,
    lockdown: &NetworkLockdown,
    server_host: Option<&str>,
    sinkhole: &[String],
    safe: &SafeSearch,
) -> Vec<DnsGap> {
    let mut gaps = Vec::new();
    let (conf, unavailable) = render_rules(dns, lockdown, server_host, sinkhole, safe);
    // Not looked up yet (a first start, before the lookup a moment later) is
    // not a gap; a lookup that found nothing is.
    if !unavailable.is_empty() && safe.attempted() {
        tracing::warn!(
            "safe search not forced on {} (front end not resolved): passed through",
            unavailable.join(", ")
        );
        gaps.push(DnsGap::SafeSearchUnavailable);
    }
    if let Err(e) = exec.write_file(DNSMASQ_CONF, &conf) {
        tracing::error!("could not write the DNS ruleset: {e:#}");
        gaps.push(DnsGap::RulesNotWritten);
    }

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
    let installed = !exec.observes() || resolver_installed(exec);
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
        match pin_resolv_conf(exec) {
            Ok(g) => gaps.extend(g),
            // Still our pin from before (it couldn't be rewritten, but it
            // points at the resolver): nothing is lost.
            Err(e) if is_our_pin(&exec.read_file(RESOLV_CONF).unwrap_or_default()) => {
                tracing::warn!("could not rewrite /etc/resolv.conf, the pin stands: {e:#}");
            }
            Err(e) => {
                tracing::error!("could not pin /etc/resolv.conf: {e:#}");
                gaps.push(DnsGap::ResolvConfNotPinned);
            }
        }
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
    gaps
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

fn saved_pre_pin(exec: &Exec) -> Option<PrePin> {
    exec.read_file(&pre_pin_path())
        .and_then(|s| serde_json::from_str::<PrePin>(&s).ok())
}

fn keep_pre_pin(exec: &Exec, saved: &PrePin) {
    if let Ok(json) = serde_json::to_string(saved) {
        if let Err(e) = exec.write_file(&pre_pin_path(), &json) {
            tracing::warn!("could not keep resolv.conf's previous contents: {e}");
        }
    }
}

/// Make /etc/resolv.conf a file of ours to pin where it is another service's
/// symlink — systemd-resolved's stub (Ubuntu, Fedora, Debian cloud images),
/// resolvconf's. The agent can't replace a symlink from inside its sandbox,
/// and writing through it lands in that service's file on /run, which the
/// service replaces on the next network change with a new file the sandbox
/// can't write: the filter silently off — and with forced DNS, no DNS at
/// all. So this runs outside the sandbox (install-service, `__refresh-units`).
/// The link is kept, and comes back when the pin goes ([`unpin_resolv_conf`]
/// outside the sandbox: the retirement). Returns whether it changed anything.
pub fn own_resolv_conf(exec: &Exec) -> Result<bool> {
    let Some(target) = exec.read_link(RESOLV_CONF) else {
        return Ok(false);
    };
    let content = exec.read_file(RESOLV_CONF).unwrap_or_default();
    keep_pre_pin(
        exec,
        &PrePin {
            symlink: Some(target.clone()),
            content: content.clone(),
        },
    );
    // Its contents as a file: DNS goes on working exactly as before until
    // the agent pins it to the local resolver.
    let body = if content.contains("nameserver") {
        content
    } else {
        FALLBACK_RESOLV.to_string()
    };
    exec.remove_file(RESOLV_CONF)?;
    exec.write_file(RESOLV_CONF, &body)?;
    tracing::info!("/etc/resolv.conf was a link to {target}; it is a file now (the link comes back when OpenScreenTime leaves)");
    Ok(true)
}

/// Is /etc/resolv.conf another service's symlink that [`own_resolv_conf`]
/// would make ours?
pub fn resolv_conf_is_a_link(exec: &Exec) -> bool {
    exec.read_link(RESOLV_CONF).is_some()
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
    let link = exec.read_link(RESOLV_CONF);
    if !current.trim().is_empty() && !is_our_pin(&current) {
        // A link [`own_resolv_conf`] replaced is still the one to put back.
        let symlink = link
            .clone()
            .or_else(|| saved_pre_pin(exec).and_then(|p| p.symlink));
        keep_pre_pin(
            exec,
            &PrePin {
                symlink,
                content: current,
            },
        );
    }

    // If the path is a symlink, another service owns it. Writing through the
    // link lands in *that* service's file — typically on tmpfs, where the
    // immutable bit does not exist — so `chattr +i` silently no-ops and the
    // owner rewrites our nameserver on the next network change. Replace the
    // link with a real file we control (outside the agent's sandbox only —
    // install-service does it first, see [`own_resolv_conf`]).
    if link.is_some() {
        gaps.push(DnsGap::ResolvConfNotAFile);
        let _ = exec.remove_file(RESOLV_CONF);
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
/// that isn't our pin — except put back a symlink that was there before
/// (systemd-resolved's), which only works outside the agent's sandbox: the
/// agent puts back its contents meanwhile, the retirement the link itself.
/// Returns whether it changed anything.
pub fn unpin_resolv_conf(exec: &Exec) -> bool {
    let current = exec.read_file(RESOLV_CONF).unwrap_or_default();
    let ours = is_our_pin(&current);
    let saved = saved_pre_pin(exec);
    let link_to_restore = saved.as_ref().is_some_and(|p| p.symlink.is_some())
        && exec.read_link(RESOLV_CONF).is_none();
    if !ours && !link_to_restore {
        return false;
    }
    let _ = exec.run("chattr", &["-i", RESOLV_CONF]);
    if let Some(saved) = saved.filter(|p| p.symlink.is_some() || !is_our_pin(&p.content)) {
        // A symlink comes back as the symlink where that's possible (outside
        // the agent's sandbox); otherwise its contents do.
        let relinked = saved.symlink.as_deref().is_some_and(|target| {
            !exec.dry_run()
                && std::path::Path::new(target).exists()
                && std::fs::remove_file(RESOLV_CONF).is_ok()
                && std::os::unix::fs::symlink(target, RESOLV_CONF).is_ok()
        });
        if relinked {
            let _ = exec.remove_file(&pre_pin_path());
            tracing::warn!("resolv.conf un-pinned: it is {}'s link again", {
                saved.symlink.as_deref().unwrap_or_default()
            });
            return true;
        }
        if !ours {
            return false; // the pin is off already; the link waits for the retirement
        }
        if !is_our_pin(&saved.content) && exec.write_file(RESOLV_CONF, &saved.content).is_ok() {
            // A link still to come back keeps its record.
            if saved.symlink.is_none() {
                let _ = exec.remove_file(&pre_pin_path());
            }
            tracing::warn!("resolv.conf un-pinned: put back what this computer used before");
            return true;
        }
    }
    if !ours {
        return false;
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
        let conf = render_dnsmasq(
            &dns,
            &NetworkLockdown::default(),
            None,
            &[],
            &SafeSearch::default(),
        );
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
        let conf = render_dnsmasq(
            &dns,
            &NetworkLockdown::default(),
            None,
            &[],
            &SafeSearch::default(),
        );
        assert!(conf.contains("server=/wikipedia.org/1.1.1.2"));
        assert!(conf.contains("server=/edu/1.1.1.2"));
        assert!(conf.contains("address=/#/"));
        // Not looked up yet: nothing is redirected (and nothing un-denied).
        assert!(!conf.contains("host-record="));
        assert!(!conf.contains("cname="));
    }

    /// Acceptance round 3: `cname=www.google.com,forcesafesearch.google.com`
    /// answered with the CNAME and no address — Google, YouTube and Bing
    /// stopped resolving. Now every covered name answers with the front
    /// end's own addresses.
    #[test]
    fn safe_search_answers_with_addresses_never_a_bare_cname() {
        let dns = DnsPolicy {
            mode: "allow_all".into(),
            allowlist: vec!["*".into()],
            blocklist: vec![],
            safe_search: true,
            upstream: "1.1.1.3".into(),
        };
        let safe = safesearch::tests::resolved();
        let conf = render_dnsmasq(&dns, &NetworkLockdown::default(), None, &[], &safe);
        assert!(!conf.contains("cname="));
        assert!(conf.contains("host-record=www.google.com,216.239.38.120,2001:4860:4802:32::78\n"));
        assert!(conf.contains("host-record=www.youtube.com,216.239.38.120\n"));
        assert!(conf.contains("host-record=www.bing.com,150.171.27.16,2620:1ec:33::16\n"));
        assert!(conf.contains("host-record=duckduckgo.com,40.114.177.246\n"));
        // Everything else still goes to the upstream.
        assert!(conf.contains("server=1.1.1.3\n"));

        // Safe search off: none of it.
        let off = DnsPolicy {
            safe_search: false,
            ..dns.clone()
        };
        let conf = render_dnsmasq(&off, &NetworkLockdown::default(), None, &[], &safe);
        assert!(!conf.contains("host-record="));

        // A blocked YouTube stays blocked; a custom block on a Google domain too.
        let blocks = crate::policy::AppBlocks {
            apps: vec!["youtube".into()],
            ..Default::default()
        };
        let sinkhole = openscreentime_policy::catalog::expand(&blocks).domains;
        let blocked = DnsPolicy {
            blocklist: vec!["google.de".into()],
            ..dns.clone()
        };
        let conf = render_dnsmasq(
            &blocked,
            &NetworkLockdown::default(),
            None,
            &sinkhole,
            &safe,
        );
        assert!(conf.contains("address=/youtube.com/0.0.0.0"));
        assert!(!conf.contains("host-record=www.youtube.com"));
        assert!(!conf.contains("host-record=m.youtube.com"));
        assert!(!conf.contains("host-record=www.google.de,"));
        assert!(conf.contains("host-record=www.google.com,"));

        // Default-deny: only allowlisted names are answered at all.
        let deny = DnsPolicy {
            mode: "default_deny".into(),
            allowlist: vec!["google.com".into()],
            ..dns.clone()
        };
        let conf = render_dnsmasq(&deny, &NetworkLockdown::default(), None, &[], &safe);
        assert!(conf.contains("host-record=www.google.com,"));
        assert!(!conf.contains("host-record=www.google.co.uk"));
        assert!(!conf.contains("host-record=www.bing.com"));
    }

    /// A front end that couldn't be resolved is passed through and said; one
    /// never looked up yet (the first start) is not a gap.
    #[test]
    fn unresolved_safe_search_is_a_gap_only_after_a_lookup() {
        let dns = DnsPolicy {
            mode: "allow_all".into(),
            allowlist: vec!["*".into()],
            blocklist: vec![],
            safe_search: true,
            upstream: "1.1.1.3".into(),
        };
        let lockdown = NetworkLockdown::default();
        let exec = Exec::simulated(&[], &[("systemctl is-active dnsmasq", "active\n")]);
        let fresh = SafeSearch::default();
        assert!(!apply(&exec, &dns, &lockdown, None, &[], &fresh)
            .contains(&DnsGap::SafeSearchUnavailable));
        let mut failed = safesearch::tests::resolved();
        let mut round = safesearch::Round::new();
        round.insert("strict.bing.com".into(), Some(safesearch::Addrs::default()));
        failed.merge("1.1.1.3", &round, 5);
        let gaps = apply(&exec, &dns, &lockdown, None, &[], &failed);
        assert!(gaps.contains(&DnsGap::SafeSearchUnavailable));
        assert!(!DnsGap::SafeSearchUnavailable.breaks_forced_dns());
        let conf = exec.written(DNSMASQ_CONF).unwrap_or_default();
        assert!(!conf.contains("www.bing.com"), "Bing passes through");
        assert!(conf.contains("host-record=www.google.com,"));
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
        let conf = render_dnsmasq(
            &dns,
            &NetworkLockdown::default(),
            None,
            &[],
            &SafeSearch::default(),
        );
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
        let conf = render_dnsmasq(
            &dns,
            &NetworkLockdown::default(),
            None,
            &sinkhole,
            &SafeSearch::default(),
        );
        assert!(conf.contains("address=/youtube.com/0.0.0.0\naddress=/youtube.com/::\n"));
        assert!(conf.contains("address=/googlevideo.com/0.0.0.0"));
        assert!(conf.contains("address=/pornhub.com/::"));
        assert!(conf.contains("address=/example.org/0.0.0.0"));
        // a hostile "domain" never lands in the config
        let bad = vec!["evil.com\nserver=9.9.9.9".to_string()];
        let conf = render_dnsmasq(
            &dns,
            &NetworkLockdown::default(),
            None,
            &bad,
            &SafeSearch::default(),
        );
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
        let conf = render_dnsmasq(&dns, &lockdown, None, &[], &SafeSearch::default());
        assert!(conf.contains("address=/onion/0.0.0.0"));
        assert!(conf.contains("address=/torproject.org/0.0.0.0"));
    }

    /// Acceptance round 5: the block self-check resolved the first blocked
    /// catalog domain every minute, so a child was told "123movies.to is
    /// blocked on this computer" and her parent saw "123movies.to 37×". The
    /// check has a name of its own now, answered as blocked by every ruleset
    /// (default-deny's catch-all included) — with or without any block.
    #[test]
    fn every_ruleset_answers_the_self_check_name_as_blocked() {
        let open = DnsPolicy {
            mode: "allow_all".into(),
            allowlist: vec!["*".into()],
            blocklist: vec![],
            safe_search: false,
            upstream: "1.1.1.3".into(),
        };
        let deny = DnsPolicy {
            mode: "default_deny".into(),
            allowlist: vec!["wikipedia.org".into()],
            ..open.clone()
        };
        for dns in [&open, &deny] {
            let conf = render_dnsmasq(
                dns,
                &NetworkLockdown::default(),
                None,
                &[],
                &SafeSearch::default(),
            );
            assert!(
                conf.contains(&format!(
                    "address=/{SELFTEST_NAME}/0.0.0.0\naddress=/{SELFTEST_NAME}/::\n"
                )),
                "{conf}"
            );
        }
        assert!(is_selftest("selftest.openscreentime.internal."));
        assert!(is_selftest("SELFTEST.openscreentime.internal"));
        assert!(!is_selftest("openscreentime.internal"));
        // …and it is never one of this computer's block rules (the notice
        // names only those).
        let rules = block_rules(&open, &NetworkLockdown::default(), &[]);
        assert!(blocking_rule(SELFTEST_NAME, &rules).is_none());
    }

    /// What "blocked on this computer" may name: this computer's own rules
    /// (blocklist, catalog sinkhole, Tor), matched to the rule itself — not a
    /// name the family resolver filters, not the registrable domain around a
    /// blocked subdomain.
    #[test]
    fn block_rules_name_the_rule_that_blocks() {
        let dns = DnsPolicy {
            mode: "allow_all".into(),
            allowlist: vec!["*".into()],
            blocklist: vec!["*.Example.org".into(), "ads.news.test".into()],
            safe_search: false,
            upstream: "1.1.1.3".into(),
        };
        let lockdown = NetworkLockdown {
            block_tor: true,
            ..Default::default()
        };
        let sinkhole = vec!["bet365.com".to_string(), "evil.com\nserver=1".to_string()];
        let rules = block_rules(&dns, &lockdown, &sinkhole);
        assert_eq!(
            rules,
            vec![
                "ads.news.test",
                "bet365.com",
                "example.org",
                "torproject.org"
            ]
        );
        assert_eq!(blocking_rule("www.bet365.com", &rules), Some("bet365.com"));
        assert_eq!(blocking_rule("bet365.com.", &rules), Some("bet365.com"));
        assert_eq!(
            blocking_rule("WWW.EXAMPLE.ORG", &rules),
            Some("example.org")
        );
        assert_eq!(
            blocking_rule("x.ads.news.test", &rules),
            Some("ads.news.test")
        );
        // The rest of news.test isn't blocked; neither is a look-alike.
        assert_eq!(blocking_rule("news.test", &rules), None);
        assert_eq!(blocking_rule("notbet365.com", &rules), None);
        // Filtered upstream (Firefox's ads.mozilla.org), not a rule here.
        assert_eq!(blocking_rule("ads.mozilla.org", &rules), None);
        // Tor only while Tor is blocked.
        let rules = block_rules(&dns, &NetworkLockdown::default(), &[]);
        assert_eq!(blocking_rule("www.torproject.org", &rules), None);
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
        assert_eq!(
            DnsGap::SafeSearchUnavailable.kind(),
            "dns_safesearch_unavailable"
        );
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
        let gaps = apply(
            &exec,
            &dns,
            &NetworkLockdown::default(),
            None,
            &[],
            &SafeSearch::default(),
        );
        assert_eq!(gaps, vec![DnsGap::ResolverMissing]);
        let log = exec.log();
        assert!(!log.iter().any(|l| l.contains("restart dnsmasq")));
        assert!(!log
            .iter()
            .any(|l| l == "write /etc/resolv.conf" || l.contains("chattr +i")));
    }

    /// Acceptance round 2: a Debian GNOME desktop has /usr/sbin/dnsmasq from
    /// `dnsmasq-base` (NetworkManager's), but no dnsmasq service. That is a
    /// computer without the resolver — one clear gap, not "restart failed" +
    /// "not loaded" — and it's what the installer installs the package for.
    #[test]
    fn nm_s_dnsmasq_program_without_the_service_is_no_resolver() {
        let base_only = [
            (
                "systemctl show -p LoadState --value dnsmasq.service",
                "not-found\n",
            ),
            ("read /etc/resolv.conf", NM_RESOLV),
        ];
        let exec = Exec::simulated(&[], &base_only);
        assert!(exec.has("dnsmasq"), "the program is there");
        assert!(!resolver_installed(&exec));
        let dns = DnsPolicy {
            mode: "allow_all".into(),
            allowlist: vec!["*".into()],
            blocklist: vec![],
            safe_search: true,
            upstream: "1.1.1.3".into(),
        };
        let gaps = apply(
            &exec,
            &dns,
            &NetworkLockdown::default(),
            None,
            &[],
            &SafeSearch::default(),
        );
        assert_eq!(gaps, vec![DnsGap::ResolverMissing]);
        assert!(!exec.log().iter().any(|l| l.contains("restart dnsmasq")));
        // The real package: the unit is there.
        let exec = Exec::simulated(
            &[],
            &[(
                "systemctl show -p LoadState --value dnsmasq.service",
                "loaded\n",
            )],
        );
        assert!(resolver_installed(&exec));
    }

    const STUB: &str = "../run/systemd/resolve/stub-resolv.conf";
    const STUB_RESOLV: &str =
        "# This is /run/systemd/resolve/stub-resolv.conf\nnameserver 127.0.0.53\n";

    /// Acceptance round 2 (and the container proof): /etc/resolv.conf was
    /// systemd-resolved's link. The agent pinned it by writing through the
    /// link into resolved's file, which resolved replaced on the next
    /// network change — the filter off, and with forced DNS no DNS at all,
    /// while the sandboxed agent's re-pin failed every 10 s. The installer
    /// makes it a file first (keeping the link), the pin keeps the link on
    /// record, and the link comes back only where it can: outside the sandbox.
    #[test]
    fn systemd_resolved_s_link_becomes_a_file_and_comes_back() {
        let exec = Exec::simulated(
            &[],
            &[
                ("readlink /etc/resolv.conf", STUB),
                ("read /etc/resolv.conf", STUB_RESOLV),
            ],
        );
        assert!(resolv_conf_is_a_link(&exec));
        assert!(own_resolv_conf(&exec).unwrap());
        assert_eq!(
            exec.log(),
            [
                "write /var/lib/openscreentime/resolv.conf.pre-pin",
                "remove /etc/resolv.conf",
                "write /etc/resolv.conf",
            ]
        );
        assert_eq!(exec.written(RESOLV_CONF).as_deref(), Some(STUB_RESOLV));
        let kept = exec.written(&pre_pin_path()).unwrap();
        // A file already: nothing to do.
        let file = Exec::simulated(&[], &[("read /etc/resolv.conf", STUB_RESOLV)]);
        assert!(!own_resolv_conf(&file).unwrap() && file.log().is_empty());

        // The agent pins the file; the link stays on record.
        let agent = Exec::simulated(
            &[],
            &[
                ("read /etc/resolv.conf", STUB_RESOLV),
                ("read /var/lib/openscreentime/resolv.conf.pre-pin", &kept),
            ],
        );
        assert_eq!(pin_resolv_conf(&agent).unwrap(), vec![], "a file: no gap");
        let record: PrePin =
            serde_json::from_str(&agent.written(&pre_pin_path()).unwrap()).unwrap();
        assert_eq!(record.symlink.as_deref(), Some(STUB));
        let record = serde_json::to_string(&record).unwrap();

        // Inside the sandbox the pin comes off as the stub's contents, and
        // the record waits for the link to come back…
        let pinned = render_resolv_conf();
        let agent = Exec::simulated(
            &[],
            &[
                ("read /etc/resolv.conf", &pinned),
                ("read /var/lib/openscreentime/resolv.conf.pre-pin", &record),
            ],
        );
        assert!(unpin_resolv_conf(&agent));
        assert_eq!(agent.written(RESOLV_CONF).as_deref(), Some(STUB_RESOLV));
        assert!(
            !agent.log().iter().any(|l| l.contains("remove")),
            "{:?}",
            agent.log()
        );
        // …which a dry run can't do either: nothing claimed.
        let later = Exec::simulated(
            &[],
            &[
                ("read /etc/resolv.conf", STUB_RESOLV),
                ("read /var/lib/openscreentime/resolv.conf.pre-pin", &record),
            ],
        );
        assert!(!unpin_resolv_conf(&later));
        // Once it is the link again, there's nothing left to do.
        let relinked = Exec::simulated(
            &[],
            &[
                ("readlink /etc/resolv.conf", STUB),
                ("read /etc/resolv.conf", STUB_RESOLV),
                ("read /var/lib/openscreentime/resolv.conf.pre-pin", &record),
            ],
        );
        assert!(!unpin_resolv_conf(&relinked) && relinked.log().is_empty());
    }

    /// Where dnsmasq reads our rules from, per distro — and nowhere means a
    /// gap, even when an /etc/dnsmasq.d exists that this dnsmasq never reads.
    #[test]
    fn the_include_follows_what_this_dnsmasq_really_reads() {
        const DEBIAN_CONF: &str = "# Configuration file for dnsmasq.\n#conf-dir=/etc/dnsmasq.d\n";
        const DEBIAN_DEFAULTS_FILE: &str =
            "ENABLED=1\nCONFIG_DIR=/etc/dnsmasq.d,.dpkg-dist,.dpkg-old,.dpkg-new\n";
        let debian = Exec::simulated(
            &[],
            &[
                ("read /etc/dnsmasq.conf", DEBIAN_CONF),
                ("read /etc/default/dnsmasq", DEBIAN_DEFAULTS_FILE),
            ],
        );
        assert_eq!(
            include_route(&debian),
            Include::Stub("/etc/dnsmasq.d".into())
        );
        let fedora = Exec::simulated(
            &[],
            &[(
                "read /etc/dnsmasq.conf",
                "user=dnsmasq\nconf-dir=/etc/dnsmasq.d,.rpmnew,.rpmsave,.rpmorig\n",
            )],
        );
        assert_eq!(
            include_route(&fedora),
            Include::Stub("/etc/dnsmasq.d".into())
        );

        // Arch: all comments. Nothing reads us until dnsmasq.conf names us.
        const ARCH_CONF: &str =
            "# Configuration file for dnsmasq.\n#conf-dir=/etc/dnsmasq.d/,*.conf\n";
        let arch = Exec::simulated(&[], &[("read /etc/dnsmasq.conf", ARCH_CONF)]);
        assert_eq!(include_route(&arch), Include::Nothing);
        wire_main_conf(&arch).unwrap();
        assert_eq!(arch.log(), ["write /etc/dnsmasq.conf"]);
        let wired = format!("{ARCH_CONF}{MAIN_CONF_MARK}\nconf-dir={OST_CONF_DIR}\n");
        let arch = Exec::simulated(&[], &[("read /etc/dnsmasq.conf", wired.as_str())]);
        assert_eq!(include_route(&arch), Include::MainConf);
        assert_eq!(ensure_include(&arch), None);
        // …and the lines come out again, exactly, leaving the rest as it was.
        assert_eq!(unwired(&wired).as_deref(), Some(ARCH_CONF));
        assert_eq!(unwired(ARCH_CONF), None, "nothing of ours: untouched");
        // Wiring is idempotent and never touches Debian or Fedora.
        wire_main_conf(&arch).unwrap();
        wire_main_conf(&debian).unwrap();
        assert!(arch.log().is_empty() && debian.log().is_empty());
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
            DnsGap::RulesNotWritten,
            DnsGap::ResolvConfNotPinned,
        ] {
            assert!(gap.explain().len() > 40, "{} has no guidance", gap.kind());
        }
    }
}
