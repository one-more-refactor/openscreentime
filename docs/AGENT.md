# The OpenScreenTime Linux Agent

Reference for `openscreentime`, the Rust binary that runs on a managed Linux
machine: what it installs, what it writes to disk, how it enforces policy,
and how to debug it. For the server-side deploy, see `docs/DEPLOY.md`; for
the tamper threat model, see `docs/TAMPER.md`.

The agent is a single static binary. The build the server ships (and that
`install.sh` downloads) is **headless x86_64** — no GUI, no tray. Where a
capability requires `--features gui` or `--features tray`, this doc says so
explicitly.

## Install & enroll

### One-liner (headless, x86_64 — what the server serves)

```sh
curl -fsSL https://HOST/install.sh | sudo OST_TOKEN=xxx sh -s -- --server https://HOST
```

or with the token on the command line (`--token xxx` instead of the env
var — see the warning below):

```sh
curl -fsSL https://HOST/install.sh | sudo sh -s -- --server https://HOST --token xxx
```

`server/install.sh` (served at `GET /install.sh`) does, in order:

1. Validates args: requires `--server https://HOST` and a token
   (`OST_TOKEN` env or `--token`); refuses plain `http://` unless
   `--insecure-http` is passed (dev only); requires root and `x86_64`.
2. `GET {server}/api/agent/latest`, parses out the artifact whose
   `"features"` is `"headless"` (sed, not jq — the target may not have jq).
3. Downloads the binary to a temp file **in the same directory as the final
   target** (`/usr/local/bin/.openscreentime.download.$$`) so the final `mv`
   is an atomic rename on the same filesystem — a crash mid-download can
   never leave a truncated binary at `/usr/local/bin/openscreentime`.
4. Verifies `sha256sum` against the hash pinned in the manifest; refuses to
   install on mismatch.
5. `chmod 0755`, `mv -f` into place, then runs `ost enroll
   --server ... --token ...` followed by `ost install-service`.

Prefer the `OST_TOKEN=xxx` env form over `--token xxx`: the installer
warns you if you use `--token`, because it can linger in shell history and
was briefly visible in the process list (`ps`). The token is single-use
either way.

### Manual build from source (required for `gui` / `tray`)

The shipped binary is headless-only, so a desktop machine that wants the
full-screen lockout GUI or the tray companion must be built locally:

```sh
cd client
cargo build --release --features gui,tray
sudo install -m 0755 target/release/ost /usr/local/bin/openscreentime
sudo ost enroll --server https://HOST --token <ENROLL_TOKEN>
sudo ost install-service
```

