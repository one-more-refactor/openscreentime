# Tamper Resistance & Remote Access

## Threat model & honesty

The managed user may have physical access. If they also have root, **no software can make
shutdown or network disconnection truly impossible.** OpenScreenTime's goal is therefore:

1. **Raise the cost** of tampering (make casual bypass hard).
2. **Detect and report** every tamper attempt.
3. **Recover automatically** (auto-restart, re-apply policy on drift and on boot).

We do NOT claim unbypassable enforcement. Anti-tamper marketing that claims otherwise is
lying. In the same spirit, everything below describes what the code **actually does today** —
aspirations live in the "What OpenScreenTime does not do" section, not disguised as features.
The flip side of this doc is [`TRANSPARENCY.md`](TRANSPARENCY.md), which explains the same
system to the person being managed.

## Levels

Configured per device via `devices.tamper_level`. Default **1**. **3** is opt-in per device,
toggled by the `set_tamper_level` command; the agent's `--tamper-max` flag can force a floor.

### Level 1 — Strong deterrence + alerting (DEFAULT)

- **Hardened root systemd unit** (`client/systemd/openscreentime-agent.service`, installed by
  `ost install-service`): `Restart=always`, `RestartSec=1`,
  `StartLimitIntervalSec=0` (never gives up restarting), `ProtectSystem=strict` with explicit
  `ReadWritePaths` carve-outs, `ProtectHome` off (must watch user sessions), `NoNewPrivileges`
  off (shells out to nft/resolvectl/chattr), `OOMScoreAdjust=-1000`.
- **Watchdog:** a separate `openscreentime-watchdog.timer` runs every 30 s and restarts the agent if
  its heartbeat file (`/run/openscreentime/heartbeat`, touched every enforcement tick) is missing or
  older than 90 s. Killing the agent process buys at most ~30 s.
- **Power-control masking:** a polkit rule (`/etc/polkit-1/rules.d/49-openscreentime.rules`) denies
  `org.freedesktop.login1` power-off / reboot / halt / suspend / hibernate / suspend-then-hibernate
  (and their `-multiple-sessions` variants) to everyone except root and the `ost-admin`
  recovery account. The physical power key and Magic SysRq are kernel/firmware levers a polkit
  rule cannot reach — see "What OpenScreenTime does not do".
- **DNS pinning:** `/etc/resolv.conf` points at the local filtering resolver; every 10 s tick
  re-checks it and re-pins on drift, emitting a `resolv_conf_drift` (warn) tamper event.
- **Firewall self-repair (fail-closed):** if the openscreentime nftables table disappears (e.g.
  `nft flush ruleset`), the tick emits an `nft_flush` (critical) event **and rebuilds the
  table from the effective policy** — a flush buys seconds of open network, not a session.
- **NetworkManager guard:** each tick polls `nmcli` for overall state; if NetworkManager
  reports disconnected, the agent runs `nmcli networking on` (best-effort) and emits an
  `nm_disconnect` (warn) event. This is a 10-second poll, not a D-Bus subscription — see
  "What OpenScreenTime does not do".
