//! What a stop freezes: the person's apps, not their session.
//!
//! The lock lives on its own VT, so the person's compositor doesn't have to
//! be frozen to keep their hands off it: a session on another VT gets no
//! input and draws nothing anyone sees. The freeze only has to stop what
//! their *apps* are doing — sound, video, a game, a download. Freezing the
//! session itself broke three things: a session frozen while it was still
//! starting came back without its keyboard and mouse (or never registered
//! with GDM at all), and GDM, asked to take someone back to their stopped
//! session, re-authenticates through processes inside it (the session's
//! worker, the keyring) and hung until it gave up.
//!
//! So a stop freezes, under the person's `user-<uid>.slice`:
//!
//! * every slice and unit their user manager runs (`user@<uid>.service`)
//!   except `session.slice` (compositor, session manager, settings daemons,
//!   portals, the session bus, the sound server) and the manager itself
//!   (`init.scope`) — `app.slice`, `background.slice`, and whatever a
//!   `systemd-run --user --slice=…` put anywhere else;
//! * never `app.slice` as one piece: GNOME 43 (Debian 12) starts its session
//!   manager, the keyring and the accessibility bus there, so its units are
//!   frozen one by one, skipping that session plumbing ([`is_plumbing`]); a
//!   slice below it with no plumbing in it (a terminal's) is frozen whole;
//! * apps D-Bus started inside `session.slice` (on a session bus without
//!   dbus-broker, Videos, Files and Text Editor run as children of the bus
//!   itself): they are first filed into an app scope of their own — the same
//!   call GNOME Shell makes for every app it launches — and frozen there;
//! * their text-console and SSH logins (their `session-N.scope`), whole;
//! * never their graphical login's `session-N.scope` (GDM's worker, the
//!   session launcher, Xorg).
//!
//! **The fallback** is the old whole-`user-<uid>.slice` freeze, for a desktop
//! that doesn't live under the user manager's standard slices: no
//! `session.slice` at all, or a graphical login scope that holds more than
//! the launchers of a systemd-managed session (a legacy X session, where the
//! window manager and every app run inside `session-N.scope`).
//!
//! **A session still starting is left alone**: until a graphical login has
//! been up [`SETTLE`], nothing of the desktop is frozen (the lock is on screen
//! already). A desktop frozen halfway through starting is what came back
//! without input.
//!
//! Everything here is read from the cgroup tree ([`read_tree`]) and decided by
//! [`plan`], a pure function the tests drive with fixture trees.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A graphical login must have been up this long before anything of its
/// desktop is frozen — and before a legacy desktop is frozen whole.
pub const SETTLE: Duration = Duration::from_secs(60);

const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Bounds on reading one person's tree (it is theirs to grow).
const MAX_DEPTH: usize = 10;
const MAX_CGROUPS: usize = 4096;
const MAX_PROCS: usize = 4096;

/// A process, as far as the plan cares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// `/proc/<pid>/comm` (at most 15 bytes).
    pub comm: String,
    pub argv: Vec<String>,
}

/// One cgroup of the person's tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cgroup {
    /// The directory name (`app.slice`, `session-3.scope`, `user@1000.service`).
    pub name: String,
    /// Processes directly in it.
    pub procs: Vec<Proc>,
    pub children: Vec<Cgroup>,
    /// Its own `cgroup.freeze` reads 1.
    pub frozen: bool,
}

impl Cgroup {
    pub fn child(&self, name: &str) -> Option<&Cgroup> {
        self.children.iter().find(|c| c.name == name)
    }

    /// The cgroup at `path` (relative, `/`-separated).
    pub fn at(&self, path: &str) -> Option<&Cgroup> {
        path.split('/')
            .filter(|p| !p.is_empty())
            .try_fold(self, |c, part| c.child(part))
    }

    /// Frozen by its own `cgroup.freeze` or an ancestor's (from `self` down).
    fn frozen_at(&self, path: &str) -> Option<bool> {
        let mut c = self;
        let mut frozen = c.frozen;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            c = c.child(part)?;
            frozen |= c.frozen;
        }
        Some(frozen)
    }

    fn any_frozen(&self) -> bool {
        self.frozen || self.children.iter().any(Cgroup::any_frozen)
    }

    fn all_procs(&self) -> Box<dyn Iterator<Item = &Proc> + '_> {
        Box::new(
            self.procs
                .iter()
                .chain(self.children.iter().flat_map(Cgroup::all_procs)),
        )
    }
}

/// A login session of the person, from logind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    /// logind's session id: its scope is `session-<id>.scope`.
    pub id: String,
    pub graphical: bool,
    /// How long it has been up (`None`: logind didn't say).
    pub age: Option<Duration>,
}

