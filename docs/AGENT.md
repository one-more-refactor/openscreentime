# The Sentinel Linux Agent

Reference for `sentinel-agent`, the Rust binary that runs on a managed Linux
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
curl -fsSL https://HOST/install.sh | sudo SENTINEL_TOKEN=xxx sh -s -- --server https://HOST
```

or with the token on the command line (`--token xxx` instead of the env
var — see the warning below):

```sh
curl -fsSL https://HOST/install.sh | sudo sh -s -- --server https://HOST --token xxx
```

`server/install.sh` (served at `GET /install.sh`) does, in order:

1. Validates args: requires `--server https://HOST` and a token
   (`SENTINEL_TOKEN` env or `--token`); refuses plain `http://` unless
   `--insecure-http` is passed (dev only); requires root and `x86_64`.
2. `GET {server}/api/agent/latest`, parses out the artifact whose
   `"features"` is `"headless"` (sed, not jq — the target may not have jq).
3. Downloads the binary to a temp file **in the same directory as the final
   target** (`/usr/local/bin/.sentinel-agent.download.$$`) so the final `mv`
   is an atomic rename on the same filesystem — a crash mid-download can
   never leave a truncated binary at `/usr/local/bin/sentinel-agent`.
4. Verifies `sha256sum` against the hash pinned in the manifest; refuses to
   install on mismatch.
5. `chmod 0755`, `mv -f` into place, then runs `sentinel-agent enroll
   --server ... --token ...` followed by `sentinel-agent install-service`.

Prefer the `SENTINEL_TOKEN=xxx` env form over `--token xxx`: the installer
warns you if you use `--token`, because it can linger in shell history and
was briefly visible in the process list (`ps`). The token is single-use
either way.

### Manual build from source (required for `gui` / `tray`)

The shipped binary is headless-only, so a desktop machine that wants the
full-screen lockout GUI or the tray companion must be built locally:

```sh
cd client
cargo build --release --features gui,tray
sudo install -m 0755 target/release/sentinel-agent /usr/local/bin/sentinel-agent
sudo sentinel-agent enroll --server https://HOST --token <ENROLL_TOKEN>
sudo sentinel-agent install-service
```

