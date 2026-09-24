# Tamper resistance

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

Per computer, `devices.tamper_level`: **1** (the default) or **3** (opt-in). There is
no level 2. The console has no control for it; it is set through the API
(`PATCH /api/devices/:id { tamper_level }`), which sends `set_tamper_level`.
`ost --tamper-max` on the agent raises the ceiling to 3.

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
- **The clock can't be used against it:** every decision reads the trusted clock (NTP, else
  the server's time, else boottime from the last anchor — `client/src/clock.rs`), so setting the
  clock back or forward changes nothing. A wall-clock jump of more than an hour is still
  reported as a `clock_skew` (warn) event.
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
- **Danger:** level 3 can lock you out of your own machine too. Keep a root shell or an
  `ost-admin` account working before enabling it.

## Verify, then stop

One signal is enough to put things back; only one is enough to stop every screen. The agent's
own nftables table deleted again on consecutive ticks (with a 120 s boot grace) means something
with root is removing it faster than it can be rebuilt. Then the agent stops every screen —
"Stopped until a parent checks this computer" — and sends a critical event. The unlock code
(at the lock, `ost unlock`) or a console Resume lifts it. Everything else (resolv.conf drift,
NetworkManager disconnects, clock jumps) is repaired and reported, never punished: suspend,
roaming and DHCP look exactly like tampering.

## Offline behavior (fail-closed)

Losing sight of the server never opens the network:

- **Grace window** (default 900 s, `OST_OFFLINE_GRACE_SECS`): past it, the agent emits a
  `network_offline` event, keeps the last-known policy enforced, and re-asserts DNS + firewall
  every tick until contact resumes (`network_online`). Screen time carries on as normal.
- **Offline lockdown** (per-policy `lockdown.offline_lockdown_days`, `0` = off, off in every
  preset): a computer that hasn't reached the server for N days (at least 3) stops every
  managed user like a pause. Last contact is persisted (`/var/lib/openscreentime/last_contact`),
  so it survives reboots. It engages only while the local network is up — a laptop on holiday
  with no network isn't punished — and only if the computer holds an offline way back in
  (recovery codes or a backup code); otherwise it reports `offline_lockdown_no_credential` and
  doesn't lock.

## The escape hatches that always work

Deterrence must never become a hostage situation. At every level:

- **The unlock code** — a per-computer TOTP the agent verifies offline (single-use, with a
  wrong-code back-off), or a one-time recovery code, or a legacy profile backup code
  (argon2). Typed at the lock (30 minutes; the agent checks it, the lock holds nothing), with
  `ost unlock`, or at `sudo` on a managed computer (PAM). No secret configured means no unlock —
  it fails closed. See `AGENT.md` → Unlock code.
- **The lock never takes the keyboard.** It runs in its own session on its own VT (cage as
  `ost-lock`, or the agent's text lock), so the code can always be typed even though the whole
  frozen session — compositor included — is suspended. Agent restarts don't take it down; if
  no lock can be shown, nobody is frozen behind a blank screen. See `AGENT.md` → The lock.
- **`ost recover`** (as root): masks the agent, stops the watchdog and tears enforcement down in
  one go, for when you need the machine back now.
- **`ost-admin`**: a local account by this name is exempt from every polkit denial (power
  controls, and the level-3 unit-stop mask). The agent doesn't create it; make one if you want
  that door.
- Root can always stop the agent. That is by design — see the threat model.

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
- **No remote shell and no network scanning.** Both existed once and were removed (0.4 and
  migration 0013); historical `ssh` events stay readable. The agent opens no listener.
- **Physical access + root wins eventually.** The design goal is that it can't win *silently*:
  the attempt costs real effort, generates tamper events on the way, and the end state is a
  loudly visible gone-dark device in the console — not a quietly green one.
- **Browser DNS-over-HTTPS to an arbitrary IP is not fully stopped.** Whenever a policy blocks
  anything, the agent forces plaintext DNS through its own resolver (`force_dns` is switched on
  automatically when blocks exist), and every preset under 18 also drops DoT and the known
  public DoH provider IPs (`lockdown.block_doh`/`block_dot`). That closes plaintext alt-resolvers and the common
  one-click DoH toggles. It does **not** stop a determined user who points a browser at a DoH
  endpoint on an IP not in our list, pinned so its bootstrap never hits the local resolver —
  the query rides ordinary HTTPS/443, indistinguishable from any other. The enforcement-honesty
  probe checks the *system* resolver path (`getent`), so it can report the DNS block healthy
  while a browser tunnels around it; a **blocked category briefly reappearing in "Where the time
  goes" is the signal a parent actually has** for this. Truly closing it needs egress
  443-to-approved-only (a future maximum-lockdown option), which breaks too much to be a default.

## Enforcement primitives (Linux)

- **DNS:** a local `dnsmasq` forwards everything to the policy's `upstream` (a literal IP —
  enforced server-side), sinkholes blocked domains (`0.0.0.0` / `::`), and rewrites the big
  search and video sites to safe mode. `/etc/resolv.conf` is pinned and guarded (above). A
  legacy `default_deny` profile would answer only allowlisted names; the server opens those
  at startup, so none ship.
- **Firewall:** one `inet openscreentime` nftables table, base policy accept, with targeted
  drops for the bypasses the policy turns on (stray DNS, DoH, DoT, Tor, VPN ports). Applied
  atomically (one `nft -f` transaction — a malformed rule can't leave the box with *no* table)
  and rebuilt on drift.
- **Apps:** a blocked app's processes are closed for the user who blocks it (exact process
  name, never root, never another user).
- **Screen time:** only the foreground seat session with input or sound in the last 5 minutes
  counts. Every stop is announced at 15, 5 and 1 minute; an unannounced stop gets a
  save-your-work countdown; then the lock goes up on its own VT and the user's processes are
  frozen with the cgroup v2 freezer. A screen-time freeze never falls back to killing the
  session; only a pause may end a session, and only if the freezer isn't there.
