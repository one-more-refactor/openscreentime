//! Where the time goes — the attribution sampler (CONTRACT-0.6 §3).
//!
//! Two signals, both honest about what they are:
//!
//! - **apps**: every tick, one `/proc` walk. A process is an app when it is a
//!   catalog app (`catalog::comm_to_app`, not just blocked ones — the key is
//!   the catalog id) or any **desktop application** this computer has: an
//!   entry a launcher shows (`.desktop`, see [`DesktopIndex`]) — the key is
//!   its name, "Firefox ESR", "Text Editor". An app earns tick-seconds while
//!   it is *running* for a user who is *active on a seat* and not frozen —
//!   "open", not "focused"; root has no portable way to know compositor
//!   focus. (Acceptance round 3: with only the catalog, 37 minutes of Firefox
//!   and Text Editor were "Nothing yet today".) What the session bus starts
//!   on its own — GNOME's search providers, background services — counts
//!   only once it has stayed a minute ([`SERVICE_GRACE_SECS`]).
//! - **sites**: dnsmasq writes an extra-format query log (`dns.rs` enables
//!   it); we tail it, reduce each queried name to its registrable domain, and
//!   count queries per hour, device-wide — resolver traffic has no user.
//!
//! Slices accumulate in memory keyed by (user, hour, kind, key) and are
//! drained to `POST /agent/usage` about once a minute; a failed post keeps
//! the batch (bounded) for the next try.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

/// Where dnsmasq writes its query log (see `enforce/dns.rs`).
pub const DNSQ_LOG: &str = "/var/lib/openscreentime/dnsq.log";
/// Truncate the query log once it grows past this — dnsmasq appends, so a
/// truncate under an O_APPEND writer is safe.
const TRUNCATE_AT: u64 = 20 * 1024 * 1024;
/// At most this many bytes are parsed per tick, so a burst can't stall a tick.
const READ_CAP: usize = 2 * 1024 * 1024;
/// Bounded memory: beyond this many distinct slices, new keys are dropped
/// (existing ones still accumulate) until a drain makes room.
const MAX_PENDING: usize = 2000;

/// Where desktop entries live: the system's, local installs, Flatpak's and
/// Snap's exports. (A user's own `~/.local/share/applications` is theirs to
/// write, so it can't name what their processes are.)
const APP_DIRS: &[&str] = &[
    "/usr/share/applications",
    "/usr/local/share/applications",
    "/var/lib/flatpak/exports/share/applications",
    "/var/lib/snapd/desktop/applications",
];
/// D-Bus service files: what a D-Bus-activated app really runs (Terminal's
/// window is `gnome-terminal-server`, not the `gnome-terminal` it launches).
const DBUS_DIRS: &[&str] = &[
    "/usr/share/dbus-1/services",
    "/usr/local/share/dbus-1/services",
];
/// What starts with every session. Those run in the background whether or
/// not anyone opened them (Software's updater, the calendar's alarms), so
/// they are never "open" — even when a launcher lists them too.
const AUTOSTART_DIRS: &[&str] = &["/etc/xdg/autostart"];
/// How long a read of the desktop entries is trusted (apps get installed).
const INDEX_TTL: Duration = Duration::from_secs(600);
/// How long an app the session bus started on its own (see [`bus_started`])
/// must stay before it counts — then from its start. GNOME wakes Files,
/// Characters and Disks as search providers while someone types in the
/// overview, with no window, and they leave again after 20–30 s (acceptance
/// round 4: "Files 1 min" for a person who never opened it). Someone who
/// really opens one keeps it longer than that.
const SERVICE_GRACE_SECS: i64 = 60;

/// Programs that run other programs — a shell, an interpreter, a sandbox, a
/// launcher — and OpenScreenTime itself. A desktop entry that runs one of
/// these names no app by its program, and a process running one is only an
/// app through its script (argv) or its unit.
fn is_runner(program: &str) -> bool {
    const RUNNERS: &[&str] = &[
        "sh",
        "bash",
        "dash",
        "zsh",
        "fish",
        "env",
        "exec",
        "nice",
        "ionice",
        "sudo",
        "pkexec",
        "perl",
        "ruby",
        "node",
        "nodejs",
        "java",
        "gjs",
        "lua",
        "wine",
        "mono",
        "flatpak",
        "bwrap",
        "snap",
        "xdg-open",
        "gio",
        "gtk-launch",
        "kioclient",
        "dbus-launch",
        "dbus-send",
        "gapplication",
        "openscreentime",
        "ost",
    ];
    RUNNERS.contains(&program) || program.starts_with("python") || program.starts_with("wine")
}

/// The file name of a path, without a `(deleted)` note (`/proc/<pid>/exe` of a
/// binary replaced by an update).
fn basename(path: &str) -> &str {
    let path = path.trim().trim_end_matches(" (deleted)");
    path.rsplit('/').next().unwrap_or(path)
}

/// One `[Desktop Entry]`, the keys attribution reads.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub name: String,
    pub exec: String,
    pub try_exec: String,
    /// `Type=Application` (the default when a file doesn't say).
    pub application: bool,
    /// `NoDisplay=true` or `Hidden=true`: no launcher shows it.
    pub hidden: bool,
    pub dbus_activatable: bool,
}