/// A desktop application D-Bus starts with this command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppExec {
    pub id: String,
    pub argv: Vec<String>,
}

/// An app running inside `session.slice`, to be filed into a scope of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stray {
    pub app: String,
    /// The app's process first, then everything it started that is still
    /// beside it.
    pub pids: Vec<u32>,
}

/// What a stop freezes for one person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Their whole `user-<uid>.slice`: a desktop that lives inside its login
    /// session (see the module docs).
    Whole,
    /// These cgroups (relative to `user-<uid>.slice`), after filing these
    /// strays into app scopes.
    Apps {
        targets: Vec<String>,
        strays: Vec<Stray>,
    },
}

/// Units that belong to the session, wherever a distribution put them —
/// matched on the unit name, `\xNN` unescaped. Freezing these is what broke
/// logging in, and taking someone back from the login screen.
const PLUMBING: &[&str] = &[
    // Session managers and their watchdogs (GNOME 43 runs its own in app.slice).
    "gnome-session",
    "ksmserver",
    // What a login screen re-authenticates through, and the agents beside it.
    "gnome-keyring",
    "kwallet",
    "gcr-ssh-agent",
    "ssh-agent",
    "gpg-agent",
    "polkit",
    "policykit",
    // Accessibility, portals, settings.
    "at-spi",
    "xdg-desktop-portal",
    "xdg-document-portal",
    "xdg-permission-store",
    "dconf",
    "gvfs",
    // Sound servers, input methods.
    "pipewire",
    "wireplumber",
    "pulseaudio",
    "ibus",
    "fcitx",
    // The shell and its helpers, where a distribution keeps them outside
    // session.slice.
    "org.gnome.shell",
    "gnome-shell",
    "org.gnome.settingsdaemon",
    "kwin",
    "kded",
    "kglobalaccel",
    // The data servers behind the shell's calendar.
    "evolution-source-registry",
    "evolution-calendar-factory",
    "evolution-addressbook-factory",
];

/// Processes a systemd-managed graphical login keeps in its own
/// `session-N.scope` — the display manager's worker and the session launcher.
/// Anything else in there means the desktop itself lives in the scope.
/// (Names as `/proc/<pid>/comm` has them: 15 bytes at most.)
const LAUNCHERS: &[&str] = &[
    "gdm-session-wor",
    "gdm-wayland-ses",
    "gdm-x-session",
    "gnome-session",
    "gnome-session-b",
    "sddm-helper",
    "sddm-helper-sta",
    "startplasma-way",
    "startplasma-x11",
    "lightdm",
    "Xorg",
    "Xwayland",
    "X",
    "dbus-run-sessio",
    "dbus-daemon",
    "dbus-broker",
    "dbus-broker-lau",
    "ssh-agent",
    "gpg-agent",
    "im-launch",
    "systemctl",
];