`install-service` also drops the per-user tray unit
(`/etc/systemd/user/sentinel-tray.service`) regardless of which features the
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
| `enroll` | `--server <URL>` `--token <TOKEN>` | Reports hostname, OS users, and agent version to the server; receives `device_id` + `device_token`; writes `/etc/sentinel/agent.toml` (root-owned `0600`). |
| `run` | — | The main loop: connects the WS command bus (falls back to heartbeat polling), pulls and enforces policy, dispatches server commands, streams events. Requires root unless `--dry-run`. Requires a prior `enroll`. |
| `install-service` | — | Copies the running binary to `/usr/local/bin/sentinel-agent`, writes the hardened systemd unit + watchdog timer + polkit rule, writes the (best-effort) tray user unit, then `daemon-reload` + enables/starts `sentinel-agent.service` and `sentinel-watchdog.timer`. Requires root. |
| `status` | — | Prints enrollment state (server, device ID, tamper level, poll interval), whether the process is root, and `systemctl is-active sentinel-agent.service`. Safe non-root. |
| `tray` | — | *(feature `tray` only)* Per-user tray companion — see [Build features matrix](#build-features-matrix). Runs as the desktop user, never root. |
| `unlock` | `--pin <PIN>` `--minutes <N>` (default 60) | Parent-PIN recovery: verifies the PIN against the cached policy, offline, then suspends enforcement (removes the nft table, un-pins `resolv.conf`, un-freezes every login user) for `N` minutes. Requires root. See [Lockout](#lockout-gui--wall-fallback). |

Two subcommands are intentionally hidden — not in `--help`, not real
`clap::Subcommand` variants, invoked only by the agent itself:

| Hidden subcommand | Who spawns it | Purpose |
|---|---|---|
| `__lockout <base64 LockSpec>` | The running agent, detached, when presenting a lockout overlay on a `gui` build | Runs the blocking `eframe`/`egui` event loop in a subprocess so it never stalls the enforcement tick. Fails with an error if the binary wasn't built `--features gui`. |
| `__resume-enforcement <secs>` | `sentinel-agent unlock`, detached | Sleeps out the suspend window from `unlock`, then re-applies the cached policy once and exits. |

## Files on disk

| Path | Owner : mode | Written by | Purpose |
|---|---|---|---|
| `/usr/local/bin/sentinel-agent` | root : 0755 | `install.sh` / `install-service` / self-update | The managed binary. `ExecStart` target for the systemd unit. |
| `/usr/local/bin/sentinel-agent.bak` | root : 0755 | self-update | Copy of the previous binary, kept for manual rollback after every self-update. |
| `/usr/local/bin/.sentinel-agent.new` | root : 0755 | self-update (transient) | Staging path for a downloaded update; renamed over the install path once verified. |
| `/usr/local/bin/.sentinel-agent.download.$$` | root : — | `install.sh` (transient) | Staging path for the initial download; renamed atomically into place, cleaned up by a trap on any exit. |
| `/etc/sentinel/agent.toml` | root : **0600** | `enroll` | Persisted identity: `server_url`, `device_id`, `device_token`, `poll_interval_secs`, `tamper_level`, `auto_update`. See [Config fields](#config-fields). |
| `/etc/sentinel/policy_cache.json` | root : **0600** | `run` (after every applied policy bundle) | Last-applied effective `Policy`, JSON. Not read by enforcement itself (that's in-memory); exists only so `unlock` can verify the parent PIN and know what to tear down without a live agent process. |
| `/etc/sentinel/dnsmasq.d/sentinel.conf` | root : default | `run` (DNS enforcement) | Rendered dnsmasq ruleset realizing the DNS policy. |
| `/etc/resolv.conf` | root : default, **immutable (`chattr +i`)** | `run` (DNS enforcement) | Pinned to `nameserver 127.0.0.1`; the immutable bit stops a managed user from repointing it. Re-asserted every tick if it drifts. |
| `/etc/polkit-1/rules.d/49-sentinel.rules` | root : default | `install-service` / `run` (bootstrap and on `set_tamper_level`) | Denies non-root power-off/reboot/suspend; at tamper level 3 also denies `systemctl stop/disable/mask` of the unit. `sentinel-admin` and `root` always retain access. |
| `/etc/systemd/logind.conf.d/50-sentinel.conf` | root : default | `run` (tamper level 3 only) | `ReserveVT=0` / `KillUserProcesses=yes` drop-in — disables TTY/VT switching for managed sessions. |
| `/run/sentinel/heartbeat` | root : default | `run` (every tick) / `install-service` | mtime = liveness signal for `sentinel-watchdog.timer`. |
| `/run/sentinel/status.json` | root : world-readable (0755 dir) | `run` (every tick, atomic rename via `.tmp`) | Transparency snapshot for the tray: connection state, device-lock/offline-lockdown flags, whether a remote shell is open, and per-user used/remaining minutes, frozen state, freeze countdown. |
| `/run/sentinel/unlock_pin.<user>` | dropped by a companion tool acting for the parent | consumed by `run` every tick | A **PIN attempt** (plaintext), single-use — read once and deleted regardless of outcome. Verified directly against `parent_pin_hash`; grants `PIN_OVERRIDE_GRANT_MIN` (30) minutes on match. This is the headless (no-GUI) parent-PIN override path. |
| `/run/sentinel/unlock_grant.<user>` | root-only dir (0755) — no managed user can write here | written by the `__lockout` GUI subprocess on a verified dismissal; consumed by `run` every tick | An **already-verified** unlock, trusted at face value (safe only because `/run/sentinel` is root-owned). Value is minutes granted, clamped to 1–240: 30 for a parent-PIN dismiss, 5 for a solved math challenge, single-use. |
| `/var/lib/sentinel/last_contact` | root : default | `run` (throttled, at most once/60s, on successful server contact) | RFC3339 wall-clock timestamp of the last successful server contact. Survives reboots — it's what the days-scale offline hard-lockdown timer is measured against (an `Instant` can't survive a reboot). |

### Config fields

`/etc/sentinel/agent.toml`, root-owned `0600`, written by `enroll`:

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
| `sentinel-agent.service` | `/etc/systemd/system/` | The agent itself: `ExecStart=/usr/local/bin/sentinel-agent run`. |
| `sentinel-watchdog.service` + `.timer` | `/etc/systemd/system/` | Oneshot check every 30s (after a 60s boot delay): if `/run/sentinel/heartbeat` is missing or older than 90s, `systemctl restart sentinel-agent.service`. |
| `sentinel-tray.service` | `/etc/systemd/user/` | Per-user unit, **not auto-enabled** — a desktop user opts in with `systemctl --user enable --now sentinel-tray`. Only does anything useful on a `--features tray` build. |

`sentinel-agent.service` hardening highlights (tamper level 1 baseline, see
`docs/TAMPER.md`):

- `Restart=always`, `RestartSec=1`, `StartLimitIntervalSec=0` — never gives
  up restarting.
- `OOMScoreAdjust=-1000` — survives OOM pressure; the agent must not be the
  first thing killed.
- `ProtectSystem=strict` with an explicit `ReadWritePaths=` carve-out for
  `/etc/sentinel /var/lib/sentinel /run/sentinel /etc/resolv.conf
  /etc/polkit-1/rules.d /etc/systemd/logind.conf.d` — everything else is
  read-only.
- `ProtectHome=false` **intentionally** — the agent must watch user
  sessions/cgroups.
- `NoNewPrivileges=false` **intentionally** — enforcement shells out to
  `nft`, `resolvectl`, `chattr`.
- A commented-out `WatchdogSec=30` line for `sd_notify`-based watchdogging,
  as an alternative to the separate `sentinel-watchdog.timer`.

The polkit rule (`49-sentinel.rules`) denies non-root
`power-off`/`reboot`/`suspend`; at tamper level 3 it additionally denies
`systemctl stop/disable/mask` on `sentinel-agent.service`. `sentinel-admin`
and `root` always retain full access — that's the permanent recovery path
at every tamper level.

## Build features matrix

Set via `cargo build --release --features <list>` (comma-separated). All are
additive; `default = []`.

| Feature | Adds | What you get |
|---|---|---|
| *(none — headless, what the server ships)* | — | Full enforcement (DNS, firewall, screen time, tamper hardening, self-update). Lockout/nudge screens render as a `wall`-broadcast text overlay (see `render_ascii`) instead of a graphical window. No `tray` subcommand. |
| `gui` | `eframe`/`egui` | The `__lockout` subprocess renders a real fullscreen window (black bg, monospace, accent-red CTA) instead of falling back to `wall`. Enables the parent-PIN / math-challenge typed-input box and the verified-unlock grant flow (`unlock_grant.<user>`). Self-update refuses to run on a `gui` build. |
| `tray` | `ksni` (StatusNotifierItem) + `notify-rust` | The `tray` subcommand: a per-user, non-root system tray icon + desktop notifications reading `/run/sentinel/status.json`. Self-update refuses to run on a `tray` build. |

Both `gui` and `tray` can be combined (`--features gui,tray`) for a full
desktop build. The `install-service` unit files are the same regardless of
features — the tray *user* unit is always written, it's just inert without
`tray`.

## How enforcement works

### DNS

`client/src/enforce/dns.rs`. Renders a dnsmasq config
(`/etc/sentinel/dnsmasq.d/sentinel.conf`) and restarts the local `dnsmasq`
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

`client/src/enforce/firewall.rs`. A single `inet sentinel` nftables table,
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
`/run/sentinel/unlock_grant.<user>`. If spawning fails, or the build has no
`gui` feature, it falls back to `wall -n` broadcasting the message to every
TTY plus logging it — nothing is ever silently dropped.

Challenge types: `Math` (a×b, solved answer grants 5 min on `gui`), `Wait`
(cooldown, no typed input, grants nothing early), `ParentPin` (the
configured PIN, grants 30 min), `None` (a nudge, no gate). The parent PIN,
when configured, is always accepted as a master escape regardless of the
active challenge. The headless file-drop override
(`/run/sentinel/unlock_pin.<user>`) intentionally does **not** route
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
(`/usr/local/bin/.sentinel-agent.new`), `chmod 0755`, copy the *current*
binary to `sentinel-agent.bak` (manual rollback — there is no automatic
rollback), atomically rename the staged file over
`/usr/local/bin/sentinel-agent`, POST an `agent_updated` tamper event, then
`systemctl restart sentinel-agent.service`.

Kill switches (any one disables it):

| Switch | Effect |
|---|---|
| `auto_update = false` in `agent.toml` | Disables self-update for this device. |
| `SENTINEL_NO_SELF_UPDATE=1` (env var on the service) | Runtime override, no config edit needed. |
| Built with `--features gui` or `--features tray`, or a non-x86_64 target | Never enabled — the server only ships a headless x86_64 artifact, and a feature-richer local build must never be silently downgraded to it. |
| Process is not literally `/usr/local/bin/sentinel-agent` | A `cargo run` dev build (or any binary run from elsewhere) never self-updates. |
| `--dry-run` | Logs what it would install and restart, does nothing. |

## Offline behavior

`client/src/runner.rs`. Two independent thresholds, both fail-closed (the
device stays usable under its *existing* policy — self-update and offline
handling never black out all traffic):

1. **Grace period** (`SENTINEL_OFFLINE_GRACE_SECS`, default 900s / 15 min).
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
   `/var/lib/sentinel/last_contact` (throttled to at most one disk write per
   60s), so it correctly survives reboots — a device offline for days has
   almost certainly rebooted at least once. Once exceeded, every user is
   frozen immediately (no freeze grace, treated like an admin lock) with an
   `OFFLINE TOO LONG` overlay. **The parent PIN always unlocks** — a dead or
   unreachable server can never permanently brick the family's laptop. An
   `offline_hard_lockdown_lifted` event fires once contact resumes.

## Troubleshooting

- **Watch the agent live**: `journalctl -u sentinel-agent.service -f`
  (add `-u sentinel-watchdog.service` to see restart-on-stale-heartbeat
  triggers). Verbosity is controlled by `RUST_LOG` (standard
  `tracing-subscriber` `EnvFilter` syntax, e.g.
  `RUST_LOG=sentinel_agent=debug`); default is `sentinel_agent=info,info`.
- **Simulate without touching the host**: `sudo sentinel-agent run
  --dry-run` (or any subcommand). Every enforcement action logs `WOULD RUN:
  ...` / `WOULD WRITE ...` instead of executing — safe even as non-root, and
  the only mode non-root enforcement subcommands are allowed to run in at
  all (`require_root_for_enforcement` refuses otherwise).
- **`NOT ENROLLED` from `status`**: no `/etc/sentinel/agent.toml` — run
  `enroll` first.
- **Service won't start / immediately restarts**: check
  `journalctl -u sentinel-agent.service`; a common cause is a missing
  `/etc/sentinel/agent.toml` (enroll wasn't run before `install-service`,
  or the file was deleted) — `run` bails immediately with "not enrolled?".
- **Firewall/DNS looks wrong or "stuck open"**: check whether the nft table
  exists (`nft list table inet sentinel`) — the agent repairs a missing
  table on the next tick, but only if it's actually still running; if the
  service is down, nothing is enforced (fail-open is possible only while
  the process itself is dead — this is what the watchdog timer exists to
  prevent).
- **Locked out and no server reachable**: `sudo sentinel-agent unlock --pin
  <PARENT_PIN> --minutes 60` works fully offline (verifies against the
  cached policy at `/etc/sentinel/policy_cache.json`) as long as the agent
  has applied a policy at least once and a parent PIN is configured. If
  `policy_cache.json` doesn't exist yet, this path is unavailable — it
  fails with "no cached policy on this device".
- **Self-update never happens**: check `auto_update` in `agent.toml`,
  `SENTINEL_NO_SELF_UPDATE`, that the binary is a plain headless build
  (`gui`/`tray` builds never self-update), and that it's actually running
  from `/usr/local/bin/sentinel-agent` (`current_exe()` must match exactly).
- **Tray shows "AGENT NOT RUNNING"**: `/run/sentinel/status.json` is
  missing or unreadable — the root agent isn't up, or hasn't completed a
  tick yet since starting.
