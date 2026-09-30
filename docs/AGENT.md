# The OpenScreenTime Linux Agent

Reference for `openscreentime`, the Rust binary that runs on a managed Linux
machine: what it installs, what it writes to disk, how it enforces policy,
and how to debug it. For the server-side deploy, see `docs/DEPLOY.md`; for
the tamper threat model, see `docs/TAMPER.md`.

The agent is a single binary in two builds the server ships: **headless**
(x86_64, musl-static — no GUI, no tray) and **desktop** (x86_64 glibc,
`--features gui,tray`); `install.sh` picks one. Where a capability requires
`gui` or `tray`, this doc says so explicitly.

## Install & enroll

### One-liner (x86_64 — what the server serves)

```sh
(wget -qO- https://HOST/install.sh || curl -fsSL https://HOST/install.sh || echo "echo \"Couldn't download the installer from https://HOST — is the address right and the server up?\" >&2; exit 1") 2>/dev/null | sudo OST_TOKEN=xxx sh -s -- --server https://HOST
```

or with the token on the command line (`--token xxx` instead of the env
var — see the warning below):

```sh
(wget -qO- https://HOST/install.sh || curl -fsSL https://HOST/install.sh || echo "echo \"Couldn't download the installer from https://HOST — is the address right and the server up?\" >&2; exit 1") 2>/dev/null | sudo sh -s -- --server https://HOST --token xxx
```