/// `app-gnome\x2dsession\x2dmanager.slice` → `app-gnome-session-manager.slice`.
pub fn unescape(name: &str) -> String {
    let b = name.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' {
            if let Ok(v) = u8::from_str_radix(&name[i + 2..i + 4], 16) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Session plumbing, by unit name (see [`PLUMBING`]); the session bus itself
/// by its exact name.
pub fn is_plumbing(unit: &str) -> bool {
    let name = unescape(unit).to_ascii_lowercase();
    let stem = name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(name.as_str());
    stem == "dbus" || stem == "dbus-broker" || PLUMBING.iter().any(|p| name.contains(p))
}

fn holds_plumbing(c: &Cgroup) -> bool {
    c.children
        .iter()
        .any(|k| is_plumbing(&k.name) || holds_plumbing(k))
}

/// A graphical login's scope with nothing in it but launchers.
fn launchers_only(scope: &Cgroup) -> bool {
    scope
        .all_procs()
        .all(|p| LAUNCHERS.contains(&p.comm.as_str()))
}

/// `session-3.scope` → `3`.
fn session_id(scope: &str) -> Option<&str> {
    scope.strip_prefix("session-")?.strip_suffix(".scope")
}

/// Decide what a stop freezes for the person whose `user-<uid>.slice` this is.
/// `logins`: their logind sessions; `apps`: [`dbus_apps`].
pub fn plan(slice: &Cgroup, uid: u32, logins: &[Login], apps: &[AppExec]) -> Plan {
    let settled = |l: &Login| l.age.is_none_or(|a| a >= SETTLE);
    let mut targets = Vec::new();
    let mut starting = false;
    let mut legacy = false;
    for c in &slice.children {
        let Some(id) = session_id(&c.name) else {
            continue;
        };
        match logins.iter().find(|l| l.id == id) {
            // logind no longer lists it: on its way out. Leave it be.
            None => {}
            Some(l) if l.graphical => {
                if !settled(l) {
                    starting = true;
                } else if !launchers_only(c) {
                    legacy = true;
                }
            }
            // A text console or an SSH login: all of it is theirs.
            Some(_) => targets.push(c.name.clone()),
        }
    }
    let manager = slice.child(&format!("user@{uid}.service"));
    let session_slice = manager.and_then(|m| m.child("session.slice"));
    if starting {
        // Nothing of a desktop that is still starting is frozen.
        return Plan::Apps {
            targets,
            strays: Vec::new(),
        };
    }
    let (Some(m), Some(session_slice), false) = (manager, session_slice, legacy) else {
        return Plan::Whole;
    };
    for c in &m.children {
        if c.name == "init.scope" || c.name == "session.slice" {
            continue;
        }
        collect(c, &format!("{}/{}", m.name, c.name), &mut targets);
    }
    Plan::Apps {
        targets,
        strays: strays(session_slice, apps),
    }
}

/// Freeze `c` whole, or — when session plumbing lives below it, and always
/// for `app.slice` — each of its children that isn't plumbing.
fn collect(c: &Cgroup, path: &str, out: &mut Vec<String>) {
    if is_plumbing(&c.name) {
        return;
    }
    if c.name != "app.slice" && !holds_plumbing(c) {
        out.push(path.to_string());
        return;
    }
    for k in &c.children {
        collect(k, &format!("{path}/{}", k.name), out);
    }
}

/// `p` runs `app`'s D-Bus command line — directly, or as a script through
/// its interpreter (`/usr/bin/gjs /usr/bin/foo …` for `Exec=/usr/bin/foo …`).
fn runs(p: &Proc, app: &AppExec) -> bool {
    let starts = |argv: &[String]| !app.argv.is_empty() && argv.starts_with(&app.argv);
    starts(&p.argv) || p.argv.get(1..).is_some_and(starts)
}

/// Apps D-Bus started inside `session.slice`: in each of its units, a process
/// that runs a desktop application's D-Bus command line — never the unit's own
/// main process — with everything it started that is still beside it.
fn strays(session_slice: &Cgroup, apps: &[AppExec]) -> Vec<Stray> {
    let mut out = Vec::new();
    let mut stack: Vec<&Cgroup> = vec![session_slice];
    while let Some(c) = stack.pop() {
        stack.extend(c.children.iter());
        let pids: std::collections::HashSet<u32> = c.procs.iter().map(|p| p.pid).collect();
        let mut taken: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for p in &c.procs {
            // The unit's own process(es): their parent lives elsewhere.
            if !pids.contains(&p.ppid) || taken.contains(&p.pid) {
                continue;
            }
            let Some(app) = apps.iter().find(|a| runs(p, a)) else {
                continue;
            };
            let mut family = vec![p.pid];
            let mut i = 0;
            while i < family.len() {
                let parent = family[i];
                family.extend(
                    c.procs
                        .iter()
                        .filter(|k| k.ppid == parent && !family.contains(&k.pid))
                        .map(|k| k.pid)
                        .collect::<Vec<_>>(),
                );
                i += 1;
            }
            taken.extend(family.iter().copied());
            out.push(Stray {
                app: app.id.clone(),
                pids: family,
            });
        }
    }
    out
}

/// Is the stop holding — is everything the plan freezes frozen right now?
/// (Nothing to freeze holds, too: a session still starting.)
pub fn holds(slice: &Cgroup, plan: &Plan) -> bool {
    match plan {
        Plan::Whole => slice.frozen,
        Plan::Apps { targets, strays } => {
            slice.frozen
                || (strays.is_empty() && targets.iter().all(|t| slice.frozen_at(t).unwrap_or(true)))
        }
    }
}

/// Anything of theirs frozen by a `cgroup.freeze` (ours, most likely).
pub fn any_frozen(slice: &Cgroup) -> bool {
    slice.any_frozen()
}

/// The path of every cgroup in the tree whose own `cgroup.freeze` reads 1.
pub fn frozen_paths(slice: &Cgroup) -> Vec<String> {
    fn walk(c: &Cgroup, path: &str, out: &mut Vec<String>) {
        if c.frozen {
            out.push(path.to_string());
        }
        for k in &c.children {
            let p = if path.is_empty() {
                k.name.clone()
            } else {
                format!("{path}/{}", k.name)
            };
            walk(k, &p, out);
        }
    }
    let mut out = Vec::new();
    walk(slice, "", &mut out);
    out
}

// ── Reading the machine ─────────────────────────────────────────────────────

/// `/sys/fs/cgroup/user.slice/user-<uid>.slice`.
pub fn slice_dir(uid: u32) -> PathBuf {
    Path::new(CGROUP_ROOT).join(format!("user.slice/user-{uid}.slice"))
}

/// Read a person's cgroup tree, their processes included. `None` when they
/// have no slice (not logged in).
pub fn read_tree(uid: u32) -> Option<Cgroup> {
    let dir = slice_dir(uid);
    if !dir.is_dir() {
        return None;
    }
    let mut budget = (MAX_CGROUPS, MAX_PROCS);
    Some(read_cgroup(&dir, 0, &mut budget))
}

fn read_cgroup(dir: &Path, depth: usize, budget: &mut (usize, usize)) -> Cgroup {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let frozen = std::fs::read_to_string(dir.join("cgroup.freeze")).is_ok_and(|s| s.trim() == "1");
    let mut procs = Vec::new();
    if let Ok(list) = std::fs::read_to_string(dir.join("cgroup.procs")) {
        for pid in list.lines().filter_map(|l| l.trim().parse::<u32>().ok()) {
            if budget.1 == 0 {
                break;
            }
            budget.1 -= 1;
            if let Some(p) = read_proc(pid) {
                procs.push(p);
            }
        }
    }
    let mut children = Vec::new();
    if depth < MAX_DEPTH {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut dirs: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect();
            dirs.sort();
            for d in dirs {
                if budget.0 == 0 {
                    break;
                }
                budget.0 -= 1;
                children.push(read_cgroup(&d, depth + 1, budget));
            }
        }
    }
    Cgroup {
        name,
        procs,
        children,
        frozen,
    }
}