- **Clock-skew detection:** the enforcement tick runs on a monotonic timer, so wall-clock is
  expected to advance ~10 s per tick. A jump of more than an hour (the classic "set the clock
  back to dodge bedtime" move) emits a `clock_skew` (warn) event.
- **Boot persistence:** the unit is `WantedBy=multi-user.target` with
  `After/Wants=network-online.target`; policy is pulled and re-applied at startup.
- **Config at rest:** `/etc/openscreentime/agent.toml` (device token inside) is root-owned and
  chmod'd `0600` (best-effort — a failure to chmod is logged, not fatal).
- **Event delivery:** every tamper event is posted to the server; batches that can't be
  delivered are **buffered in memory (capped at 512, oldest dropped) and retried every tick**
  until they land. An agent restart while offline loses the buffer — but the outage itself is
  visible server-side as gone-dark time, so tampering is never *silent*, even when the
  fine-grained trail is lost.

### Level 3 — Maximum lockdown (OPT-IN)

Everything in level 1 **plus**:

- The polkit rule additionally denies `stop` / `disable` / `mask` of
  `openscreentime-agent.service` **and** `openscreentime-watchdog.service` / `openscreentime-watchdog.timer`
  (the recovery net) via `systemctl` for everyone except root and `ost-admin`.
- A logind drop-in (`/etc/systemd/logind.conf.d/50-openscreentime.conf`) sets `ReserveVT=0` and
  `KillUserProcesses=yes`, cutting off the spare-VT escape and killing leftover user
  processes at logout. `ost-admin` can revert it.
- A `boot_guidance` advisory event tells the admin to set a GRUB password, a BIOS/UEFI admin
  password, and disable USB boot. **These are recommendations** — bootloader and firmware are
  physical mitigations software can only advise on, never enforce.
- **Danger:** level 3 can lock the admin out of their own machine too. The UI requires an
  explicit confirm; keep the `ost-admin` account working before enabling.

## Offline behavior (fail-closed)

Losing sight of the server never opens the network:

- **Grace window** (default 900 s, `OST_OFFLINE_GRACE_SECS`): past it, the agent emits a
  `network_offline` event, keeps the last-known policy enforced, and re-asserts DNS + firewall
  aggressively every tick until contact resumes (`network_online`).
- **Offline hard-lockdown** (per-policy `lockdown.offline_lockdown_days`, `0` = disabled): a
  device that hasn't reached the server for N *days* freezes all managed users like an admin
  lock. The clock survives reboots — last contact is persisted as a wall-clock timestamp in
  `/var/lib/openscreentime/last_contact` — so "keep it powered off for a week, then use it offline
  forever" doesn't work. The parent PIN still unlocks.

## The escape hatches that always work

Deterrence must never become a hostage situation. At every level:

- **Parent PIN** (argon2 hash in the policy, never plaintext): typed into the lockout overlay
  (grants 30 minutes), dropped via the root-only file `/run/openscreentime/unlock_pin.<user>`, or
  used with the `ost unlock` CLI. Verification is against the hash and **fails
  closed** — no PIN configured means no PIN unlock.
- **`ost-admin`**: a local account by this name is exempt from every polkit denial
  (power controls, and the level-3 unit-stop mask).
- Root can always stop the agent (`systemctl stop` at level 1; at level 3 root remains
  exempt from the polkit mask). That is by design — see the threat model.

## What OpenScreenTime does not do

Claims you might expect from this category of product that we deliberately do not make:

- **No binary or config signature verification.** The agent trusts what's on its own root-owned
  disk. Self-updates verify a sha256 pinned in the server's manifest over TLS
  (see `AGENT.md`); a v2 should pin a minisign key so binaries verify independently of the
  transport.
- **The NetworkManager guard is a poll, not a subscription.** It checks `nmcli` once per 10 s
  tick. A D-Bus `StateChanged`/`DeviceRemoved` subscription with per-connection re-activation
  is the intended upgrade.
- **No "recovery shell killing".** Level 3 disables VT switching and surfaces bootloader
  guidance; it does not (and cannot meaningfully) remove `init=/bin/bash`-style escapes —
  that's what the GRUB/BIOS password guidance is for.
- **Physical access + root wins eventually.** The design goal is that it can't win *silently*:
  the attempt costs real effort, generates tamper events on the way, and the end state is a
  loudly visible gone-dark device in the console — not a quietly green one.

## Zero-trust enforcement primitives (Linux)

- **DNS:** a local `dnsmasq` instance; under `default_deny` it answers only allowlisted names
  (wildcards supported), forwards them to the policy's `upstream` (must be a literal IP —
  enforced server-side), and returns NXDOMAIN for everything else. `/etc/resolv.conf` is
  pinned and guarded (see above).
- **Firewall:** an `nftables` table, default-deny, allowing only policy ports +
  established/related + loopback + the server + the DNS upstream. Applied atomically (one
  `nft -f` transaction — a malformed rule can't leave the box with *no* table) and rebuilt
  on drift.
- **Screen time:** per-user session accounting from logind (seat-active sessions only; idle
  sessions — `IdleHint=yes` — don't burn budget). At zero balance: warnings beforehand, a
  60-second save-your-work grace, then the user's processes are frozen via the cgroup v2
  freezer. Screen-time freezes never fall back to killing the session; only an explicit
  admin lock may terminate as a last resort.

## Remote shell — removed

OpenScreenTime used to include a server-brokered, disclosed remote shell (a root PTY bridged from
the agent to a browser terminal). It was removed in v0.4: **there is no remote shell at
all anymore** — everything an admin can do goes through the UI, and the agent still never
opens an inbound listener (it only dials out, preserving default-deny inbound). Historical
`ssh` events remain in the event log as the audit record of past sessions. A possible
replacement — a secure reverse tunnel carrying native SSH+RDP — was considered and deferred.

## Device discovery — removed

The agent used to accept a `discover` command that swept its local subnet and reported
hosts as a `discovery_result` event. It was removed (migration 0013) along with the
command type, the event type and both API routes. A screen-time app has no business
port-scanning the household network, and nothing in the product ever consumed the results.