/// Parse the `[Desktop Entry]` group of a desktop file (the unlocalized keys).
pub fn parse_desktop(text: &str) -> DesktopEntry {
    let mut e = DesktopEntry {
        application: true,
        ..Default::default()
    };
    let mut in_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "Name" => e.name = v.to_string(),
            "Exec" => e.exec = v.to_string(),
            "TryExec" => e.try_exec = v.to_string(),
            "Type" => e.application = v == "Application",
            "NoDisplay" | "Hidden" if v == "true" => e.hidden = true,
            "DBusActivatable" => e.dbus_activatable = v == "true",
            _ => {}
        }
    }
    e
}

/// The programs an `Exec=` line can run, as file names: the command (past
/// `env VAR=…`), and for `flatpak run … --command=X` that command.
fn exec_programs(exec: &str) -> Vec<String> {
    let words: Vec<&str> = exec
        .split_whitespace()
        .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let b = basename(w);
        if b == "env" || (w.contains('=') && !w.starts_with('-')) || w.starts_with('-') {
            i += 1;
            continue;
        }
        out.push(b.to_string());
        break;
    }
    for w in &words {
        if let Some(c) = w.strip_prefix("--command=") {
            out.push(basename(c).to_string());
        }
    }
    out.retain(|p| !p.is_empty() && !p.starts_with('%') && !is_runner(p));
    out
}

/// A systemd unit an app runs in → its desktop id, per the XDG naming the
/// desktops follow: `app[-<launcher>]-<id>-<random>.scope`,
/// `app[-<launcher>]-<id>[@<random>].service`, and dbus-broker's
/// `dbus-:<bus>-<id>@<n>.service`. Dashes inside the id are escaped (`\x2d`).
pub fn unit_app_id(unit: &str) -> Option<String> {
    let (stem, scope) = if let Some(s) = unit.strip_suffix(".scope") {
        (s, true)
    } else {
        (unit.strip_suffix(".service")?, false)
    };
    let stem = if scope {
        stem.rsplit_once('-')?.0
    } else {
        stem.split_once('@').map_or(stem, |(s, _)| s)
    };
    let id = if let Some(rest) = stem.strip_prefix("dbus-:") {
        rest.split_once('-')?.1
    } else {
        let rest = stem.strip_prefix("app-")?;
        rest.rsplit('-').next()?
    };
    let id = crate::enforce::screentime::freeze::unescape(id);
    (!id.is_empty()).then_some(id)
}

/// The desktop applications of this computer, by what identifies their
/// processes: the program they run, and their desktop id (their unit's name).
#[derive(Debug, Default)]
pub struct DesktopIndex {
    by_program: HashMap<String, String>,
    by_id: HashMap<String, String>,
}

impl DesktopIndex {
    /// Build from `(desktop id, desktop file)` pairs, the D-Bus service files
    /// of D-Bus-activated ones (`id → service file`), and the session's
    /// autostart entries, which are left out.
    pub fn build(
        entries: &[(String, String)],
        dbus: &HashMap<String, String>,
        autostart: &[(String, String)],
    ) -> DesktopIndex {
        let mut skip_programs: HashSet<String> = HashSet::new();
        let mut skip_ids: HashSet<String> = HashSet::new();
        for (id, text) in autostart {
            let e = parse_desktop(text);
            skip_ids.insert(id.clone());
            skip_programs.extend(exec_programs(&e.exec));
        }
        let mut idx = DesktopIndex::default();
        for (id, text) in entries {
            let e = parse_desktop(text);
            if !e.application || e.hidden || e.name.is_empty() || skip_ids.contains(id) {
                continue;
            }
            let mut programs = exec_programs(&e.exec);
            programs.extend(exec_programs(&e.try_exec));
            if e.dbus_activatable {
                if let Some(service) = dbus.get(id) {
                    let exec = service
                        .lines()
                        .find_map(|l| l.trim().strip_prefix("Exec="))
                        .unwrap_or_default();
                    programs.extend(exec_programs(exec));
                }
            }
            if programs.iter().any(|p| skip_programs.contains(p)) {
                continue;
            }
            idx.by_id
                .entry(id.clone())
                .or_insert_with(|| e.name.clone());
            for p in programs {
                idx.by_program.entry(p).or_insert_with(|| e.name.clone());
            }
        }
        idx
    }