fn read_proc(pid: u32) -> Option<Proc> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state ppid …` — comm may hold spaces and parentheses.
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat.get(open + 1..close)?.to_string();
    let ppid = stat
        .get(close + 1..)?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    let argv = std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| {
            b.split(|c| *c == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect()
        })
        .unwrap_or_default();
    Some(Proc {
        pid,
        ppid,
        comm,
        argv,
    })
}

/// Where desktop files and D-Bus service files are installed.
const DATA_DIRS: &[&str] = &["/usr/local/share", "/usr/share"];

/// Desktop applications D-Bus can start (`DBusActivatable=true`), with the
/// command line their D-Bus service file runs.
pub fn dbus_apps() -> Vec<AppExec> {
    let mut out: Vec<AppExec> = Vec::new();
    for base in DATA_DIRS {
        let Ok(entries) = std::fs::read_dir(Path::new(base).join("applications")) else {
            continue;
        };
        for e in entries.filter_map(|e| e.ok()) {
            let path = e.path();
            let Some(id) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".desktop"))
            else {
                continue;
            };
            if out.iter().any(|a| a.id == id) {
                continue;
            }
            let Ok(desktop) = std::fs::read_to_string(&path) else {
                continue;
            };
            if !dbus_activatable(&desktop) {
                continue;
            }
            let service = DATA_DIRS.iter().find_map(|b| {
                std::fs::read_to_string(Path::new(b).join(format!("dbus-1/services/{id}.service")))
                    .ok()
            });
            if let Some(argv) = service.as_deref().and_then(service_exec) {
                out.push(AppExec {
                    id: id.to_string(),
                    argv,
                });
            }
        }
    }
    out
}

/// A desktop file's `[Desktop Entry]` says `DBusActivatable=true`.
pub fn dbus_activatable(desktop: &str) -> bool {
    let mut in_entry = false;
    for line in desktop.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if in_entry {
            if let Some(v) = line.strip_prefix("DBusActivatable=") {
                return v.trim() == "true";
            }
        }
    }
    false
}

/// A D-Bus service file's `Exec=`, split the way dbus-daemon splits it
/// (whitespace, with simple quoting). `None` without one, or when it hands
/// the start to systemd (`SystemdService=`: then it lands in a unit of its own).
pub fn service_exec(service: &str) -> Option<Vec<String>> {
    if service
        .lines()
        .any(|l| l.trim().starts_with("SystemdService="))
    {
        return None;
    }
    let exec = service
        .lines()
        .find_map(|l| l.trim().strip_prefix("Exec="))?;
    let mut argv = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for ch in exec.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(ch);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    argv.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        argv.push(cur);
    }
    (!argv.is_empty()).then_some(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    /// A fixture tree from lines of `path: proc proc …` (colon and space:
    /// unit names may hold a colon), paths relative to
    /// the user slice. A proc is `comm`, or `pid<ppid=argv…` with `+` for
    /// spaces in argv (`614<1=/usr/bin/dbus-daemon`). A path ending in `*`
    /// is frozen.
    fn tree(lines: &[&str]) -> Cgroup {
        let mut root = Cgroup {
            name: "user-1000.slice".into(),
            ..Default::default()
        };
        let mut next_pid = 5000;
        for line in lines {
            let (path, procs) = line.split_once(": ").unwrap_or((line, ""));
            let path = path.trim();
            let (path, frozen) = match path.strip_suffix('*') {
                Some(p) => (p, true),
                None => (path, false),
            };
            let mut c = &mut root;
            if path.is_empty() {
                c.frozen |= frozen;
            }
            for part in path.split('/').filter(|p| !p.is_empty()) {
                let i = match c.children.iter().position(|k| k.name == part) {
                    Some(i) => i,
                    None => {
                        c.children.push(Cgroup {
                            name: part.into(),
                            ..Default::default()
                        });
                        c.children.len() - 1
                    }
                };
                c = &mut c.children[i];
            }
            if !path.is_empty() {
                c.frozen |= frozen;
            }
            for p in procs.split_whitespace() {
                c.procs.push(match p.split_once('=') {
                    Some((ids, argv)) => {
                        let (pid, ppid) = ids.split_once('<').unwrap();
                        let argv: Vec<String> = argv.split('+').map(str::to_string).collect();
                        let comm: String = Path::new(&argv[0])
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .chars()
                            .take(15)
                            .collect();
                        Proc {
                            pid: pid.parse().unwrap(),
                            ppid: ppid.parse().unwrap(),
                            comm,
                            argv,
                        }
                    }
                    None => {
                        next_pid += 1;
                        Proc {
                            pid: next_pid,
                            ppid: 1,
                            comm: p.into(),
                            argv: vec![p.into()],
                        }
                    }
                });
            }
        }
        root
    }

    fn login(id: &str, graphical: bool, age: Duration) -> Login {
        Login {
            id: id.into(),
            graphical,
            age: Some(age),
        }
    }

    fn totem() -> Vec<AppExec> {
        vec![
            AppExec {
                id: "org.gnome.Totem".into(),
                argv: vec!["/usr/bin/totem".into(), "--gapplication-service".into()],
            },
            AppExec {
                id: "org.gnome.Weather".into(),
                argv: vec![
                    "/usr/bin/gnome-weather".into(),
                    "--gapplication-service".into(),
                ],
            },
        ]
    }

    /// Debian 12's GNOME 43, as `systemd-cgls` showed it on a real login —
    /// with Videos started from the overview (a child of the session bus).
    fn debian12() -> Cgroup {
        tree(&[
            "session-1.scope: gdm-session-wor gdm-wayland-ses gnome-session-b",
            "user@1000.service/init.scope: systemd (sd-pam)",
            "user@1000.service/session.slice/org.gnome.Shell@wayland.service: gnome-shell",
            "user@1000.service/session.slice/pipewire.service: pipewire",
            "user@1000.service/session.slice/org.gnome.SettingsDaemon.Power.service: gsd-power",
            "user@1000.service/session.slice/dbus.service: 614<590=/usr/bin/dbus-daemon+--session 842<614=/usr/bin/gjs+/usr/share/gnome-shell/org.gnome.Shell.Notifications 4069<614=/usr/bin/totem+--gapplication-service 4100<4069=/usr/lib/totem/helper",
            "user@1000.service/background.slice/tracker-miner-fs-3.service: tracker-miner-f",
            "user@1000.service/app.slice/gnome-keyring-daemon.service: gnome-keyring-d",
            "user@1000.service/app.slice/app-gnome\\x2dsession\\x2dmanager.slice/gnome-session-manager@gnome.service: gnome-session-b dbus-daemon at-spi2-registr",
            "user@1000.service/app.slice/gnome-session-monitor.service: gnome-session-c",
            "user@1000.service/app.slice/app-gnome-at\\x2dspi\\x2ddbus\\x2dbus-717.scope: at-spi-bus-laun",
            "user@1000.service/app.slice/xdg-desktop-portal-gnome.service: xdg-desktop-por",
            "user@1000.service/app.slice/evolution-source-registry.service: evolution-sourc",
            "user@1000.service/app.slice/gcr-ssh-agent.service: gcr-ssh-agent",
            "user@1000.service/app.slice/ssh-agent.service: ssh-agent",
            "user@1000.service/app.slice/app-gnome-org.gnome.SettingsDaemon.DiskUtilityNotify-887.scope: gsd-disk-utilit",
            "user@1000.service/app.slice/app-gnome-org.gnome.Software-922.scope: gnome-software",
            "user@1000.service/app.slice/app-gnome-firefox\\x2desr-2001.scope: firefox-esr",
            "user@1000.service/app.slice/app-org.gnome.Terminal.slice/gnome-terminal-server.service: gnome-terminal-",
            "user@1000.service/app.slice/app-org.gnome.Terminal.slice/vte-spawn-1f2e.scope: bash speaker-test",
        ])
    }

    #[test]
    fn a_gnome_43_desktop_freezes_its_apps_and_keeps_its_session() {
        let t = debian12();
        let p = plan(&t, 1000, &[login("1", true, 5 * MIN)], &totem());
        let Plan::Apps { targets, strays } = &p else {
            panic!("expected apps, got {p:?}");
        };
        let m = "user@1000.service";
        assert_eq!(
            targets,
            &vec![
                format!("{m}/background.slice"),
                format!("{m}/app.slice/app-gnome-org.gnome.Software-922.scope"),
                format!("{m}/app.slice/app-gnome-firefox\\x2desr-2001.scope"),
                format!("{m}/app.slice/app-org.gnome.Terminal.slice"),
            ]
        );
        // Videos, started by the session bus, is filed into a scope of its
        // own with its helper — never the bus, never the shell's own gjs.
        assert_eq!(
            strays,
            &vec![Stray {
                app: "org.gnome.Totem".into(),
                pids: vec![4069, 4100],
            }]
        );
        // Never the session, the keyring, the session manager or its login scope.
        for t in targets {
            assert!(!t.contains("session.slice") && !t.contains("keyring"));
            assert!(!t.contains("session-1.scope") && !t.contains("session\\x2dmanager"));
        }
    }

    #[test]
    fn a_session_still_starting_is_left_alone() {
        let t = debian12();
        let p = plan(
            &t,
            1000,
            &[login("1", true, Duration::from_secs(3))],
            &totem(),
        );
        assert_eq!(
            p,
            Plan::Apps {
                targets: vec![],
                strays: vec![]
            }
        );
        // …but their SSH login is theirs, starting desktop or not.
        let t = tree(&[
            "session-1.scope: gdm-session-wor gdm-wayland-ses gnome-session-b",
            "session-7.scope: sshd sshd bash wget",
            "user@1000.service/session.slice/org.gnome.Shell@wayland.service: gnome-shell",
        ]);
        let p = plan(
            &t,
            1000,
            &[
                login("1", true, Duration::from_secs(3)),
                login("7", false, 30 * MIN),
            ],
            &[],
        );
        assert_eq!(
            p,
            Plan::Apps {
                targets: vec!["session-7.scope".into()],
                strays: vec![]
            }
        );
    }

    #[test]
    fn a_legacy_x_session_falls_back_to_the_whole_slice() {
        // Xfce under LightDM: the desktop and every app live in the login
        // scope; the user manager only runs the sound server and the bus.
        let t = tree(&[
            "session-2.scope: lightdm xfce4-session xfwm4 xfce4-panel Thunar firefox-esr",
            "user@1000.service/init.scope: systemd (sd-pam)",
            "user@1000.service/session.slice/pipewire.service: pipewire",
            "user@1000.service/session.slice/dbus.service: dbus-daemon",
            "user@1000.service/app.slice/xdg-desktop-portal-gtk.service: xdg-desktop-por",
        ]);
        let settled = [login("2", true, 10 * MIN)];
        assert_eq!(plan(&t, 1000, &settled, &[]), Plan::Whole);
        // Not while it is still starting: the old freeze-mid-start bug.
        let young = [login("2", true, Duration::from_secs(20))];
        assert!(matches!(plan(&t, 1000, &young, &[]), Plan::Apps { .. }));
    }

    #[test]
    fn no_standard_slices_falls_back_to_the_whole_slice() {
        // No user manager at all.
        let t = tree(&["session-4.scope: login bash"]);
        assert_eq!(plan(&t, 1000, &[login("4", false, MIN)], &[]), Plan::Whole);
        // An older user manager: apps straight under it, no session.slice.
        let t = tree(&[
            "session-1.scope: gdm-session-wor gdm-wayland-ses gnome-session-b",
            "user@1000.service/init.scope: systemd",
            "user@1000.service/gnome-shell-wayland.service: gnome-shell",
            "user@1000.service/gnome-launched-firefox.desktop-1234.scope: firefox",
        ]);
        assert_eq!(
            plan(&t, 1000, &[login("1", true, 5 * MIN)], &[]),
            Plan::Whole
        );
    }

    #[test]
    fn a_plasma_6_desktop_freezes_its_app_units() {
        let t = tree(&[
            "session-3.scope: sddm-helper startplasma-way",
            "user@1000.service/init.scope: systemd",
            "user@1000.service/session.slice/plasma-kwin_wayland.service: kwin_wayland",
            "user@1000.service/session.slice/plasma-plasmashell.service: plasmashell",
            "user@1000.service/app.slice/app-org.kde.konsole@a1b2.service: konsole bash",
            "user@1000.service/app.slice/app-steam@c3d4.service: steam",
            "user@1000.service/app.slice/dbus-:1.2-org.kde.kwalletd6@0.service: kwalletd6",
            "user@1000.service/app.slice/dbus-:1.9-org.kde.dolphin@0.service: dolphin",
            "user@1000.service/background.slice/plasma-baloorunner.service: baloorunner",
        ]);
        let p = plan(&t, 1000, &[login("3", true, 5 * MIN)], &[]);
        let m = "user@1000.service";
        assert_eq!(
            p,
            Plan::Apps {
                targets: vec![
                    format!("{m}/app.slice/app-org.kde.konsole@a1b2.service"),
                    format!("{m}/app.slice/app-steam@c3d4.service"),
                    format!("{m}/app.slice/dbus-:1.9-org.kde.dolphin@0.service"),
                    format!("{m}/background.slice"),
                ],
                strays: vec![]
            }
        );
    }

    #[test]
    fn apps_placed_by_hand_are_frozen_wherever_they_are_put() {
        // `systemd-run --user --slice=games.slice …` and `--slice=-.slice`.
        let t = tree(&[
            "session-1.scope: gdm-session-wor gdm-wayland-ses gnome-session-b",
            "user@1000.service/init.scope: systemd",
            "user@1000.service/session.slice/org.gnome.Shell@wayland.service: gnome-shell",
            "user@1000.service/games.slice/run-u12.service: supertuxkart",
            "user@1000.service/run-u13.service: mpv",
        ]);
        let p = plan(&t, 1000, &[login("1", true, 5 * MIN)], &[]);
        assert_eq!(
            p,
            Plan::Apps {
                targets: vec![
                    "user@1000.service/games.slice".into(),
                    "user@1000.service/run-u13.service".into(),
                ],
                strays: vec![]
            }
        );
    }

    #[test]
    fn a_login_scope_with_the_desktop_in_it_is_not_a_launcher() {
        // GNOME on Xorg: Xorg in the scope is still just the launcher's.
        let x11 = tree(&[
            "session-1.scope: gdm-session-wor gdm-x-session Xorg gnome-session-b",
            "user@1000.service/session.slice/org.gnome.Shell@x11.service: gnome-shell",
        ]);
        assert!(matches!(
            plan(&x11, 1000, &[login("1", true, 5 * MIN)], &[]),
            Plan::Apps { .. }
        ));
        // Something started from ~/.profile beside the launcher: the desktop
        // can't be told apart from what runs in the scope — freeze it all.
        let odd = tree(&[
            "session-1.scope: gdm-session-wor gdm-wayland-ses gnome-session-b mpv",
            "user@1000.service/session.slice/org.gnome.Shell@wayland.service: gnome-shell",
        ]);
        assert_eq!(
            plan(&odd, 1000, &[login("1", true, 5 * MIN)], &[]),
            Plan::Whole
        );
        // A scope logind no longer lists (closing) is never touched.
        let closing = tree(&[
            "session-1.scope: gdm-session-wor gdm-wayland-ses gnome-session-b",
            "session-9.scope: sshd bash",
            "user@1000.service/session.slice/org.gnome.Shell@wayland.service: gnome-shell",
        ]);
        assert_eq!(
            plan(&closing, 1000, &[login("1", true, 5 * MIN)], &[]),
            Plan::Apps {
                targets: vec![],
                strays: vec![]
            }
        );
    }

    #[test]
    fn strays_are_d_bus_started_apps_and_their_children_only() {
        let t = tree(&[
            // A script run through its interpreter matches its Exec; the
            // shell's own gjs services and the bus itself never do.
            "user@1000.service/session.slice/dbus.service: 614<590=/usr/bin/dbus-daemon 700<614=/usr/bin/gjs+/usr/bin/gnome-weather+--gapplication-service 701<700=/usr/bin/curl 842<614=/usr/bin/gjs+/usr/share/gnome-shell/org.gnome.Shell.Notifications",
            // A unit's own main process is never a stray, whatever it runs.
            "user@1000.service/session.slice/odd.service: 900<590=/usr/bin/totem+--gapplication-service",
        ]);
        let s = t
            .at("user@1000.service/session.slice")
            .map(|s| strays(s, &totem()))
            .unwrap();
        assert_eq!(
            s,
            vec![Stray {
                app: "org.gnome.Weather".into(),
                pids: vec![700, 701],
            }]
        );
    }

    #[test]
    fn holding_reads_the_kernel_not_the_plan() {
        let m = "user@1000.service";
        let plan = Plan::Apps {
            targets: vec![
                format!("{m}/background.slice"),
                format!("{m}/app.slice/app-x-1.scope"),
            ],
            strays: vec![],
        };
        let lines = |app_frozen: bool| {
            let app = if app_frozen {
                "user@1000.service/app.slice/app-x-1.scope*: x"
            } else {
                "user@1000.service/app.slice/app-x-1.scope: x"
            };
            tree(&["user@1000.service/background.slice*: tracker", app])
        };
        assert!(holds(&lines(true), &plan));
        assert!(!holds(&lines(false), &plan));
        assert!(any_frozen(&lines(false)));
        // A whole-slice freeze (an older agent's) holds anything.
        let mut whole = lines(false);
        whole.frozen = true;
        assert!(holds(&whole, &plan));
        assert!(holds(&whole, &Plan::Whole));
        assert!(!holds(&lines(true), &Plan::Whole));
        // A target that has gone (the app quit) is nothing left to stop.
        let gone = tree(&["user@1000.service/background.slice*: tracker"]);
        assert!(holds(&gone, &plan));
        // Nothing to freeze holds; nothing frozen is not "frozen".
        let empty = Plan::Apps {
            targets: vec![],
            strays: vec![],
        };
        let none = tree(&["user@1000.service/app.slice/app-x-1.scope: x"]);
        assert!(holds(&none, &empty) && !any_frozen(&none));
        assert_eq!(
            frozen_paths(&lines(true)),
            vec![
                format!("{m}/background.slice"),
                format!("{m}/app.slice/app-x-1.scope"),
            ]
        );
    }

    #[test]
    fn unit_names_are_unescaped_before_they_are_judged() {
        assert_eq!(
            unescape("app-gnome\\x2dsession\\x2dmanager.slice"),
            "app-gnome-session-manager.slice"
        );
        assert_eq!(unescape("plain.service"), "plain.service");
        assert_eq!(unescape("broken\\x2"), "broken\\x2");
        for p in [
            "gnome-keyring-daemon.service",
            "app-gnome\\x2dsession\\x2dmanager.slice",
            "app-gnome-at\\x2dspi\\x2ddbus\\x2dbus-717.scope",
            "dbus.service",
            "dbus-broker.service",
            "xdg-desktop-portal-gnome.service",
            "dbus-:1.2-org.kde.kwalletd6@0.service",
        ] {
            assert!(is_plumbing(p), "{p} is session plumbing");
        }
        for a in [
            "app-gnome-firefox\\x2desr-2001.scope",
            "dbus-:1.9-org.kde.dolphin@0.service",
            "app-org.gnome.Terminal.slice",
            "background.slice",
            "app-gnome-org.gnome.Evolution\\x2dalarm\\x2dnotify-894.scope",
        ] {
            assert!(!is_plumbing(a), "{a} is an app");
        }
    }

    #[test]
    fn desktop_and_service_files_are_read_like_dbus_reads_them() {
        let desktop = "[Desktop Entry]\nName=Videos\nExec=totem %U\nDBusActivatable=true\n\
                       [Desktop Action new]\nDBusActivatable=false\n";
        assert!(dbus_activatable(desktop));
        assert!(!dbus_activatable(
            "[Desktop Entry]\nExec=firefox\n[Other]\nDBusActivatable=true\n"
        ));
        assert_eq!(
            service_exec("[D-BUS Service]\nName=org.gnome.Totem\nExec=/usr/bin/totem --gapplication-service\n"),
            Some(vec!["/usr/bin/totem".to_string(), "--gapplication-service".to_string()])
        );
        assert_eq!(
            service_exec("[D-BUS Service]\nExec=\"/opt/My App/app\" 'a b'\n"),
            Some(vec!["/opt/My App/app".to_string(), "a b".to_string()])
        );
        // Started by systemd: it gets a unit of its own, not the bus's.
        assert_eq!(
            service_exec("[D-BUS Service]\nExec=/usr/bin/x\nSystemdService=x.service\n"),
            None
        );
        assert_eq!(service_exec("[D-BUS Service]\nName=x\n"), None);
    }
}