`install-service` also drops the per-user tray unit
(`/etc/systemd/user/openscreentime-tray.service`) regardless of which features the
binary was built with — it's a no-op unless you built with `--features
tray` and a desktop user opts in (see [systemd units](#systemd-units)).

Self-update (below) refuses to touch a `gui`/`tray` build — it only ever
manages the plain headless binary the server ships.

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
| `enroll` | `--server <URL>` `--token <TOKEN>` | Reports hostname, OS users, and agent version to the server; receives `device_id` + `device_token`; writes `/etc/openscreentime/agent.toml` (root-owned `0600`). |
| `run` | — | The main loop: connects the WS command bus (falls back to heartbeat polling), pulls and enforces policy, dispatches server commands, streams events. Requires root unless `--dry-run`. Requires a prior `enroll`. |
| `install-service` | — | Copies the running binary to `/usr/local/bin/openscreentime`, writes the hardened systemd unit + watchdog timer + polkit rule, writes the (best-effort) tray user unit, then `daemon-reload` + enables/starts `openscreentime-agent.service` and `openscreentime-watchdog.timer`. Requires root. |
| `status` | `--json` | Prints enrollment state (server, device ID, tamper level, poll interval), whether the process is root, and `systemctl is-active openscreentime-agent.service`. Safe non-root. |
| `time` | `--json` | How much screen time the calling user has left today. Reads the per-user status snapshot the agent writes each tick. Safe non-root, no display needed. |
| `ask` | `--json` | Sends a time request to a parent, from the keyboard. Writes a marker inside the caller's own `/run/user/<uid>/openscreentime/` — which is what proves the request came from them. Safe non-root. |
| `login` | `--print-url` `--json` | Opens the console in a browser, already signed in, using this computer's enrollment as proof. See [Autologin](#autologin-ost-login). |
| `pair` | `--server <URL>` `--token <TOKEN>` | Stores a scoped **parent access token** (minted in the web console → Settings → Parent access) at `~/.config/openscreentime/parent.toml` (`0600`). Enables the tray's parent mode. Runs as the desktop user, never root. |
| `tray` | — | *(feature `tray` only)* Per-user tray companion — see [Build features matrix](#build-features-matrix). With a `pair`ed token it also shows and approves time requests. Runs as the desktop user, never root. |
| `unlock` | `--pin <PIN>` `--minutes <N>` (default 60) | Parent-PIN recovery: verifies the PIN against the cached policy, offline, then suspends enforcement (removes the nft table, un-pins `resolv.conf`, un-freezes every login user) for `N` minutes. Requires root. See [Lockout](#lockout-gui--wall-fallback). |

Two subcommands are intentionally hidden — not in `--help`, not real
`clap::Subcommand` variants, invoked only by the agent itself:

| Hidden subcommand | Who spawns it | Purpose |
|---|---|---|
| `__lockout <base64 LockSpec>` | The running agent, detached, when presenting a lockout overlay on a `gui` build | Runs the blocking `eframe`/`egui` event loop in a subprocess so it never stalls the enforcement tick. Fails with an error if the binary wasn't built `--features gui`. |
| `__intro` | The tray, detached, on first run (`gui`+`tray` build) | Shows the skippable first-run child intro cards, then writes `intro_seen` so it never shows again. Fails with an error if the binary wasn't built `--features gui`. |
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
  "left_minutes": 28,       // null when "limited" is false
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

What a voucher session can and cannot do:

* It can **read** everything the account can read.
* It **cannot change anything** without a second factor. The session never
  starts with a step-up grant — possession of the laptop is not possession of
  the phone — so every mutation still answers `428 step_up_required`.
* It is **single-use** and expires after two minutes.

### Environment

| Variable | Purpose |
|---|---|
| `OST_CONFIG` | Read/write the agent config at this path instead of `/etc/openscreentime/agent.toml`. For development and tests — it lets `enroll`, `status` and `login` be exercised without root. The systemd unit sets no such variable, so it cannot redirect what the real root agent reads. |
| `OST_NO_SELF_UPDATE=1` | Disable the daily self-update at runtime. |

## Files on disk

| Path | Owner : mode | Written by | Purpose |
|---|---|---|---|
| `/usr/local/bin/openscreentime` | root : 0755 | `install.sh` / `install-service` / self-update | The managed binary. `ExecStart` target for the systemd unit. |
| `/usr/local/bin/openscreentime.bak` | root : 0755 | self-update | Copy of the previous binary, kept for manual rollback after every self-update. |
| `/usr/local/bin/.openscreentime.new` | root : 0755 | self-update (transient) | Staging path for a downloaded update; renamed over the install path once verified. |
| `/usr/local/bin/.openscreentime.download.$$` | root : — | `install.sh` (transient) | Staging path for the initial download; renamed atomically into place, cleaned up by a trap on any exit. |
| `/etc/openscreentime/agent.toml` | root : **0600** | `enroll` | Persisted identity: `server_url`, `device_id`, `device_token`, `poll_interval_secs`, `tamper_level`, `auto_update`. See [Config fields](#config-fields). |
| `/etc/openscreentime/policy_cache.json` | root : **0600** | `run` (after every applied policy bundle) | Last-applied effective `Policy`, JSON. Not read by enforcement itself (that's in-memory); exists only so `unlock` can verify the parent PIN and know what to tear down without a live agent process. |
| `/etc/openscreentime/dnsmasq.d/openscreentime.conf` | root : default | `run` (DNS enforcement) | Rendered dnsmasq ruleset realizing the DNS policy. |
| `/etc/resolv.conf` | root : default, **immutable (`chattr +i`)** | `run` (DNS enforcement) | Pinned to `nameserver 127.0.0.1`; the immutable bit stops a managed user from repointing it. Re-asserted every tick if it drifts. |
| `/etc/wireguard/openscreentime.conf` | root : **0600** | `run` (VPN enforcement) | The device's WireGuard client config, verbatim as uploaded in the console (it contains the private key — hence 0600, and dry-run logs withhold its contents). Present only while a `wireguard` profile is set; runs as `wg-quick@openscreentime`. |
| `/etc/openvpn/client/openscreentime.conf` | root : **0600** | `run` (VPN enforcement) | Same for an OpenVPN profile; runs as `openvpn-client@openscreentime`. |
| `/etc/polkit-1/rules.d/49-openscreentime.rules` | root : default | `install-service` / `run` (bootstrap and on `set_tamper_level`) | Denies non-root power-off/reboot/suspend; at tamper level 3 also denies `systemctl stop/disable/mask` of the unit. `ost-admin` and `root` always retain access. |
| `/etc/systemd/logind.conf.d/50-openscreentime.conf` | root : default | `run` (tamper level 3 only) | `ReserveVT=0` / `KillUserProcesses=yes` drop-in — disables TTY/VT switching for managed sessions. |
| `/run/openscreentime/heartbeat` | root : default | `run` (every tick) / `install-service` | mtime = liveness signal for `openscreentime-watchdog.timer`. |
| `/run/openscreentime/status.json` | root : world-readable (0755 dir) | `run` (every tick, atomic rename via `.tmp`) | Transparency snapshot for the tray: connection state, device-lock / offline-lockdown / tamper-lockdown flags, per-user used/remaining minutes, frozen state, freeze countdown, and a short queue of agent-published notifications (id, title, body, urgency, target user) for the tray to deliver as desktop notifications. |
| `/run/openscreentime/unlock_pin.<user>` | dropped by a companion tool acting for the parent | consumed by `run` every tick | A **PIN attempt** (plaintext), single-use — read once and deleted regardless of outcome. Verified directly against `parent_pin_hash`; grants `PIN_OVERRIDE_GRANT_MIN` (30) minutes on match. This is the headless (no-GUI) parent-PIN override path. |
| `/run/openscreentime/unlock_grant.<user>` | root-only dir (0755) — no managed user can write here | written by the `__lockout` GUI subprocess on a verified dismissal; consumed by `run` every tick | An **already-verified** unlock, trusted at face value (safe only because `/run/openscreentime` is root-owned). Value is minutes granted, clamped to 1–240: 30 for a parent-PIN dismiss, 5 for a solved math challenge, single-use. |
| `/var/lib/openscreentime/last_contact` | root : default | `run` (throttled, at most once/60s, on successful server contact) | RFC3339 wall-clock timestamp of the last successful server contact. Survives reboots — it's what the days-scale offline hard-lockdown timer is measured against (an `Instant` can't survive a reboot). |
| `/var/lib/openscreentime/usage_ledger.json` | root : default | `run` (every tick, and on `credit_time`; atomic rename via `.tmp`) | The day's per-user screen-time counters (used + earned seconds). Reloaded on startup so a restart resumes today's usage instead of granting a fresh budget. The day boundary is forward-only: a clock set backward keeps the accumulated usage rather than resetting it. |
| `~/.config/openscreentime/parent.toml` | the desktop user : `0600` | `pair` (writes) / `tray` (reads, parent mode) | A paired parent's server URL + scoped access token. Written by `ost pair`; read by the tray to enable parent mode. Not present unless the machine was paired. |
| `~/.config/openscreentime/intro_seen` | the desktop user : default | `__intro` (writes) / `tray` (checks) | Marker that the first-run child intro has been shown. Present = don't show it again. |
| `/run/user/<uid>/openscreentime/earn_request` | the desktop user : `0700` dir | written by the `tray` (REQUEST MORE TIME); consumed by `run` every tick | An on-demand "request more time" marker. The unprivileged tray can only write inside its own `/run/user/<uid>`, which only that user and root can touch — so the root agent trusts it as an authentic request from that user (a spoof-proof privilege bridge). Single-use: read once, deleted, filed as an earn-request. |

### Config fields

`/etc/openscreentime/agent.toml`, root-owned `0600`, written by `enroll`:

| Field | Default | Meaning |
|---|---|---|
| `server_url` | — | The enrolled server's base URL. |
| `device_id` / `device_token` | — | Issued by the server at enroll time. |
| `poll_interval_secs` | `30` | Heartbeat interval used by the polling fallback (when the WS bus is unavailable). |
| `tamper_level` | `1` | Persisted tamper ceiling; the effective level is `max(this, 3 if --tamper-max else 1)`, and can be raised further by a `set_tamper_level` command up to that ceiling. |
| `auto_update` | `true` | Daily self-update from the enrolled server. `false` disables it; see [Self-update](#self-update) for the other kill switches. |

## systemd units

Installed by `install-service` (source in `client/systemd/`):

| Unit | Path | Purpose |
|---|---|---|
| `openscreentime-agent.service` | `/etc/systemd/system/` | The agent itself: `ExecStart=/usr/local/bin/ost run`. |
| `openscreentime-watchdog.service` + `.timer` | `/etc/systemd/system/` | Oneshot check every 30s (after a 60s boot delay): if `/run/openscreentime/heartbeat` is missing or older than 90s, `systemctl restart openscreentime-agent.service`. |
| `openscreentime-tray.service` | `/etc/systemd/user/` | Per-user unit, **not auto-enabled** — a desktop user opts in with `systemctl --user enable --now openscreentime-tray`. Only does anything useful on a `--features tray` build. |

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

The polkit rule (`49-openscreentime.rules`) denies non-root
`power-off`/`reboot`/`suspend`; at tamper level 3 it additionally denies
`systemctl stop/disable/mask` on `openscreentime-agent.service`. `ost-admin`
and `root` always retain full access — that's the permanent recovery path
at every tamper level.

## Build features matrix

Set via `cargo build --release --features <list>` (comma-separated). All are
additive; `default = []`.

| Feature | Adds | What you get |
|---|---|---|
| *(none — headless, what the server ships)* | — | Full enforcement (DNS, firewall, screen time, tamper hardening, self-update). Lockout/nudge screens render as a `wall`-broadcast text overlay (see `render_ascii`) instead of a graphical window. No `tray` subcommand. |
| `gui` | `eframe`/`egui` | The `__lockout` subprocess renders a real fullscreen window (black bg, monospace, accent-red CTA) instead of falling back to `wall`. Enables the parent-PIN / math-challenge typed-input box and the verified-unlock grant flow (`unlock_grant.<user>`). Self-update refuses to run on a `gui` build. |
| `tray` | `ksni` (StatusNotifierItem) + `notify-rust` | The `tray` subcommand: a per-user, non-root system tray icon + desktop notifications reading `/run/openscreentime/status.json`. In **parent mode** (after `ost pair`) a background worker also polls `/api/parent/*` to show pending time requests + alerts and approve/deny them from the menu. Self-update refuses to run on a `tray` build. |

Both `gui` and `tray` can be combined (`--features gui,tray`) for a full
desktop build. The `install-service` unit files are the same regardless of
features — the tray *user* unit is always written, it's just inert without
`tray`.

## How enforcement works

### DNS

`client/src/enforce/dns.rs`. Renders a dnsmasq config
(`/etc/openscreentime/dnsmasq.d/openscreentime.conf`) and restarts the local `dnsmasq`
(falling back to `resolvectl flush-caches` if that's what's running
instead). Under `default_deny` (and not a `*` wildcard allowlist), only
allowlisted domains get a `server=/domain/upstream` forward line, and a
trailing `address=/#/` NXDOMAINs everything else. Under allow-all (or an
explicit `*` allowlist), every query forwards to the filtered `upstream`
instead — firewall ports and safe-search still apply. `block_tor` NXDOMAINs
`.onion` and `torproject.org`; `safe_search` rewrites the big search/video
providers via `cname=` redirects. `/etc/resolv.conf` is pinned to
`127.0.0.1` and set immutable (`chattr +i`); re-pinned every tick if it
drifts off the local resolver.

### Firewall

`client/src/enforce/firewall.rs`. A single `inet openscreentime` nftables table,
applied atomically via one `nft -f -` transaction (`add table` → `delete
table` → fresh rules) so a malformed policy aborts the whole load and
leaves the last-known-good table in place, never a fail-open gap.
Default-deny input/output/forward, with `established,related` and loopback
always accepted; output also always allows the DNS upstream and (if it's a
literal IP) the enrolled server. `NetworkLockdown` toggles add **drop**
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

`client/src/enforce/screentime.rs`. Active seat users come from `loginctl
list-sessions`; a session counts only if `Active=yes`, `Remote=no`, and
`IdleHint=no` (idle time — lid closed, away from keyboard — never burns the
budget; DEs that don't set the hint fall back to the old always-count
behavior). Usage accumulates in-memory per user, resetting at local
midnight; `earned` minutes (approved earn-time requests) extend the daily
budget. `evaluate()` checks bedtime first, then the allowed-hours schedule,
then the daily limit.

**Freeze grace**: the first tick a lock reason fires, the runner shows the
overlay (with an earn-time offer, auto-requested, if it's a daily-limit
run-out) and arms a 60s (`FREEZE_GRACE`) countdown — the cgroup freeze
itself only lands once that expires, so it never looks like a sudden kernel
hang. An **admin lock** (`lock` command, or offline hard-lockdown) skips
the grace and freezes immediately.

The freeze writes `1`/`0` to
`/sys/fs/cgroup/user.slice/user-<uid>.slice/cgroup.freeze`. If that write
fails: a `hard` freeze (admin lock) falls back to `loginctl
terminate-user`; a screen-time freeze (`hard=false`) never escalates to
terminating the session — unsaved work must never be destroyed over a time
limit, so it just logs and stays best-effort.

Verified unlocks (an overlay grant, or a headless parent-PIN file drop) are
consumed every tick for every managed user, including already-frozen ones,
so a parent standing at the machine can always get someone out. While an
unlock grace window is active, screen-time AND an admin device lock are
both suspended for that user (the parent always wins).

### Lockout GUI + wall fallback

`client/src/lockout.rs`. `present()` always renders a `LockSpec` (headline,
detail, challenge, optional big-number) as ASCII art (`render_ascii`) for
logging. On a `gui` build it additionally spawns the `__lockout` subprocess
detached (so the blocking `egui` event loop never stalls the tick), which
shows a real fullscreen window and, on a verified dismissal, writes
`/run/openscreentime/unlock_grant.<user>`. If spawning fails, or the build has no
`gui` feature, it falls back to `wall -n` broadcasting the message to every
TTY plus logging it — nothing is ever silently dropped.

Challenge types: `Math` (a×b, solved answer grants 5 min on `gui`), `Wait`
(cooldown, no typed input, grants nothing early), `ParentPin` (the
configured PIN, grants 30 min), `None` (a nudge, no gate). The parent PIN,
when configured, is always accepted as a master escape regardless of the
active challenge. The headless file-drop override
(`/run/openscreentime/unlock_pin.<user>`) intentionally does **not** route
through the generic `Challenge::verify` — it checks the PIN hash directly,
because `Challenge::None` verifies unconditionally and routing an override
through it would let a dropped file bypass even an admin lock with no PIN
configured.

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
(`version`, `artifacts: [{target, features, url, sha256}]`) → if
`version` parses as newer than the running `AGENT_VERSION` and an artifact
matches `target == "x86_64-linux-musl"` and `features == "headless"`,
download it, re-hash the downloaded bytes and compare to the manifest's
`sha256` (refuses on mismatch), write to a staging path
(`/usr/local/bin/.openscreentime.new`), `chmod 0755`, copy the *current*
binary to `openscreentime.bak` (manual rollback — there is no automatic
rollback), atomically rename the staged file over
`/usr/local/bin/openscreentime`, POST an `agent_updated` tamper event, then
`systemctl restart openscreentime-agent.service`.

Kill switches (any one disables it):

| Switch | Effect |
|---|---|
| `auto_update = false` in `agent.toml` | Disables self-update for this device. |
| `OST_NO_SELF_UPDATE=1` (env var on the service) | Runtime override, no config edit needed. |
| Built with `--features gui` or `--features tray`, or a non-x86_64 target | Never enabled — the server only ships a headless x86_64 artifact, and a feature-richer local build must never be silently downgraded to it. |
| Process is not literally `/usr/local/bin/openscreentime` | A `cargo run` dev build (or any binary run from elsewhere) never self-updates. |
| `--dry-run` | Logs what it would install and restart, does nothing. |

## Offline behavior

`client/src/runner.rs`. Two independent thresholds, both fail-closed (the
device stays usable under its *existing* policy — self-update and offline
handling never black out all traffic):

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
   frozen immediately (no freeze grace, treated like an admin lock) with an
   `OFFLINE TOO LONG` overlay. **The parent PIN always unlocks** — a dead or
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
- **`enforcement_degraded` events (critical)**: the policy was written but the
  host can't enforce all of it. The payload `kind` says which:
  | `kind` | Meaning | Fix |
  |---|---|---|
  | `dns_no_local_resolver` | dnsmasq isn't installed or won't start, so the allowlist filters nothing | `apt install dnsmasq` (or the distro equivalent) and check `systemctl status dnsmasq` |
  | `dns_resolv_conf_not_a_file` | `/etc/resolv.conf` was a symlink owned by `systemd-resolved`/`resolvconf`; the agent replaced it with a real file | `systemctl disable --now systemd-resolved`, or it fights the pin on every network change |
  | `dns_resolv_conf_not_locked` | `chattr +i` isn't supported on that filesystem, so the pin is only re-asserted every 10s | use a filesystem that supports immutability for `/etc` |

  These are the reason a distro whose `/etc/resolv.conf` is a
  systemd-resolved symlink (Ubuntu, Mint, Fedora) needs resolved disabled
  and dnsmasq installed *before* enrollment. On Debian and Arch, where
  NetworkManager writes a real file, none of them fire.
- **Locked out and no server reachable**: `sudo ost unlock --pin
  <PARENT_PIN> --minutes 60` works fully offline (verifies against the
  cached policy at `/etc/openscreentime/policy_cache.json`) as long as the agent
  has applied a policy at least once and a parent PIN is configured. If
  `policy_cache.json` doesn't exist yet, this path is unavailable — it
  fails with "no cached policy on this device".
- **Self-update never happens**: check `auto_update` in `agent.toml`,
  `OST_NO_SELF_UPDATE`, that the binary is a plain headless build
  (`gui`/`tray` builds never self-update), and that it's actually running
  from `/usr/local/bin/openscreentime` (`current_exe()` must match exactly).
- **Tray shows "AGENT NOT RUNNING"**: `/run/openscreentime/status.json` is
  missing or unreadable — the root agent isn't up, or hasn't completed a
  tick yet since starting.