    /// Read this computer's desktop entries.
    pub fn read() -> DesktopIndex {
        fn files(dirs: &[&str], ext: &str) -> Vec<(String, String)> {
            let mut out: Vec<(String, String)> = Vec::new();
            for dir in dirs {
                let Ok(rd) = std::fs::read_dir(dir) else {
                    continue;
                };
                let mut paths: Vec<_> = rd.flatten().map(|e| e.path()).collect();
                paths.sort();
                for p in paths {
                    let Some(id) = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|n| n.strip_suffix(ext))
                    else {
                        continue;
                    };
                    if out.iter().any(|(i, _)| i == id) {
                        continue;
                    }
                    // Bounded: a desktop file is a few KiB.
                    if p.metadata().is_ok_and(|m| m.len() > 64 * 1024) {
                        continue;
                    }
                    if let Ok(text) = std::fs::read_to_string(&p) {
                        out.push((id.to_string(), text));
                    }
                }
            }
            out
        }
        let dbus: HashMap<String, String> = files(DBUS_DIRS, ".service").into_iter().collect();
        DesktopIndex::build(
            &files(APP_DIRS, ".desktop"),
            &dbus,
            &files(AUTOSTART_DIRS, ".desktop"),
        )
    }

    fn program(&self, p: &str) -> Option<&str> {
        self.by_program.get(p).map(String::as_str)
    }

    /// `comm` is the program's name cut to 15 bytes.
    fn comm(&self, comm: &str) -> Option<&str> {
        if comm.len() < 15 {
            return self.program(comm);
        }
        self.by_program
            .iter()
            .find(|(p, _)| p.len() >= 15 && p.as_bytes()[..15] == comm.as_bytes()[..15])
            .map(|(_, n)| n.as_str())
    }
}

/// What a process says about itself, as far as naming its app goes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProcApp {
    pub uid: u32,
    /// `/proc/<pid>/comm`.
    pub comm: String,
    /// The target of `/proc/<pid>/exe` (empty when unreadable).
    pub exe: String,
    pub argv: Vec<String>,
    /// The innermost unit of its cgroup (`app-gnome-firefox\x2desr-2345.scope`).
    pub unit: String,
}

/// The app a process is — a catalog id or a desktop app's name — or `None`
/// for everything else (the session, services, shells). In order: the
/// catalog by `comm`; the unit a launcher started it in; the program it runs
/// (its script, when that's an interpreter); its `comm`.
pub fn app_of(
    p: &ProcApp,
    catalog: &HashMap<&'static str, &'static str>,
    index: &DesktopIndex,
) -> Option<String> {
    if let Some(id) = catalog.get(p.comm.as_str()) {
        return Some((*id).to_string());
    }
    // A desktop app that is a catalog app too keeps its catalog id.
    let named = |program: &str, name: &str| -> String {
        catalog
            .get(program)
            .map_or_else(|| name.to_string(), |id| (*id).to_string())
    };
    if let Some(name) = unit_app_id(&p.unit).and_then(|id| index.by_id.get(&id)) {
        return Some(name.clone());
    }
    let exe = basename(&p.exe);
    if !exe.is_empty() && !is_runner(exe) {
        if let Some(name) = index.program(exe) {
            return Some(named(exe, name));
        }
    }
    // An interpreter running an app's script: `/usr/bin/python3 /usr/bin/foo`.
    let script = if exe.is_empty()
        || is_runner(exe)
        || p.argv.first().is_some_and(|a| is_runner(basename(a)))
    {
        p.argv.iter().skip(1).find(|a| !a.starts_with('-'))
    } else {
        p.argv.first()
    };
    if let Some(program) = script
        .map(|a| basename(a))
        .filter(|b| !b.is_empty() && !is_runner(b))
    {
        if let Some(name) = index.program(program) {
            return Some(named(program, name));
        }
    }
    if exe.is_empty() && !is_runner(&p.comm) {
        return index.comm(&p.comm).map(str::to_string);
    }
    None
}

/// Did the session bus start this process on its own, rather than a person
/// through a launcher? A GApplication started as a service
/// (`--gapplication-service`) outside a launcher's scope, or anything in the
/// bus's own unit (`dbus.service`, dbus-broker's `dbus-:…` units, the XDG
/// `app-dbus-…` scopes). That is how GNOME's search providers and background
/// services run: no window of their own. A launcher's scope
/// (`app-gnome-…`, `app-flatpak-…`) or a service of its own
/// (`gnome-terminal-server.service`) is someone opening an app.
pub fn bus_started(p: &ProcApp) -> bool {
    let unit = p.unit.as_str();
    unit == "dbus.service"
        || unit == "dbus-broker.service"
        || unit.starts_with("dbus-:")
        || unit.starts_with("app-dbus-")
        || (!unit.starts_with("app-") && p.argv.iter().any(|a| a == "--gapplication-service"))
}

/// An app seen under a person that the bus started on its own, until it has
/// stayed long enough to be one they use (or has gone, never counted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// Seconds it has been running so far, not counted yet.
    Holding(i64),
    /// Counts, tick by tick.
    Counting,
}