The line downloads with wget where there is one (stock Debian and Ubuntu
have wget and no curl), else curl, both quietly. When neither download
works — no downloader, the server down, a typo in the address — it hands
`sh` one sentence naming the server ("Couldn't download the installer from
… — is the address right and the server up?") and an `exit 1`: never a
quiet exit 0 with nothing installed, never a bare "curl: command not
found". The script itself
runs only from `main "$@"` on its last line, so a download cut short runs
nothing. `sh -s -- --help` prints its options.

`server/install.sh` (served at `GET /install.sh`) does, in order:

1. Validates args: requires `--server https://HOST` and a token
   (`OST_TOKEN` env or `--token`); refuses plain `http://` unless
   `--insecure-http` is passed (dev only); requires root and `x86_64`;
   downloads with curl or wget, whichever is there (neither: it says so).
2. `GET {server}/api/agent/latest`, parses out the artifact to install
   (sed, not jq — the target may not have jq): `"desktop"` where a graphical
   session exists, else `"headless"`; `--desktop` / `--headless` force it,
   and an older server without a desktop build gets headless.
3. Downloads the binary to a temp file **in the same directory as the final
   target** (`/usr/local/bin/.openscreentime.download.$$`) so the final `mv`
   is an atomic rename on the same filesystem — a crash mid-download can
   never leave a truncated binary at `/usr/local/bin/openscreentime`.
4. Verifies `sha256sum` against the hash pinned in the manifest; refuses to
   install on mismatch.
5. `chmod 0755`, `mv -f` into place, then runs `OST_TOKEN=… ost enroll
   --server ...` (the token in the environment, never in argv) followed by
   `ost install-service`.

Prefer the `OST_TOKEN=xxx` env form over `--token xxx`: the installer
warns you if you use `--token`, because it can linger in shell history and
was briefly visible in the process list (`ps`). The token is single-use
either way.

### Manual build from source (required for `gui` / `tray`)

The server ships two builds: **headless** (musl-static, any x86_64 Linux)
and **desktop** (glibc, `--features gui,tray` — the graphical lock, the app
window, the companion). `install.sh` picks desktop where it finds a graphical
session (`--desktop` / `--headless` force it). To build one yourself:

```sh
cd client
cargo build --release --features gui,tray
sudo install -m 0755 target/release/ost /usr/local/bin/openscreentime
sudo OST_TOKEN=<ENROLL_TOKEN> ost enroll --server https://HOST
sudo ost install-service
```

`install-service` also drops the per-user companion unit
(`/etc/systemd/user/openscreentime-tray.service`); on a `tray` build it enables it
globally and adds an XDG autostart entry, and on a `gui` build it sets up the lock
screen (`ost-lock`, `openscreentime-lock@.service`, cage) — see
[systemd units](#systemd-units).

Self-update (below) keeps a build on its own variant: a desktop build only
ever installs the desktop artifact, a headless one the headless artifact.

## CLI reference

Global flags (apply to every subcommand):

| Flag | Effect |
|---|---|
| `--dry-run` | Log every enforcement action instead of touching the host. Safe to run as non-root. |
| `--tamper-max` | Raise the tamper ceiling to level 3 (opt-in maximum lockdown — see `docs/TAMPER.md`). |
| `--time-accel <N>` | Accelerate screen-time accounting for local dev (`N=60` → 1 real second counts as 1 minute). Default 1. |

Subcommands:

| Subcommand | Flags | What it does |
|---|---|---|
| `enroll` | `--server <URL>`, the token in `OST_TOKEN` (or `--token -` to read it from stdin; `--token <TOKEN>` works but shows in `ps`) | Reports hostname, OS users, and agent version to the server; receives `device_id` + `device_token`; writes `/etc/openscreentime/agent.toml` (root-owned `0600`). |
| `run` | — | The main loop: connects the WS command bus (falls back to heartbeat polling), pulls and enforces policy, dispatches server commands, streams events. Requires root unless `--dry-run`. Requires a prior `enroll`. |
| `install-service` | — | Copies the running binary to `/usr/local/bin/openscreentime`, installs what the network rules need where it's missing (the `dnsmasq` *service* — NetworkManager's `dnsmasq-base` is only the program — and `nftables`, via apt/dnf/pacman/zypper; dnsmasq's first start pre-seeded to listen on 127.0.0.1 only with `bind-interfaces`, next to systemd-resolved's 127.0.0.53; on a distro whose dnsmasq reads no config directory, Arch, two marked lines in `/etc/dnsmasq.conf` name ours), makes `/etc/resolv.conf` a file where it is systemd-resolved's or resolvconf's link (keeping the link to put back when OpenScreenTime leaves — the sandboxed agent can't, and resolved replaces its file on every network change), writes the hardened systemd unit + watchdog timer + polkit rule, writes the (best-effort) tray user unit, then `daemon-reload`, enables and **restarts** `openscreentime-agent.service` (so re-running the one-liner reaches a running agent with its new token) and enables `openscreentime-watchdog.timer`. On a tray build it also starts the companion for whoever is signed in to a desktop right now (`systemctl --user -M <user>@ start`, else in their session via `runuser`). Requires root. |
| `status` | `--json` | Prints enrollment state (server, device ID, tamper level, poll interval), whether the process is root, and `systemctl is-active openscreentime-agent.service`. Safe non-root. |
| `time` | `--json` | How much screen time the calling user has left today. Reads the per-user status snapshot the agent writes each tick. Safe non-root, no display needed. |
| `ask` | `--json` | Sends a time request to a parent, from the keyboard. Writes a marker inside the caller's own `/run/user/<uid>/openscreentime/` — which is what proves the request came from them. Safe non-root. |
| `login` | `--print-url` `--json` | Opens the console in a browser, already signed in, using this computer's enrollment as proof. See [Autologin](#autologin-ost-login). |
| `pair` | `--server <URL>` `--token <TOKEN>` | Stores a scoped **parent access token** (minted in the web console → Settings → Parent access) at `~/.config/openscreentime/parent.toml` (`0600`). Enables the tray's parent mode. Runs as the desktop user, never root. |
| `tray` | — | *(feature `tray` only)* Per-user tray companion — see [Build features matrix](#build-features-matrix). With a `pair`ed token it also shows and approves time requests. Runs as the desktop user, never root. |
| `unlock` | `--code <CODE>` `--minutes <N>` (default 60) | Parent recovery: verifies the **unlock code** (the 6 digits the console shows, a one-time recovery code, or a profile backup code) fully offline, then suspends enforcement (removes the nft table, un-pins `resolv.conf`, un-freezes every login user) for `N` minutes. Omit `--code` and it is read from the terminal (`--pin` is a hidden alias). Requires root. See [Parent code](#parent-code). |
| `uninstall` | — | Disables and removes the systemd units, the sudo/PAM unlock-code hook and nothing else (enrollment config, state and the binary stay). Requires root. |

Some subcommands are intentionally hidden — not in `--help`, not real
`clap::Subcommand` variants, invoked only by the agent itself:

| Hidden subcommand | Who spawns it | Purpose |
|---|---|---|
| `__lock-session` | `openscreentime-lock@<vt>.service`, as `ost-lock` (`gui` build) | Starts `cage` (no `-s`) hosting `__lockscreen`; if cage exits within 6 s (a GPU wlroots can't drive) it is run once more with `WLR_RENDERER=pixman LIBGL_ALWAYS_SOFTWARE=1`. Fails if cage is missing or both fail — the agent then draws the text lock (on VT 14). |
| `__lockscreen` | `cage`, inside the lock unit (`gui` build) | The graphical lock window. Shows what the agent publishes and sends typed codes / "ask" / "snooze" over `/run/openscreentime/lock.sock`; holds no secret. |
| `pam-auth` | `pam_exec.so` from `/etc/pam.d/openscreentime-parent` (i.e. `sudo` on a managed machine) | Reads the typed token from stdin, verifies it as an unlock code offline, posts a `parent_code_*` event (5 s bound, best-effort), exits 0/1. See [Parent sudo](#parent-sudo-pam). |
| `__resume-enforcement <secs>` | `ost unlock`, detached | Sleeps out the suspend window from `unlock`, then re-applies the cached policy once and exits. |

### Machine-readable output

Every read subcommand takes `--json`. The contract:

* **stdout carries the JSON and nothing else.** Logs, warnings and progress all
  go to stderr, so `ost time --json | jq` works without filtering.
* **Exit code 0 means the JSON is meaningful.** A non-zero exit means the
  question could not be answered (not enrolled, agent not running); do not
  parse stdout in that case.
* Fields are added, never repurposed. A consumer that ignores unknown keys
  keeps working across upgrades.

```console
$ ost time --json
{
  "limited": true,          // false = no limit configured for this user
  "used_minutes": 32,
  "left_minutes": 28,       // null when "limited" is false; minutes until the screen stops
  "unlocked_until": null,   // a parent's override keeps it on until then (RFC 3339)
  "frozen": false,          // the screen is paused right now
  "freeze_in_secs": null    // set during the save-your-work countdown
}
```

`limited` exists so "no limit is set" can never be mistaken for "no time
left" — the one distinction a consumer must not get wrong.

```console
$ ost status --json
{ "enrolled": true, "server_url": "https://…", "device_id": "…",
  "tamper_level": 1, "poll_interval_secs": 30,
  "config_path": "/etc/openscreentime/agent.toml",
  "root": false, "service": "active" }
```

`status --json` never includes `device_token`: it is the subcommand most likely
to be piped somewhere, and the token is a bearer credential.

### Autologin (`ost login`)

`ost login` opens the web console already signed in, using the machine's own
enrollment as the proof of identity — no password, no passkey prompt.

```console
$ ost login
Opening the console — you'll already be signed in.
(The link is good for 120 seconds.)

$ ost login --print-url        # headless, or open it on another machine
https://ost.example.com/#v=8ba0ffff…
```

The agent asks the server for a one-time voucher over its device token and puts
it in the URL **fragment**. That is deliberate: a fragment is never sent to a
server, so the voucher cannot appear in an access log or a proxy trace. The
console redeems it on load and strips it from the address bar with
`history.replaceState`, leaving no history entry to go Back to.

What a voucher is:

* A sign-in for **the person behind the OS login that asked** — a child's
  login opens the child's page. A parent is vouched for only from their own
  computer, and there only from its owner's login (docs/AUTH.md).
* A sign-in like the others: the session (7 days) opens with the 15-minute
  "confirm it's you" window, and a parent's raises an `account_login` alert.
* **Single-use**, and it expires after two minutes.

### Environment

| Variable | Purpose |
|---|---|
| `OST_CONFIG` | Read/write the agent config at this path instead of `/etc/openscreentime/agent.toml`. For development and tests — it lets `enroll`, `status` and `login` be exercised without root. The systemd unit sets no such variable, so it cannot redirect what the real root agent reads. |
| `OST_NO_SELF_UPDATE=1` | Disable the daily self-update at runtime. |

## Files on disk

`/run/openscreentime/login/` — the **login broker** drop-box (sticky
`1733`, like /tmp). `ost login` run by an ordinary desktop user writes
`<user>.req`; the root agent checks the file is owned by that user's uid, mints
a device voucher bound to the person behind that OS login, and answers with
`<user>.url` (`0600`, owned by the user). The CLI reads it, deletes it and
opens the console already signed in — as *that person*, never as the parent.
Run as root (`sudo ost login`) the CLI mints directly with `SUDO_USER`.


| Path | Owner : mode | Written by | Purpose |
|---|---|---|---|
| `/usr/local/bin/openscreentime` | root : 0755 | `install.sh` / `install-service` / self-update | The managed binary. `ExecStart` target for the systemd unit. |
| `/usr/local/bin/openscreentime.bak` | root : 0755 | self-update | The previous binary. The watchdog puts it back automatically if a new build crash-loops or stops ticking before it confirms itself. |
| `/usr/local/bin/.openscreentime.new` | root : 0755 | self-update (transient) | Staging path for a downloaded update; renamed over the install path once verified. |
| `/usr/local/bin/.openscreentime.download.$$` | root : — | `install.sh` (transient) | Staging path for the initial download; renamed atomically into place, cleaned up by a trap on any exit. |
| `/etc/openscreentime/agent.toml` | root : **0600** | `enroll` | Persisted identity: `server_url`, `device_id`, `device_token`, `poll_interval_secs`, `tamper_level`, `auto_update`. See [Config fields](#config-fields). |
| `/etc/openscreentime/policy_cache.json` | root : **0600** | `run` (after every applied policy bundle) | Last-applied effective `Policy`, JSON. Not read by enforcement itself (that's in-memory); exists only so `unlock` knows what to tear down without a live agent process. |
| `/etc/openscreentime/policy_bundle.json` | root : **0600** | `run` (after every applied policy bundle) | The whole last bundle, verbatim — per-user policies, VPN profile and the device's `parent_code` (TOTP secret + unused recovery-code MACs). The boot fallback when the server is unreachable, and what `unlock` / `pam-auth` / the agent (for codes typed at the lock) verify unlock codes against. |
| `/var/lib/openscreentime/parent_code.json` | root : **0600** | `run` (codes typed at the lock), `unlock`, `pam-auth` | Unlock-code replay counter (last accepted TOTP step — a code is single-use), the ids of recovery codes already spent on this device, and the wrong-attempt counter / lockout deadline. |
| `/etc/pam.d/openscreentime-parent` | root : default | `install-service` | PAM service: `auth required pam_exec.so expose_authtok quiet /usr/local/bin/openscreentime pam-auth`. Removed by `uninstall`. |
| `/etc/sudoers.d/10-openscreentime` | root : **0440** | `install-service`, then `run` on every policy apply (staged under a dot-name, `visudo -c -f` validated, renamed into place) | `Defaults:<managed users> pam_service=openscreentime-parent, timestamp_timeout=0` + `<managed users> ALL=(ALL:ALL) ALL`. Managed = every OS user whose profile kind is not `adult`/`default`. Removed by `uninstall`. |
| `/etc/openscreentime/dnsmasq.d/openscreentime.conf` | root : default | `run` (DNS enforcement) | Rendered dnsmasq ruleset realizing the DNS policy. |
| `/etc/resolv.conf` | root : default, **immutable (`chattr +i`)** | `run` (DNS enforcement) | Pinned to `nameserver 127.0.0.1`; the immutable bit stops a managed user from repointing it. Re-asserted every tick if it drifts. |
| `/etc/wireguard/openscreentime.conf` | root : **0600** | `run` (VPN enforcement) | The device's WireGuard client config, verbatim as uploaded in the console (it contains the private key — hence 0600, and dry-run logs withhold its contents). Present only while a `wireguard` profile is set; runs as `wg-quick@openscreentime`. |
| `/etc/openvpn/client/openscreentime.conf` | root : **0600** | `run` (VPN enforcement) | Same for an OpenVPN profile; runs as `openvpn-client@openscreentime`. |
| `/etc/polkit-1/rules.d/49-openscreentime.rules` | root : default | `run` (bootstrap, and whenever the tamper level changes) | Tamper level 3 only: denies `systemctl stop/disable/mask` of the agent and watchdog units to everyone but `root` and `ost-admin`. Below level 3 there is no rule, and the agent removes the file (earlier builds wrote one denying power-off/reboot/suspend at every level). |
| `/etc/systemd/logind.conf.d/50-openscreentime.conf` | root : default | `run` (tamper level 3 only) | `ReserveVT=0` / `KillUserProcesses=yes` drop-in — disables TTY/VT switching for managed sessions. |
| `/run/openscreentime/heartbeat` | root : default | `run` (every tick) / `install-service` | mtime = liveness signal for `openscreentime-watchdog.timer`. |
| `/run/openscreentime/status.json` | root : world-readable (0755 dir) | `run` (every tick, atomic rename via `.tmp`) | Device-wide snapshot for the tray/app: connection state, device-lock / offline-lockdown / tamper-lockdown flags and device-wide notifications. **No per-user data** (`users: []`). |
| `/run/openscreentime/status.<user>.json` | the user : **0600** | `run` (every tick) | That user's own view: the device-wide fields plus their **verdict** — time used, budget left, whether and when screens stop and why, the next heads-up, any parent override — and their notifications and sign-in prompts. Field by field in [The verdict](#the-verdict-statususerjson). |
| `/run/openscreentime/lock.sock` | root : group `ost-lock`, **0660** | `run` | The graphical lock's line to the agent. The agent answers only a peer whose `SO_PEERCRED` uid is `ost-lock`; one JSON request (`face` / `code` / `ask`) and one reply per connection. |
| `/var/lib/openscreentime/freeze_state.json` | root : default | `run` (every tick) | Who is stopped (or inside a save-your-work countdown), a confirmed-evasion lockdown, and the lock on screen (`lock`: subject, VT, mode, boot id) — so a restarted agent adopts the lock instead of forgetting it. |
| `/var/lib/openscreentime/last_contact` | root : default | `run` (throttled, at most once/60s, on successful server contact) | RFC3339 wall-clock timestamp of the last successful server contact. Survives reboots — it's what the days-scale offline hard-lockdown timer is measured against (an `Instant` can't survive a reboot). |
| `/var/lib/openscreentime/usage_ledger.json` | root : default | `run` (every tick, and on `credit_time` / `unlock` / `ost unlock`; atomic rename via `.tmp`) | The day's ledger: per-user used and earned seconds on this device, the person's use elsewhere as last reported by the server (tagged with its day), **parent overrides** (user → end, trusted UTC), applied grant command ids (idempotency), and the **trusted-clock anchor** (boot id, boottime, wall). Reloaded on startup so a restart resumes today's usage and keeps a parent's override. The day boundary is forward-only and follows the trusted clock (see [Screen time](#screen-time)). |
| `/var/lib/openscreentime/local_recovery` | root : default | `ost unlock` / `ost recover` | `"<unix secs> <minutes>"` — a parent recovered the device at the machine. The live agent clears every device-level lock once per marker and, when `minutes > 0`, holds the screen-time rules off for everyone on the machine for that long. |
| `~/.config/openscreentime/parent.toml` | the desktop user : `0600` | `pair` (writes) / `tray` (reads, parent mode) | A paired parent's server URL + scoped access token. Written by `ost pair`; read by the tray to enable parent mode. Not present unless the machine was paired. |
| `~/.config/openscreentime/intro_seen` | the desktop user : default | `app` (writes, as the window opens with the cards) / `tray` + `app` (check) | The first-run cards (shown inside the app window) have been shown — skipping or closing counts. Absent = the companion opens the window once. |
| `/run/user/<uid>/openscreentime/app.lock`, `app.sock` | the desktop user | `app` | One window per person: the first holds the lock; a second `ost app` rings the socket (the first comes forward) and exits. |
| `/run/user/<uid>/openscreentime/earn_request` | the desktop user : `0700` dir | written by the `tray` or the `app` ("Ask for more time"); consumed by `run` every tick | An on-demand "request more time" marker. The unprivileged tray can only write inside its own `/run/user/<uid>`, which only that user and root can touch — so the root agent trusts it as an authentic request from that user (a spoof-proof privilege bridge). Single-use: read once, deleted, filed as an earn-request. |

### Config fields

`/etc/openscreentime/agent.toml`, root-owned `0600`, written by `enroll`:

| Field | Default | Meaning |
|---|---|---|
| `server_url` | — | The enrolled server's base URL. |
| `device_id` / `device_token` | — | Issued by the server at enroll time. |
| `poll_interval_secs` | `30` | Heartbeat interval used by the polling fallback (when the WS bus is unavailable). |
| `tamper_level` | `1` | Persisted starting level; the effective level is `max(this, 3 if --tamper-max else 1)`, never above the computer's ceiling (3 with `--tamper-max`, else 1). A `set_tamper_level` command moves it within that ceiling; a request above it is capped and reported (`capped` in the ack, a `tamper_level_capped` event). |
| `auto_update` | `true` | Daily self-update from the enrolled server. `false` disables it; see [Self-update](#self-update) for the other kill switches. |

## systemd units

Installed by `install-service` (source in `client/systemd/`):

| Unit | Path | Purpose |
|---|---|---|
| `openscreentime-agent.service` | `/etc/systemd/system/` | The agent itself: `ExecStart=/usr/local/bin/ost run`. |
| `openscreentime-watchdog.service` + `.timer` | `/etc/systemd/system/` | Oneshot check every 30s (after a 60s boot delay): if `/run/openscreentime/heartbeat` is missing or older than 90s, `systemctl restart openscreentime-agent.service`. |
| `openscreentime-tray.service` | `/etc/systemd/user/` | The per-user companion (warnings, "You're back"). Enabled globally on a `tray` build (`WantedBy=graphical-session.target`); `/etc/xdg/autostart/openscreentime-companion.desktop` starts it where there is no systemd user session. One instance per person. |
| `openscreentime-lock@.service` | `/etc/systemd/system/` | The lock screen on VT `%i` (the agent uses 13): `cage` as `ost-lock` with its own logind session (`PAMName=openscreentime-lock`, `TTYPath=/dev/tty%i`). Started and stopped by the agent, never enabled; separate from the agent unit so restarts never take a lock down. `gui` build only. |

`openscreentime-agent.service` hardening highlights (tamper level 1 baseline, see
`docs/TAMPER.md`):

- `Restart=always`, `RestartSec=1`, `StartLimitIntervalSec=0` — never gives
  up restarting.
- `OOMScoreAdjust=-1000` — survives OOM pressure; the agent must not be the
  first thing killed.
- `ProtectSystem=strict` with an explicit `ReadWritePaths=` carve-out for
  `/etc/openscreentime /var/lib/openscreentime /run/openscreentime /etc/resolv.conf
  /etc/polkit-1/rules.d /etc/systemd/logind.conf.d` — everything else is
  read-only.
- `ProtectHome=false` **intentionally** — the agent must watch user
  sessions/cgroups.
- `NoNewPrivileges=false` **intentionally** — enforcement shells out to
  `nft`, `resolvectl`, `chattr`.
- A commented-out `WatchdogSec=30` line for `sd_notify`-based watchdogging,
  as an alternative to the separate `openscreentime-watchdog.timer`.

The polkit rule (`49-openscreentime.rules`) exists at tamper level 3 only:
it denies `systemctl stop/disable/mask` on `openscreentime-agent.service` and
the watchdog to everyone but `root` and `ost-admin` — that's the recovery
path. Power-off, reboot and suspend are never blocked; the persisted ledger
and `freeze_state.json` make a restart come back to the same day and stop.

## Build features matrix

Set via `cargo build --release --features <list>` (comma-separated). All are
additive; `default = []`.

| Feature | Adds | What you get |
|---|---|---|
| *(none — the headless build)* | — | Full enforcement (DNS, firewall, screen time, tamper hardening, self-update). The lock is the text lock on its own VT. No `tray` subcommand. |
| `gui` | `eframe`/`egui` | The graphical lock (`__lock-session` / `__lockscreen` inside `cage`, see [The lock](#the-lock)); `install-service` also creates `ost-lock`, the lock unit and its PAM file, and installs cage where apt/pacman/dnf has it. |
| `tray` | `ksni` (StatusNotifierItem) + `notify-rust` | The `tray` subcommand: a per-user, non-root system tray icon + desktop notifications reading `/run/openscreentime/status.json`. In **parent mode** (after `ost pair`) a background worker also polls `/api/parent/*` to show pending time requests + alerts and approve/deny them from the menu. A `gui`/`tray` build self-updates from the desktop artifact. |

Both `gui` and `tray` can be combined (`--features gui,tray`) for a full
desktop build. The `install-service` unit files are the same regardless of
features — the tray *user* unit is always written, it's just inert without
`tray`.

## How enforcement works

### DNS

`client/src/enforce/dns.rs`. Renders a dnsmasq config
(`/etc/openscreentime/dnsmasq.d/openscreentime.conf`) and restarts the local `dnsmasq`
(falling back to `resolvectl flush-caches` if that's what's running
instead). Since 0.6 the network is **open by default** (`allow_all`, every
bracket): every query forwards to the filtered `upstream`, and blocking is
the blocklist — categories, apps, sites — sinkholed exactly. Only an old,
hand-edited profile with an explicit `default_deny` (and not a `*`
allowlist) gets allowlist mode: allowlisted domains forward, a trailing
`address=/#/` NXDOMAINs the rest. `block_tor` NXDOMAINs
`.onion` and `torproject.org`. `safe_search` points the search engines'
own names at their safe-search front ends, as each vendor documents it:
`www.google.<every Google country domain>` → `forcesafesearch.google.com`;
`www.youtube.com`, `m.youtube.com`, `youtubei.googleapis.com`,
`youtube.googleapis.com`, `www.youtube-nocookie.com` →
`restrict.youtube.com`; `www.bing.com` → `strict.bing.com`;
`duckduckgo.com` → `safe.duckduckgo.com` (`enforce/safesearch.rs`). Never
with `cname=`: dnsmasq only follows a CNAME to a name it knows itself, so
that answered with no address and took the sites away. The agent looks the
front ends up straight from the upstream (A + AAAA) and writes
`host-record=` lines with their addresses — at start (the last lookup is
kept in `/var/lib/openscreentime/safesearch.json`, so a reboot redirects at
once), hourly, a minute after an incomplete lookup, and whenever the default
routes change (a new network). A front end that doesn't resolve is not
redirected: its names pass through, the site works without safe search, and
the gap `dns_safesearch_unavailable` says so. A name a block covers stays
blocked. `/etc/resolv.conf` is pinned to
`127.0.0.1` and set immutable (`chattr +i`) — **only while dnsmasq is
running**: the tick re-pins on drift only then, and when the resolver is gone
it takes its own pin off and puts back what the computer used before (kept in
`/var/lib/openscreentime/resolv.conf.pre-pin`; else NetworkManager's or
systemd-resolved's file; else the family resolvers). A computer that can't
filter stays online and says so (`dns_resolver_missing`,
`dns_no_local_resolver`). dnsmasq reads the ruleset through a one-line
`conf-dir=` stub in the directory it really reads (Debian/Ubuntu's
`CONFIG_DIR` in `/etc/default/dnsmasq`, a `conf-dir=` in `dnsmasq.conf` on
Fedora), or through `dnsmasq.conf` itself where it reads none (Arch).

DNS, the firewall, its lockdown rules and the VPN are applied as separate
stages: one that fails is its own gap and never skips the others (a
resolv.conf that can't be written leaves the firewall loaded; lockdown rules
the kernel refuses leave the base firewall loaded, `firewall_lockdown_not_applied`).
Forced DNS is held back whenever nothing would answer it.

### Firewall

`client/src/enforce/firewall.rs`. A single `inet openscreentime` nftables table,
applied atomically via one `nft -f -` transaction (`add table` → `delete
table` → fresh rules) so a malformed policy aborts the whole load and
leaves the last-known-good table in place, never a fail-open gap.
The chains' policy is `accept` (open by default, CONTRACT-0.6); only an
explicit `default_deny` firewall mode makes it `drop`, with
`established,related` and loopback always accepted and output always
allowing the DNS upstream and (if it's a literal IP) the enrolled server. `NetworkLockdown` toggles add **drop**
rules ahead of those generic accepts (nftables is first-match-wins):
`block_dot` (853), `force_dns` (non-upstream port 53), `block_doh` (a
hardcoded list of public DoH resolver IPs, excluding the configured
upstream), `block_vpn` (WireGuard/OpenVPN/IPsec ports), `block_tor`
(OR/directory/SOCKS ports). A missing table is detected every tick
(`table_missing`) and immediately re-applied with the last effective
policy.

### VPN profile

`client/src/enforce/vpn.rs`. The policy bundle can carry a device-level
`vpn` profile — a WireGuard or OpenVPN client config uploaded in the
console (device → VPN PROFILE). The agent reconciles declaratively on
every policy apply: profile present → write the config (root-only `0600`;
dry-run logs withhold the body — it contains the private key), `systemctl
enable` + `restart` the matching unit (`wg-quick@openscreentime` /
`openvpn-client@openscreentime`, switching kinds tears the other down); profile
absent → stop/disable the unit and delete the config. The firewall
cooperates: the tunnel interface (`openscreentime` / `tun*`) and the parsed
endpoint(s) (`Endpoint =` / `remote` lines) are accepted **ahead of** the
lockdown drop rules, so the parent's own tunnel survives `block_vpn` and
default-deny. A profile whose unit isn't active after apply (wg-quick /
openvpn not installed, bad config) is reported as an
`enforcement_degraded` critical event (`vpn_not_running`) — never a silent
green. CLI paths without server state (`ost unlock`) never
touch the tunnel.

### Screen time

Three pieces, each small and tested on its own. The full audit and the
reasoning behind every rule is `docs/TRACKING.md`.

**What counts** — `client/src/enforce/activity.rs`. A minute is billed to a
person only while their session is the **foreground session on a seat**
(`loginctl`: `Active=yes`, `State=active`, a `Seat`, a `user*` class) **and**
there was keyboard/mouse/touch/gamepad input on that seat in the last
**5 minutes**, or sound is playing. Not counted: the systemd ≥ 256
`Class=manager` session, `closing` leftovers, SSH logins (seatless),
fast-user-switched background sessions, a locked screen nobody touches, a
closed lid. Input is read by root from `/dev/input/event*`
**non-exclusively** (never grabbed; only the event *type* is looked at, one
"last input" time per seat is kept, no key codes are stored or sent); sound
from `/proc/asound/card*/pcm*p/sub*/status` (`state: RUNNING`). Where no input
device can be read (a container, no `/dev/input`), the seat falls back to
presence and the status says `measured: false`. A frozen user never accrues.

**How long** — `client/src/runner.rs` (`tick_loop`, `billable_elapsed`). The
enforcement tick runs every 10 s **on its own timer**, independent of the
WS/poll loops, and bills the measured *awake* time since the last tick
(`CLOCK_MONOTONIC` — a suspended laptop bills nothing), capped at 60 s per
tick. The watchdog heartbeat is touched by this tick, so a healthy agent
with no server stays healthy.

**Which day** — `client/src/clock.rs`. Every decision reads the *trusted
clock*: the wall clock while the kernel says it is NTP-synchronized, else
the family server's clock (sent with every usage reply), else the last
anchor extrapolated by `CLOCK_BOOTTIME` (real time including suspend; nobody
can set it). A wall clock moved by hand is therefore ignored: the day rolls
at local midnight, **forward only**, never earlier than real elapsed time
allows, and never needs the server. On a new boot the anchor is the wall
clock, but never earlier than the last trusted time before shutdown.

**The rules** — `openscreentime_policy::rules::evaluate` (the same function
the server uses for the console). Limit 0 = no limit; a day without an
allowed-hours window is **any time**; a window ending `00:00` runs to
midnight; an end before the start runs past midnight; an empty or unreadable
window, or a whole-day bedtime, is ignored (never a 24/7 lockout — the server
refuses them on save). It returns *allowed?*, *why*, and *when the next stop
lands*, whichever comes first of the budget running out (assuming continuous
use), bedtime, the end of the allowed window, or the end of an override.

**Per person.** A daily limit is one budget across all of a person's
computers. Each usage report carries this device's own seconds, its local
day and UTC offset; the server files it under that day and answers with what
the same person used (and was granted) on their other logins that day. The
device enforces its own use plus that; offline, the last answer for today
keeps applying.

**One override per person on this device**, persisted in the ledger, beats
the limit, bedtime and allowed hours, and expires on the trusted clock.
Written by every parent action:

| Parent action | Override |
|---|---|
| `credit_time` (approved request, console "+N min") | N minutes on today's budget **and** an override for N minutes — "N more minutes, now, whatever the rule". Idempotent by command id (a redelivery after a lost ack is acked as `duplicate`, never credited twice); a grant filed for an earlier day is acked `stale_day` and not credited. |
| Code at the lock screen | 30 minutes (plus every device-level lock cleared). |
| `ost unlock --minutes N` | N minutes for everyone on the machine (plus locks cleared). |
| Console Resume (`unlock`) | Clears the pause — and gives nobody time: whoever their own rules stop (time's up, bedtime) stays stopped, and the lock says why. An explicit grant in the payload (`os_username` with `minutes` or `until: "end_of_day"`) overrides that person for that long. |

A **pause beats an override** (a parent who gives "+30" and then pauses
means the pause); every source that should lift a pause clears it directly.

**Warnings and grace**: every stop is announced at 15, 5 and 1 minute
(see [The lock](#the-lock)). A stop whose 1-minute warning went out, or that
someone logs into, lands at T-0. A stop nobody saw coming (a rule changed, a
code's 30 minutes ran out) gets a 60 s save-your-work countdown (120 s for
teens) that the companion counts down as a notification. An **admin lock**
(`lock` command, or offline hard-lockdown) is immediate. Someone who isn't
logged in is never frozen; they meet the lock when they log in.

**The freeze stops the person's apps, never their session**
(`enforce/screentime/freeze.rs`). The lock has its own VT, so their session
gets no input and shows nothing while it holds; the freeze only has to stop
what their apps do (sound, video, a game, a download). It writes
`cgroup.freeze` under `/sys/fs/cgroup/user.slice/user-<uid>.slice/`, on:

- every slice and unit their user manager (`user@<uid>.service`) runs
  **except `session.slice`** (compositor, session manager, settings
  daemons, the session bus, the sound server, portals) and `init.scope`
  (the manager itself) — `background.slice` whole, and whatever a
  `systemd-run --user --slice=…` put anywhere else;
- **`app.slice` unit by unit, never as one piece**: GNOME 43 (Debian 12)
  starts its session manager, the keyring, the accessibility bus and the
  portals there. Those, by unit name, are left running (the keyring is what
  GDM re-authenticates through); a sub-slice without any of them (a
  terminal's) is frozen whole;
- apps **D-Bus started inside `session.slice`**: on a session bus without
  dbus-broker (Debian 12) Videos, Files and Text Editor run inside the bus's
  own `dbus.service`. A process there running a `DBusActivatable` desktop
  application's D-Bus command line (and what it started) is first filed into
  a scope of its own in `app.slice` — `StartTransientUnit` with its PIDs on
  their own service manager, the call GNOME Shell makes for every app it
  launches — and frozen there;
- their **text-console and SSH logins** (`session-N.scope`) whole — never
  their graphical login's scope (GDM's worker, the session launcher, Xorg).

**Fallback**: a desktop that doesn't live under the user manager's
standard slices — no `session.slice`, or a graphical login scope holding
more than the launchers of a systemd-managed session (a legacy X session,
where the window manager and every app run in `session-N.scope`) — gets the
old whole-`user-<uid>.slice` freeze. **A session still starting is left
alone**: until a graphical login is a minute old nothing of its desktop is
frozen (the lock is on screen already), so GNOME finishes starting behind
the lock. Every tick re-applies a stop quietly: an app that started since (a
timer's; a new app in `app.slice` is caught within a tick, while one in
`background.slice` or another frozen slice is frozen at birth) and anything
thawed is frozen again, without taking the screen from whoever has it. A
thaw writes `0` to every cgroup of theirs that reads frozen.

If a write fails: a `hard` freeze (admin lock) falls back to `loginctl
terminate-user`; a screen-time freeze (`hard=false`) never escalates to
terminating the session — unsaved work must never be destroyed over a time
limit, so it just logs and stays best-effort.

A code typed at the lock is checked by the agent the moment it's typed
(see [The lock](#the-lock)): it clears every device-level lock, thaws, and
writes the override above for 30 minutes.

### The verdict: `status.<user>.json`

What the app window, tray and lock screen build on. Written every tick
(10 s) by `Agent::user_status`, from the same rules function and trusted
clock the enforcement tick uses — what it says is what will happen. Each
file carries `users: [ {…} ]` with exactly one entry:

| Field | Type | Meaning |
|---|---|---|
| `name` | string | The OS login. |
| `used_minutes` | int | Minutes the **person** used today, on every computer (this one + what the server reported for the others). Floored. |
| `used_here_minutes` | int | Of which on this computer. |
| `remaining_minutes` | int \| null | The day's **budget** left: limit + earned − used, rounded up, may be ≤ 0. `null` = no daily limit. Only for readers of an older agent: it knows nothing of bedtime, the hours or an override (after an unlock code it is ≤ 0 while the person has 30 minutes). Time left is `minutes_left`. |
| `allowed` | bool | May they use the screen right now? |
| `reason` | `"limit"` \| `"bedtime"` \| `"outside_hours"` \| `"paused"` \| null | Why they are stopped now (`allowed: false`), or why the **next** stop will come (`allowed: true`). `null` = no stop ahead. |
| `minutes_left` | int \| null | **Time left** — the one number every surface shows (the app window, the companion, `ost time`, and the console, which computes it with the same rules function and inputs): minutes until `stop_at`, **rounded up**, honouring the budget (assuming continuous use), bedtime, the end of the allowed window, the end of an override and a pending pause — whichever comes first. During a save-your-work countdown it is the countdown (and `allowed` is still true). `0` when stopped; `null` when nothing stops them in the next 48 h. Readers count down live to `stop_at` (`glance::Clock::left`), except for an idle person's limit stop, which slides later every tick. |
| `stop_at` | RFC 3339 local \| null | When that stop lands. Equals "now" when stopped. A budget stop moves later while they're idle (idle time isn't billed). |
| `resume_at` | RFC 3339 local \| null | When stopped: when the screen comes back on its own (bedtime ends, the window opens, midnight's fresh budget). `null` when allowed or paused. |
| `next_warning_at` | RFC 3339 local \| null | The next heads-up: 15, 5, then 1 minute before `stop_at` — when the companion announces it. `null` when none is ahead (under a minute left, or no stop). |
| `override_until` | RFC 3339 local \| null | A parent override is running until then. When `stop_at` is that moment, the override is what keeps the screen on, and the surfaces say "Unlocked until 00:27" instead of a red zero. |
| `counting` | bool | This minute is being billed (at the seat, with recent input or sound, not frozen). |
| `ask_pending` | bool | An "Ask for more time" is waiting on a parent today. A grant or a "not now" clears it; the app window offers Ask again. |
| `measured` | bool | Input activity could be read for every present seat (`false` = presence fallback). |
| `day` | `YYYY-MM-DD` \| null | The accounting day (trusted local date). |
| `frozen` | bool | The agent has frozen this user. |
| `freeze_in_secs` | int \| null | A save-your-work countdown is running; seconds left. |

`--dry-run` doesn't write these files; it logs the same JSON once a minute
as `STATUS <user>: {…}`.

### The lock

`client/src/lock/`. The lock never lives inside the session it stops: it is
its own session on its own VT (13), so that session — left running, its
apps frozen (see [Screen time](#screen-time)) — gets no
input and shows nothing while the lock holds the screen:

- **Graphical**: `openscreentime-lock@13.service` runs `cage` (without `-s`,
  so the keyboard can't switch VTs) as the unprivileged system user
  `ost-lock`, hosting `ost __lockscreen`. It is brand board 05a: the day's
  ring completed in red with "0 min left" inside (a parent's pause: the
  neutral dashed ring), one sentence ("Time's up for today", "Bedtime until
  07:00", "Paused by a parent", "Outside allowed hours until 15:00") and a
  second line ("You used all 90 minutes. Screens come back tomorrow at
  07:00."), a code field that has the keyboard (digits only, 3+3, Enter or
  Unlock submits), and — separately — "Ask for more time". With no unlock
  code on the device it says so and offers only the ask.
- **Someone who sets their own limits** (the adult bracket, or a login the
  bundle marks `self_managed` — a self-managed member or a parent's own
  login) has nobody to ask: at their own limit, bedtime or hours the lock
  offers "Give me 15 more minutes" instead, usable once it has been up for
  60 s, three times a day (counted in the usage ledger). The 60 s run from
  the first time the lock stood in front of them for this stop — a fresh
  login or an agent restart keeps counting (`snooze_wait` in the freeze
  state) — and both locks count down to that moment on their own; pressed
  early, it says how long is left. The agent decides
  (`snooze` over the lock socket; a child's lock is refused), writes the
  15-minute override and files `screen_time_earned` with `via: "self"`.
- **Software rendering**: a cage that exits within 6 s is run once more on
  the CPU (`WLR_RENDERER=pixman`, `LIBGL_ALWAYS_SOFTWARE=1`). That is how the
  graphical lock comes up where wlroots won't use the GPU path — current
  wlroots refuses llvmpipe ("Software rendering detected") on virtual and
  driverless GPUs.
- **Text**: with no `cage`, a headless build, or a graphical lock that gave
  up or is slow, the agent draws a plain text lock itself (the same
  sentences, the ring with its tick) on **VT 14** — its own VT, because the
  lock unit resets, hangs up and deallocates VT 13 on every cage start and
  stop — and locks VT switching (`VT_LOCKSWITCH`, as `vlock -a`). While it
  holds the screen the kernel's console log level is 1 (emergencies only),
  so no kernel line is drawn over it; the level from before is kept in
  `/var/lib/openscreentime/printk-console-level` and put back when it lets
  go. A cage that
  gives up (its unit in `auto-restart` or `failed`) is noticed at once, so
  the text lock is on screen within about a second; a cage that is only
  slow gets 2.5 s, then the text lock shows while it keeps starting, and the
  graphical lock takes over when it says hello. That still includes Debian
  12: its wlroots 0.15 refuses a KMS device without PRIME import (seen on
  QEMU's standard VGA, bochs-drm) in the DRM backend, before any renderer is
  chosen, so the software retry fails the same way.
- **Switch user** (a shared computer): with a display manager and more than
  one login, the lock offers a quiet "Switch user" (the text lock: `S`),
  except during a whole-computer pause. The agent lets go of
  `VT_LOCKSWITCH` and brings up the login screen: a running greeter session
  is activated, else GDM's `CreateTransientDisplay`, else the freedesktop
  `DisplayManager` seat's `SwitchToGreeter` (LightDM, SDDM). The stopped
  person stays stopped behind it (an inactive session counts no time). A
  stop holds for a desktop behind someone else's too: when a person's time
  runs out (an override ends, bedtime starts) while another person has the
  screen, their apps are frozen at the next tick — no countdown, nothing on
  screen changes — and their session meets the lock when it comes back. The
  lock stands in front of stopped people only: someone else's session keeps
  the screen for as long as they like; a login screen keeps it for 90 s,
  then the stopped person's lock (with its "Switch user") comes back; and a
  stopped session coming on screen — switched to, logged in to, or picked
  at the login screen (GDM re-authenticates through the session's own worker
  and keyring, which the freeze leaves running) — meets the lock first,
  within about a second (the VT watch wakes on the kernel's `POLLPRI` on
  `/sys/class/tty/tty0/active`).
- **A stopped session that ends** (a log-out, `loginctl terminate-user`, a
  crash) leaves no lock behind for nobody: within about a second (the VT
  watch also wakes when logind's session files in `/run/systemd/sessions`
  change) the lock lets go of `VT_LOCKSWITCH`, hands the screen to the login
  screen (the same ways "Switch user" does; the VT from before where there
  is no display manager) and stops once the login screen has the screen.
  The person stays stopped and meets the lock at their next login.
- **Logging in to a stop**: the lock is in front within about a second,
  while the desktop is still starting; it finishes starting behind the lock
  and nothing of it is frozen until it is a minute old.
- **The way back**: the lock records whether the person's own desktop lock
  was up when it went up (logind `LockedHint`). If it was open, for a few
  seconds after switching back the agent takes a desktop lock that appears
  off again (`loginctl unlock-session`) — a code or a parent's time doesn't
  end at a second password prompt. A desktop its owner had locked stays
  locked. And GNOME 43's mutter, when its session lost the screen while it
  was still starting, comes back with the GPU but not one input device: a
  desktop that draws and can't be used. So for 3 s after switching back the
  agent watches the person's processes; if they hold the GPU and no input
  device, it switches away and back once more, which makes the compositor
  take them (logged: "…came back without its keyboard and mouse").

Codes are checked by the agent (root), never by the lock: the graphical lock
sends them over `/run/openscreentime/lock.sock`, which answers only uid
`ost-lock` (`SO_PEERCRED`); the text lock hands them over in-process. Both go
through `parentcode`, `via: lock_screen`.

Order of operations: **lock** = start the lock → switch to its VT → freeze
their apps. **Unlock** = thaw → switch back to their session (and see it
took its keyboard and mouse back) → stop the lock. The agent reconciles the lock after every
tick, command, lock request and VT change, so every thaw path (a code, a
console Resume / grant, midnight, bedtime's end, a window opening,
`ost unlock`) takes it down, and switching or logging in to a stopped
session brings it up at once. The lock on screen is recorded in
`freeze_state.json`; a restarted agent adopts it (and the kernel's frozen
set) instead of forgetting it. If no lock can be shown, nobody is frozen
behind a blank screen — the console gets `lock_screen_unavailable`.
Needs virtual terminals (`CONFIG_VT`), which every desktop distro has.

Warnings: the per-user companion (`ost tray`) announces every stop — limit,
bedtime, end of allowed hours, a scheduled pause — at 15, 5 and 1 minute,
from `stop_at`/`reason`/`pause_at`/`freeze_in_secs` in the status snapshot;
the last minute is one critical notification updated in place. Actions: "Ask
for more time", "Open OpenScreenTime". It works without a tray host (GNOME).
Someone with no desktop hears the same words on their own terminals only.
The time a warning names ("ends at 23:38") is the time the lock comes, to the
minute: a limit stop is announced only while the minutes are really being
used (an idle person's stop slides later every tick, so at login, before any
input, there is no true time to say yet); a stop that moves to another
minute afterwards is announced again with its real time; and the agent runs
a tick of its own at a stop due before the next regular one, so the lock
isn't up to 10 s late. Billing carries the fraction of a second a tick
doesn't bill.

### Unlock code

`client/src/parentcode.rs`. The per-device **TOTP secret** is held by the
server and this agent, nobody else: the server mints it when the device is
added, the agent gets it in the policy bundle (`parent_code.totp_secret`,
cached root-only in `policy_bundle.json`), and the parent reads the *current*
6-digit **unlock code** off the console (a step-up gated read). Verification
is **offline** — RFC 6238, SHA1, 6 digits, 30 s, ±1 step — with a persisted
last-accepted counter (a code is single-use) and a wrong-attempt lockout (5
wrong → 60 s, doubling, max 15 min; `/var/lib/openscreentime/parent_code.json`).

**Recovery codes** (8 digits, one-time) come from the same console page and
arrive in the bundle as `parent_code.recovery_codes: [{id, mac}]` with
`mac = hex HMAC-SHA256(decoded secret, the 8 digits)` — the code itself is
never sent. A match is `Verdict::Recovery(id)`: the id is remembered in
`parent_code.json` (single-use even offline) and reported as
`parent_code_backup_used { recovery_id }` so the server retires it too. A
profile-level **backup code** (`parent_pin_hash`, argon2) still opens the
door, reported as `parent_code_backup_used` without an id. Every attempt emits
`parent_code_ok` / `parent_code_failed` / `parent_code_backup_used` with
payload `{ via: lock_screen|unlock|pam, user, detail[, recovery_id] }`.

Where it is asked for: the lock's code field, `ost unlock`, and `sudo` on a
managed machine (below). Secrets never leave the agent: the lock only
forwards what was typed.

### Parent sudo (PAM)

`install-service` writes `/etc/pam.d/openscreentime-parent` and a sudoers
drop-in, `/etc/sudoers.d/10-openscreentime`, that the agent rewrites on every
policy apply with the current **managed** OS users (any profile kind except
`adult`/`default`): `Defaults:<users> pam_service=openscreentime-parent,
timestamp_timeout=0` and `<users> ALL=(ALL:ALL) ALL`. Effect: a child typing
`sudo` is asked for the unlock code (verified by the hidden
`pam-auth` helper via `pam_exec expose_authtok`), so the parent can
administer the machine without a local password and the child cannot.
Adults' sudo is untouched. The drop-in is staged under a dot-name (sudo
ignores those), validated with `visudo -c -f`, and only then renamed into
place — a broken sudoers.d file would lock everyone out, so one is never
left behind. `ost uninstall` removes both files.

### App & category blocks

`policy.blocks` (`openscreentime_policy::catalog`) is expanded on the device:
the union of every user's blocked domains goes into the dnsmasq ruleset as
`address=/domain/0.0.0.0` + `address=/domain/::` sinkholes (subdomains
included), and `client/src/enforce/apps.rs` walks `/proc` every tick and
SIGKILLs processes whose `comm` is on a blocked app's process list *and*
whose uid belongs to the user that blocks it — never root, never another
user, exact `comm` matches only. One `app_blocked` event (info, `{ app,
comm, user }`) per user/app/day.

A blocked site is only the browser's "Unable to connect", so the agent says
why: when the query log shows a name answered with the blocked address
(`0.0.0.0` / `::`) **by this computer's own rules** (`config` — the
blocklist, the catalog's app and category blocks, focus-hours sites, Tor)
twice within 5 minutes, the person whose time is counting gets one
notification naming the rule that blocks it, "example.org is blocked on this
computer" (for `www.example.org` too) — once per site per day, one a minute
at most, never to someone stopped. What the family resolver filters upstream
is not named (Firefox's background `ads.mozilla.org` is not "mozilla.org is
blocked"), and neither is anything the computer looks up on its own
(`attrib::OS_LOOKUPS`). Resolver traffic has no user, so it is said as a
fact about the computer. There is no block page of our own: an HTTPS site can't
be answered for without a certificate warning.

Every minute the agent checks the block is real: `getent hosts
selftest.openscreentime.internal` — a name every ruleset answers as blocked
and nobody visits — must come back `0.0.0.0` / `::`. A real address or no
answer twice in a row is a `sinkhole_ineffective` incident. It never looks
up a real blocked site (that lookup once showed up as the child's browsing),
and neither the sites list nor the notice ever counts the self-check's name.

### Presence: the `state` frame

On the WS bus the agent sends `{"type":"state","state":{…}}` on connect,
whenever it changes, and at least every 60 s: `locked` (a device lock is
intended **and** the kernel freezer confirms every present managed user's
stop holds — everything their stop freezes reads back frozen from
`cgroup.freeze`, never the agent's intention),
`lock_intent`, `frozen_users`, `enforcing` (policy applied with no standing
gaps), `gaps` (kinds), `agent_version`, `active_users`, and `overrides`
(`{ login: end (UTC) }`, the parent overrides running now — the console
counts time left from the same inputs the computer does). The usage
`heartbeat` frame goes every 30 s. Reconnects use jittered exponential
backoff (1 → 60 s); while the WS is down the agent polls `/agent/heartbeat`
every 30 s for one-minute rounds, then tries the bus again. Usage is written
to the local ledger every tick regardless, so nothing is lost offline.

## Self-update

`client/src/update.rs`. First check ~2 minutes after `run` starts (catches
up quickly after an offline stretch), then once every 24 hours.

**Trust model v1** (see `docs/CONTRACT-PROD.md`): the manifest's `sha256`,
fetched over TLS from the enrolled server, is the only integrity check. A
compromised server can already push arbitrary root commands to the fleet,
so this doesn't weaken the model — the hash mainly guards against truncated
downloads or a tampered cache. A v2 pinning an independent (e.g. minisign)
signing key is called out as future work.

Mechanics: `GET {server}/api/agent/latest` → a JSON manifest
(`version`, `build`, `artifacts: [{target, features, url, sha256}]`) → if
its build differs from this binary's (`OST_BUILD_ID`; without build ids, a
newer version — never a downgrade, never identical bytes) and an artifact
matches this build's variant (`x86_64-linux-musl`/`headless`, or
`x86_64-linux-gnu`/`desktop` for a `gui`/`tray` build), download it, check
the `sha256` of the exact bytes, stage it (`/usr/local/bin/.openscreentime.new`)
and **preflight** it (it must run `--version` here), keep the current binary
as `openscreentime.bak`, write an update-pending marker, swap atomically and
restart. The new build clears the marker once it has run for a minute; if it
crash-loops or stops ticking first, the watchdog unit restores `.bak` and
restarts — **automatic rollback** — and that build is skipped from then on.

Kill switches (any one disables it):

| Switch | Effect |
|---|---|
| `auto_update = false` in `agent.toml` | Disables self-update for this device. |
| `OST_NO_SELF_UPDATE=1` (env var on the service) | Runtime override, no config edit needed. |
| A non-x86_64 target, or no artifact of this build's variant in the manifest | Nothing to install — a desktop build is never swapped for the headless one. |
| Process is not literally `/usr/local/bin/openscreentime` | A `cargo run` dev build (or any binary run from elsewhere) never self-updates. |
| `--dry-run` | Logs what it would install and restart, does nothing. |

## Offline behavior

`client/src/runner.rs`. Screen time is unaffected by the network: the
enforcement tick runs on its own timer (usage counts at full rate, rules and
stops apply, the watchdog heartbeat stays fresh), the day rolls at local
midnight on the trusted clock with no server needed, and the person's use on
their other computers stays at its last reported value for the day. On top
of that, two independent thresholds, both fail-closed (the device stays
usable under its *existing* policy — self-update and offline handling never
black out all traffic):

1. **Grace period** (`OST_OFFLINE_GRACE_SECS`, default 900s / 15 min).
   Measured against the last successful WS message or poll/heartbeat
   (`Instant`-based, does not survive a reboot — a fresh process starts the
   clock at "now"). Past the grace window: emit one `network_offline`
   tamper event, and on every subsequent tick aggressively re-assert the
   last-known DNS/firewall/resolv-conf policy so nothing can drift open
   while the server is unreachable. A `network_online` event fires once
   contact resumes.
2. **Offline hard-lockdown** (`lockdown.offline_lockdown_days` in policy,
   0 = off; the device-wide threshold is the smallest non-zero value across
   all managed users). Measured against a wall-clock timestamp persisted to
   `/var/lib/openscreentime/last_contact` (throttled to at most one disk write per
   60s), so it correctly survives reboots — a device offline for days has
   almost certainly rebooted at least once. Once exceeded, every user is
   frozen immediately (no freeze grace, treated like an admin lock) behind
   the lock ("Stopped until this computer reaches the family server").
   **The parent code always unlocks** — a dead or
   unreachable server can never permanently brick the family's laptop. An
   `offline_hard_lockdown_lifted` event fires once contact resumes.

## Troubleshooting

- **Watch the agent live**: `journalctl -u openscreentime-agent.service -f`
  (add `-u openscreentime-watchdog.service` to see restart-on-stale-heartbeat
  triggers). Verbosity is controlled by `RUST_LOG` (standard
  `tracing-subscriber` `EnvFilter` syntax, e.g.
  `RUST_LOG=openscreentime=debug`); default is `openscreentime=info,info`.
- **Simulate without touching the host**: `sudo ost run
  --dry-run` (or any subcommand). Every enforcement action logs `WOULD RUN:
  ...` / `WOULD WRITE ...` instead of executing — safe even as non-root, and
  the only mode non-root enforcement subcommands are allowed to run in at
  all (`require_root_for_enforcement` refuses otherwise).
- **`NOT ENROLLED` from `status`**: no `/etc/openscreentime/agent.toml` — run
  `enroll` first.
- **Service won't start / immediately restarts**: check
  `journalctl -u openscreentime-agent.service`; a common cause is a missing
  `/etc/openscreentime/agent.toml` (enroll wasn't run before `install-service`,
  or the file was deleted) — `run` bails immediately with "not enrolled?".
- **Firewall/DNS looks wrong or "stuck open"**: check whether the nft table
  exists (`nft list table inet openscreentime`) — the agent repairs a missing
  table on the next tick, but only if it's actually still running; if the
  service is down, nothing is enforced (fail-open is possible only while
  the process itself is dead — this is what the watchdog timer exists to
  prevent).
- **`enforcement_degraded` events (warn)**: the policy was written but the
  host can't enforce all of it. The payload `kind` says which (and `kinds`
  lists every gap of that area):
  | `kind` | Meaning | Fix |
  |---|---|---|
  | `dns_resolver_missing` | the dnsmasq service isn't installed (NetworkManager's `dnsmasq-base` is only the program): websites aren't filtered (the computer keeps its own DNS; screen time works) | re-run the install one-liner (it installs dnsmasq + nftables), or install the dnsmasq package |
  | `dns_no_local_resolver` | dnsmasq is installed but won't start, so the allowlist filters nothing | `systemctl status dnsmasq` |
  | `dns_policy_not_loaded` | dnsmasq reads no directory our ruleset is in | re-run the install one-liner |
  | `dns_rules_not_written` | the ruleset couldn't be written; dnsmasq serves what it had | the agent's journal says why |
  | `dns_resolv_conf_not_pinned` | `/etc/resolv.conf` couldn't be pointed at 127.0.0.1: nothing filtered, still online | the agent's journal says why |
  | `dns_resolv_conf_not_a_file` | `/etc/resolv.conf` is systemd-resolved's/resolvconf's link, which the sandboxed agent can't replace | re-run the install one-liner (it makes it a file, and puts the link back on removal) |
  | `dns_safesearch_unavailable` | safe search is on but a front end (`forcesafesearch.google.com`, `restrict.youtube.com`, `strict.bing.com`, `safe.duckduckgo.com`) couldn't be looked up: that engine opens without safe search (never broken); websites are still filtered | nothing: it's looked up again every minute and on a network change; the journal says which |
  | `dns_resolv_conf_not_locked` | `chattr +i` isn't supported on that filesystem, so the pin is only re-asserted every 10s | use a filesystem that supports immutability for `/etc` |
  | `firewall_not_installed` | `nft` isn't installed: no firewall rules (screen time works) | re-run the install one-liner, or install nftables |
  | `firewall_not_applied` | `nft` refused the ruleset | the agent's journal says why |
  | `firewall_lockdown_not_applied` | the firewall is loaded without its anti-bypass rules, which `nft` refused | the agent's journal says why |
  | `vpn_not_applied` | the VPN profile couldn't be written | the agent's journal says why |

  One area (dns, firewall, vpn — one sentence in the console) is one
  incident: one event when its first gap appears, none while it stands — the
  `state` frame's `gaps` carries it, which is what the console's Family and
  Computers pages show — and a new one only after it went away and came
  back. What was reported is kept in `/var/lib/openscreentime/degraded.json`,
  so a restart or a reboot with the same gaps sends nothing. `warn`, not
  `critical`: a missing package is a setup to fix, not an alarm for a phone,
  and never a `tamper` event. The tamper loop's own findings
  (`resolv_conf_drift`, `nft_flush`, `nm_disconnect`, …) are likewise
  reported once per incident, not every 10 s.
- **Locked out and no server reachable**: `sudo ost unlock --minutes 60`
  (it asks for the unlock code — read it off the console, or use a recovery code)
  works fully offline against the cached bundle, as long as the agent has
  applied a policy at least once. If `policy_cache.json` doesn't exist yet,
  this path is unavailable — it fails with "no cached policy on this device".
  "too many wrong codes" means the lockout in `parent_code.json` is running
  (60 s, doubling); wait it out, it is not a bug.
- **Self-update never happens**: check `auto_update` in `agent.toml`,
  `OST_NO_SELF_UPDATE`, that the server's manifest has an artifact of this
  build's variant (desktop or headless), that
  `/var/lib/openscreentime/update-rejected.json` doesn't name the build (a
  rolled-back one is skipped), and that it's actually running from
  `/usr/local/bin/openscreentime` (`current_exe()` must match exactly).
- **Tray says "OpenScreenTime isn't running"**: `/run/openscreentime/status.json` is
  missing or unreadable — the root agent isn't up, or hasn't completed a
  tick yet since starting.