fn read_proc_app(pid: u32, status: &str) -> Option<ProcApp> {
    let uid = status
        .lines()
        .find(|l| l.starts_with("Uid:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|u| u.parse::<u32>().ok())?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let argv = std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| {
            b.split(|c| *c == 0)
                .filter(|a| !a.is_empty())
                .take(4)
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect()
        })
        .unwrap_or_default();
    // cgroup v2: one line, `0::/user.slice/…/app-gnome-foo-123.scope`.
    let unit = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .ok()
        .and_then(|c| {
            c.lines()
                .find_map(|l| l.strip_prefix("0::"))
                .map(|p| basename(p).to_string())
        })
        .unwrap_or_default();
    Some(ProcApp {
        uid,
        comm: comm.trim().to_string(),
        exe,
        argv,
        unit,
    })
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct SliceKey {
    /// "" = the whole device (site slices).
    user: String,
    /// RFC3339 of the UTC hour.
    hour: String,
    kind: &'static str,
    key: String,
}

pub struct Attrib {
    pending: HashMap<SliceKey, i64>,
    comm_index: HashMap<&'static str, &'static str>,
    /// The desktop apps, and when they were read (`None`: not yet).
    desktop: DesktopIndex,
    desktop_read: Option<Instant>,
    log_offset: u64,
    /// (user, app) seen at the last walk: counting, or a bus-started one
    /// still being held (see [`SERVICE_GRACE_SECS`]).
    seen: HashMap<(String, String), Seen>,
}

fn hour_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:00:00Z").to_string()
}

/// Registrable-domain approximation: the last two labels, or three when the
/// second-to-last is a common public second level (`co.uk`, `com.au`, …).
/// Wrong for exotic suffixes, right for the ones a family actually visits.
pub fn registrable(domain: &str) -> String {
    let d = domain.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = d.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    let second = labels[labels.len() - 2];
    let take = if matches!(second, "co" | "com" | "org" | "net" | "ac" | "gov" | "edu") {
        3
    } else {
        2
    };
    labels[labels.len().saturating_sub(take)..].join(".")
}

/// Pull the queried name out of one extra-format dnsmasq log line:
/// `... query[A] www.youtube.com from 127.0.0.1`.
fn queried_name(line: &str) -> Option<&str> {
    let idx = line.find(" query[")?;
    let rest = &line[idx..];
    let close = rest.find("] ")?;
    let after = &rest[close + 2..];
    let name = after.split_whitespace().next()?;
    (!name.is_empty()).then_some(name)
}

impl Attrib {
    pub fn new() -> Self {
        Attrib {
            pending: HashMap::new(),
            comm_index: openscreentime_policy::catalog::comm_to_app(),
            desktop: DesktopIndex::default(),
            desktop_read: None,
            log_offset: 0,
            seen: HashMap::new(),
        }
    }

    fn bump(&mut self, user: &str, kind: &'static str, key: String, amount: i64) {
        let k = SliceKey {
            user: user.to_string(),
            hour: hour_now(),
            kind,
            key,
        };
        if self.pending.len() >= MAX_PENDING && !self.pending.contains_key(&k) {
            return;
        }
        *self.pending.entry(k).or_insert(0) += amount;
    }

    /// One `/proc` walk: tick-seconds for every app — catalog or desktop —
    /// running under an active, unfrozen user. Each (user, app) counts once
    /// per tick no matter how many processes it has.
    pub fn sample_apps(&mut self, active_uids: &HashMap<String, u32>, tick_secs: i64) {
        if active_uids.is_empty() || tick_secs <= 0 {
            return;
        }
        if self.desktop_read.is_none_or(|t| t.elapsed() >= INDEX_TTL) {
            self.desktop = DesktopIndex::read();
            self.desktop_read = Some(Instant::now());
        }
        let wanted: HashSet<u32> = active_uids.values().copied().collect();
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return;
        };
        let procs: Vec<ProcApp> = dir
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
            .filter_map(|pid| {
                // The uid first: nobody else's processes are read further.
                let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
                let uid = status
                    .lines()
                    .find(|l| l.starts_with("Uid:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|u| u.parse::<u32>().ok())?;
                wanted
                    .contains(&uid)
                    .then(|| read_proc_app(pid, &status))
                    .flatten()
            })
            .collect();
        self.count_apps(&procs, active_uids, tick_secs);
    }

    /// The pure half of [`Attrib::sample_apps`]: which apps these processes
    /// are, counted once per (user, app). An app only the session bus started
    /// (a search provider, a background service — [`bus_started`]) counts
    /// once it has stayed [`SERVICE_GRACE_SECS`], from its start; one that
    /// leaves sooner never counts.
    fn count_apps(
        &mut self,
        procs: &[ProcApp],
        active_uids: &HashMap<String, u32>,
        tick_secs: i64,
    ) {
        let by_uid: HashMap<u32, &String> = active_uids.iter().map(|(u, id)| (*id, u)).collect();
        // (user, app) → whether any of its processes is someone opening it.
        let mut now: HashMap<(String, String), bool> = HashMap::new();
        for p in procs {
            let Some(user) = by_uid.get(&p.uid) else {
                continue;
            };
            let Some(app) = app_of(p, &self.comm_index, &self.desktop) else {
                continue;
            };
            *now.entry(((*user).clone(), app)).or_insert(false) |= !bus_started(p);
        }
        // Whoever was walked this time and no longer runs an app: it's gone
        // (a search provider that never counted is forgotten with it).
        let walked: HashSet<&String> = active_uids.keys().collect();
        self.seen
            .retain(|k, _| !walked.contains(&k.0) || now.contains_key(k));
        let mut keys: Vec<_> = now.into_iter().collect();
        keys.sort();
        for (key, opened) in keys {
            let amount = match self.seen.get(&key).copied() {
                Some(Seen::Counting) => tick_secs,
                held => {
                    let so_far = match held {
                        Some(Seen::Holding(s)) => s,
                        _ => 0,
                    } + tick_secs;
                    if opened || so_far > SERVICE_GRACE_SECS {
                        so_far
                    } else {
                        self.seen.insert(key, Seen::Holding(so_far));
                        continue;
                    }
                }
            };
            self.seen.insert(key.clone(), Seen::Counting);
            self.bump(&key.0, "app", key.1, amount);
        }
    }

    /// Tail the dnsmasq query log: one hit per query, keyed by registrable
    /// domain, attributed to the device. Handles rotation-by-truncation.
    pub fn ingest_dns_log(&mut self) {
        let Ok(mut f) = std::fs::File::open(DNSQ_LOG) else {
            return;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.log_offset {
            self.log_offset = 0; // someone rotated/truncated it
        }
        if len == self.log_offset {
            return;
        }
        if f.seek(SeekFrom::Start(self.log_offset)).is_err() {
            return;
        }
        let to_read = ((len - self.log_offset) as usize).min(READ_CAP);
        let mut buf = vec![0u8; to_read];
        let Ok(n) = f.read(&mut buf) else {
            return;
        };
        buf.truncate(n);
        // Only consume up to the last newline; a read that lands mid-line
        // (READ_CAP boundary, or the writer still appending) would otherwise
        // parse a truncated name like "www.you" into a bogus "site". The
        // remainder is re-read next tick because we advance the offset by the
        // consumed bytes, not by `n`.
        let last_nl = buf.iter().rposition(|&b| b == b'\n');
        let consume = match last_nl {
            Some(i) => i + 1,
            // No newline in a full READ_CAP read → a pathological single line;
            // consume it so we don't wedge, but don't parse the fragment.
            None if n == to_read && to_read == READ_CAP => n,
            None => return, // partial line still being written; wait for more
        };
        self.log_offset += consume as u64;
        let text = String::from_utf8_lossy(&buf[..consume]);
        for line in text.lines() {
            if let Some(name) = queried_name(line) {
                let site = registrable(name);
                if !site.is_empty() && site.contains('.') {
                    self.bump("", "site", site, 1);
                }
            }
        }
        // Keep the log from eating the disk; dnsmasq appends, so this is safe.
        if len > TRUNCATE_AT {
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(DNSQ_LOG);
            self.log_offset = 0;
        }
    }

    /// Take up to `cap` slices for posting. Returns an empty vec when idle.
    pub fn drain(&mut self, cap: usize) -> Vec<Value> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let keys: Vec<SliceKey> = self.pending.keys().take(cap).cloned().collect();
        keys.into_iter()
            .filter_map(|k| {
                let amount = self.pending.remove(&k)?;
                Some(json!({
                    "os_username": k.user,
                    "hour": k.hour,
                    "kind": k.kind,
                    "key": k.key,
                    "amount": amount,
                }))
            })
            .collect()
    }

    /// Put a failed batch back (bounded by MAX_PENDING like everything else).
    pub fn requeue(&mut self, slices: Vec<Value>) {
        for s in slices {
            let (Some(user), Some(hour), Some(kind), Some(key), Some(amount)) = (
                s.get("os_username").and_then(Value::as_str),
                s.get("hour").and_then(Value::as_str),
                s.get("kind").and_then(Value::as_str),
                s.get("key").and_then(Value::as_str),
                s.get("amount").and_then(Value::as_i64),
            ) else {
                continue;
            };
            let kind = if kind == "app" { "app" } else { "site" };
            let k = SliceKey {
                user: user.to_string(),
                hour: hour.to_string(),
                kind,
                key: key.to_string(),
            };
            if self.pending.len() < MAX_PENDING || self.pending.contains_key(&k) {
                *self.pending.entry(k).or_insert(0) += amount;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrable_takes_the_sensible_tail() {
        assert_eq!(registrable("www.youtube.com"), "youtube.com");
        assert_eq!(registrable("m.media-amazon.com"), "media-amazon.com");
        assert_eq!(registrable("news.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(registrable("youtube.com."), "youtube.com");
        assert_eq!(registrable("localhost"), "localhost");
    }

    #[test]
    fn dnsmasq_extra_lines_parse() {
        let line = "Aug 27 14:12:33 dnsmasq[123]: 4711 127.0.0.1/5353 query[A] www.youtube.com from 127.0.0.1";
        assert_eq!(queried_name(line), Some("www.youtube.com"));
        assert_eq!(
            queried_name("Aug 27 dnsmasq[1]: reply youtube.com is 1.2.3.4"),
            None
        );
    }

    fn desktop(name: &str, exec: &str, extra: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName={name}\nName[de]=Anders\nExec={exec}\n{extra}\n[Desktop Action new-window]\nName=New Window\nExec=other\n")
    }

    /// A stock Debian 12 GNOME computer, as far as its desktop entries go.
    fn debian_gnome() -> DesktopIndex {
        let entries = vec![
            ("firefox-esr".to_string(), desktop("Firefox ESR", "/usr/lib/firefox-esr/firefox-esr %u", "")),
            (
                "org.gnome.TextEditor".to_string(),
                desktop("Text Editor", "gnome-text-editor %U", "DBusActivatable=true"),
            ),
            (
                "org.gnome.Terminal".to_string(),
                desktop("Terminal", "gnome-terminal", "DBusActivatable=true"),
            ),
            (
                "org.gnome.Software".to_string(),
                desktop("Software", "gnome-software %U", "DBusActivatable=true"),
            ),
            ("org.gnome.Shell".to_string(), desktop("GNOME Shell", "/usr/bin/gnome-shell", "NoDisplay=true")),
            ("python3.11".to_string(), desktop("Python (v3.11)", "/usr/bin/python3.11", "Terminal=true")),
            ("steam".to_string(), desktop("Steam", "/usr/games/steam %U", "")),
            (
                "org.mozilla.Thunderbird".to_string(),
                desktop(
                    "Thunderbird",
                    "/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=thunderbird org.mozilla.Thunderbird",
                    "",
                ),
            ),
        ];
        let dbus: HashMap<String, String> = [
            (
                "org.gnome.TextEditor".to_string(),
                "[D-BUS Service]\nName=org.gnome.TextEditor\nExec=/usr/bin/gnome-text-editor --gapplication-service\n"
                    .to_string(),
            ),
            (
                "org.gnome.Terminal".to_string(),
                "[D-BUS Service]\nName=org.gnome.Terminal\nSystemdService=gnome-terminal-server.service\nExec=/usr/libexec/gnome-terminal-server\n"
                    .to_string(),
            ),
        ]
        .into();
        let autostart = vec![(
            "org.gnome.Software".to_string(),
            "[Desktop Entry]\nType=Application\nName=GNOME Software\nExec=/usr/bin/gnome-software --gapplication-service\nNoDisplay=true\n"
                .to_string(),
        )];
        DesktopIndex::build(&entries, &dbus, &autostart)
    }

    fn proc(uid: u32, comm: &str, exe: &str, argv: &[&str], unit: &str) -> ProcApp {
        ProcApp {
            uid,
            comm: comm.into(),
            exe: exe.into(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            unit: unit.into(),
        }
    }

    #[test]
    fn exec_lines_name_their_program() {
        assert_eq!(
            exec_programs("/usr/lib/firefox-esr/firefox-esr %u"),
            vec!["firefox-esr"]
        );
        assert_eq!(
            exec_programs("env BAMF_DESKTOP_FILE_HINT=/x.desktop /snap/bin/firefox %u"),
            vec!["firefox"]
        );
        assert_eq!(
            exec_programs("/usr/bin/flatpak run --branch=stable --command=thunderbird org.mozilla.Thunderbird"),
            vec!["thunderbird"]
        );
        assert!(exec_programs("sh -c \"foo\"").is_empty());
        assert!(exec_programs("/usr/bin/python3.11").is_empty());
        let e = parse_desktop(&desktop("Firefox ESR", "firefox-esr", "NoDisplay=false"));
        assert_eq!(
            e.name, "Firefox ESR",
            "the unlocalized name, not an action's"
        );
        assert_eq!(e.exec, "firefox-esr");
        assert!(!e.hidden && e.application);
    }

    #[test]
    fn units_name_their_desktop_app() {
        for (unit, id) in [
            (r"app-gnome-firefox\x2desr-2345.scope", Some("firefox-esr")),
            (
                "app-org.gnome.TextEditor-54920.scope",
                Some("org.gnome.TextEditor"),
            ),
            (
                "app-dbus-org.gnome.TextEditor-54920.scope",
                Some("org.gnome.TextEditor"),
            ),
            (
                "app-flatpak-org.mozilla.Thunderbird-812.scope",
                Some("org.mozilla.Thunderbird"),
            ),
            (
                "app-gnome-org.gnome.Software@autostart.service",
                Some("org.gnome.Software"),
            ),
            (
                "dbus-:1.2-org.gnome.Nautilus@0.service",
                Some("org.gnome.Nautilus"),
            ),
            ("session-3.scope", None),
            ("vte-spawn-7a1c.scope", None),
            ("gnome-terminal-server.service", None),
            ("dbus.service", None),
            ("init.scope", None),
        ] {
            assert_eq!(unit_app_id(unit).as_deref(), id, "{unit}");
        }
    }

    /// Acceptance round 3: Mia used Firefox and Text Editor for 37 minutes and
    /// "Where the time went" said "Nothing yet today" — only the blocking
    /// catalog could name an app. Now what her desktop runs counts, and the
    /// session around it doesn't.
    #[test]
    fn firefox_and_text_editor_count_the_session_does_not() {
        let mut a = Attrib::new();
        a.desktop = debian_gnome();
        let mia = 1000;
        let philip = 1001;
        let procs = vec![
            // Firefox from the dash, and one of its content processes.
            proc(
                mia,
                "firefox-esr",
                "/usr/lib/firefox-esr/firefox-esr",
                &["/usr/lib/firefox-esr/firefox-esr"],
                r"app-gnome-firefox\x2desr-2345.scope",
            ),
            proc(
                mia,
                "Isolated Web Co",
                "/usr/lib/firefox-esr/firefox-esr",
                &["/usr/lib/firefox-esr/firefox-esr", "-contentproc"],
                r"app-gnome-firefox\x2desr-2345.scope",
            ),
            // Text Editor, started by D-Bus inside the session bus (Debian 12).
            proc(
                mia,
                "gnome-text-edit",
                "/usr/bin/gnome-text-editor",
                &["/usr/bin/gnome-text-editor", "--gapplication-service"],
                "dbus.service",
            ),
            // The session: the shell, a settings daemon, Software's autostarted
            // background service, the sound server, a python helper.
            proc(
                mia,
                "gnome-shell",
                "/usr/bin/gnome-shell",
                &["/usr/bin/gnome-shell"],
                "org.gnome.Shell@wayland.service",
            ),
            proc(
                mia,
                "gsd-media-keys",
                "/usr/libexec/gsd-media-keys",
                &["/usr/libexec/gsd-media-keys"],
                "org.gnome.SettingsDaemon.MediaKeys.service",
            ),
            proc(
                mia,
                "gnome-software",
                "/usr/bin/gnome-software",
                &["/usr/bin/gnome-software", "--gapplication-service"],
                "app-gnome-org.gnome.Software-1612.scope",
            ),
            proc(
                mia,
                "pipewire",
                "/usr/bin/pipewire",
                &["/usr/bin/pipewire"],
                "pipewire.service",
            ),
            proc(
                mia,
                "python3",
                "/usr/bin/python3.11",
                &["/usr/bin/python3", "/usr/libexec/ibus-setup-helper"],
                "app.slice",
            ),
            // Steam: a catalog app keeps its catalog id.
            proc(
                mia,
                "steam",
                "/home/mia/.steam/ubuntu12_32/steam",
                &["steam"],
                "app-gnome-steam-777.scope",
            ),
            // Philip isn't at the seat: his Terminal counts for nobody.
            proc(
                philip,
                "gnome-terminal-",
                "/usr/libexec/gnome-terminal-server",
                &["/usr/libexec/gnome-terminal-server"],
                "gnome-terminal-server.service",
            ),
        ];
        let users: HashMap<String, u32> = [("mia".to_string(), mia)].into();
        // Seventy seconds of it. Text Editor was started by the bus (as
        // GNOME's search providers are): it counts once it has stayed a
        // minute — and then from its start.
        for _ in 0..7 {
            a.count_apps(&procs, &users, 10);
        }
        let got = totals(&mut a);
        assert!(got.keys().all(|(u, _)| u == "mia"));
        let got: Vec<(String, i64)> = got.into_iter().map(|((_, k), v)| (k, v)).collect();
        assert_eq!(
            got,
            vec![
                ("Firefox ESR".into(), 70),
                ("Text Editor".into(), 70),
                ("steam".into(), 70)
            ]
        );

        // Philip at the seat: his Terminal (a D-Bus app with its own service).
        let users: HashMap<String, u32> = [("philip".to_string(), philip)].into();
        a.count_apps(&procs, &users, 10);
        let got: Vec<String> = a
            .drain(100)
            .iter()
            .map(|s| s["key"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(got, vec!["Terminal"]);
    }

    /// Drain everything into (user, app) → seconds (slices of one hour).
    fn totals(a: &mut Attrib) -> std::collections::BTreeMap<(String, String), i64> {
        let mut out = std::collections::BTreeMap::new();
        for s in a.drain(1000) {
            assert_eq!(s["kind"], "app");
            let k = (
                s["os_username"].as_str().unwrap().to_string(),
                s["key"].as_str().unwrap().to_string(),
            );
            *out.entry(k).or_insert(0) += s["amount"].as_i64().unwrap();
        }
        out
    }

    /// Acceptance round 4: "Where the time went" showed Files, Characters and
    /// Disks (21–31 s each) for Mia and Philip, who never opened them — GNOME
    /// starts their search providers when you type in the overview. On the
    /// Debian 12 fixture: what the bus starts and lets go within a minute
    /// never counts; the app she typed her way to does; an app opened from
    /// the dash counts at once; a search provider she then opens a window
    /// of (the same process, staying) counts from its start.
    #[test]
    fn search_providers_that_come_and_go_are_not_apps_she_used() {
        let mut a = Attrib::new();
        let mut idx = debian_gnome();
        let more = DesktopIndex::build(
            &[
                (
                    "org.gnome.Nautilus".to_string(),
                    desktop("Files", "nautilus --new-window %U", "DBusActivatable=true"),
                ),
                (
                    "org.gnome.Characters".to_string(),
                    desktop("Characters", "gnome-characters", "DBusActivatable=true"),
                ),
                (
                    "org.gnome.DiskUtility".to_string(),
                    desktop("Disks", "gnome-disks", "DBusActivatable=true"),
                ),
            ],
            &[
                (
                    "org.gnome.Nautilus".to_string(),
                    "[D-BUS Service]\nName=org.gnome.Nautilus\nExec=/usr/bin/nautilus --gapplication-service\n".to_string(),
                ),
                (
                    "org.gnome.DiskUtility".to_string(),
                    "[D-BUS Service]\nName=org.gnome.DiskUtility\nExec=/usr/bin/gnome-disks --gapplication-service\n".to_string(),
                ),
            ]
            .into(),
            &[],
        );
        idx.by_program.extend(more.by_program);
        idx.by_id.extend(more.by_id);
        a.desktop = idx;
        let mia = 1000;
        let users: HashMap<String, u32> = [("mia".to_string(), mia)].into();
        let files = proc(
            mia,
            "nautilus",
            "/usr/bin/nautilus",
            &["/usr/bin/nautilus", "--gapplication-service"],
            "dbus.service",
        );
        let characters = proc(
            mia,
            "gjs",
            "/usr/bin/gjs-console",
            &[
                "/usr/bin/gjs",
                "/usr/bin/gnome-characters",
                "--gapplication-service",
            ],
            "dbus.service",
        );
        let disks = proc(
            mia,
            "gnome-disks",
            "/usr/bin/gnome-disks",
            &["/usr/bin/gnome-disks", "--gapplication-service"],
            "dbus.service",
        );
        let editor = proc(
            mia,
            "gnome-text-edit",
            "/usr/bin/gnome-text-editor",
            &["/usr/bin/gnome-text-editor", "--gapplication-service"],
            "dbus.service",
        );
        for p in [&files, &characters, &disks] {
            assert!(
                app_of(p, &a.comm_index, &a.desktop).is_some(),
                "the fixture names them: {p:?}"
            );
            assert!(bus_started(p));
        }
        // She types "text" in the overview: every search provider wakes, she
        // picks Text Editor. 30 s later the providers have left.
        let typing = vec![
            files.clone(),
            characters.clone(),
            disks.clone(),
            editor.clone(),
        ];
        for _ in 0..3 {
            a.count_apps(&typing, &users, 10);
        }
        assert!(totals(&mut a).is_empty(), "nothing yet: a minute first");
        for _ in 0..9 {
            a.count_apps(std::slice::from_ref(&editor), &users, 10);
        }
        let got = totals(&mut a);
        assert_eq!(
            got.into_iter()
                .map(|((_, k), v)| (k, v))
                .collect::<Vec<_>>(),
            vec![("Text Editor".to_string(), 120)],
            "the providers never count; the editor from its start"
        );

        // Files opened from the dash: a launcher's scope, counted at once.
        let opened = proc(
            mia,
            "nautilus",
            "/usr/bin/nautilus",
            &["/usr/bin/nautilus", "--new-window"],
            "app-gnome-org.gnome.Nautilus-4242.scope",
        );
        assert!(!bus_started(&opened));
        a.count_apps(&[opened], &users, 10);
        assert_eq!(
            totals(&mut a).get(&("mia".into(), "Files".into())),
            Some(&10)
        );

        // The provider that became a window (she clicked a result): it stays,
        // and counts from when it started.
        let mut a2 = Attrib::new();
        a2.desktop = std::mem::take(&mut a.desktop);
        for _ in 0..8 {
            a2.count_apps(std::slice::from_ref(&files), &users, 10);
        }
        assert_eq!(
            totals(&mut a2).get(&("mia".into(), "Files".into())),
            Some(&80)
        );
    }

    /// The real `/proc` walk, on this test's own processes: a child running a
    /// program a desktop entry names is that app; the test runner is nothing.
    #[test]
    fn the_proc_walk_finds_a_running_app() {
        let Ok(mut child) = std::process::Command::new("sleep").arg("30").spawn() else {
            return; // no `sleep` here: nothing to walk
        };
        let mut a = Attrib::new();
        a.desktop = DesktopIndex::build(
            &[("sleepy".to_string(), desktop("Sleepy", "sleep %u", ""))],
            &HashMap::new(),
            &[],
        );
        a.desktop_read = Some(Instant::now());
        let me = unsafe { libc::getuid() };
        a.sample_apps(&[("me".to_string(), me)].into(), 10);
        let _ = child.kill();
        let _ = child.wait();
        let got: Vec<(String, i64)> = a
            .drain(100)
            .iter()
            .map(|s| {
                (
                    s["key"].as_str().unwrap().to_string(),
                    s["amount"].as_i64().unwrap(),
                )
            })
            .collect();
        assert!(got.contains(&("Sleepy".to_string(), 10)), "{got:?}");
        // Whatever else this machine's user runs is a catalog app (a
        // developer's Steam), never the test runner, a shell or the session.
        let catalog: HashSet<&str> = a.comm_index.values().copied().collect();
        for (key, _) in &got {
            assert!(key == "Sleepy" || catalog.contains(key.as_str()), "{key}");
        }
    }

    #[test]
    fn drain_then_requeue_round_trips() {
        let mut a = Attrib::new();
        a.bump("mia", "app", "discord".into(), 10);
        a.bump("", "site", "youtube.com".into(), 3);
        let batch = a.drain(500);
        assert_eq!(batch.len(), 2);
        assert!(a.drain(500).is_empty());
        a.requeue(batch);
        assert_eq!(a.drain(500).len(), 2);
    }
}
