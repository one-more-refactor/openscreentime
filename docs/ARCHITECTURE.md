# Sentinel architecture

This is the technical map of how Sentinel is put together: the four components,
how they talk, how enforcement actually works on a device, and the trust
boundaries that decide what the software can and cannot promise. It's written
against the code — where a limitation exists, it's named rather than rounded up.

For audience-specific guides see the [docs index](README.md). This document is
for people building on, operating, or auditing the system.

---

## The shape of it

Sentinel is a self-hosted, zero-trust device manager for families and small
organizations. One server holds policy and identity; each managed device runs a
root agent that enforces that policy locally and keeps working when the server
is unreachable. Everything is owned by the operator — their VPS, their domain,
their data.

```
        ┌──────────────────────────────────────────────┐
        │  Web control center (web/)                     │  Bun · React · Vite
        │  Nothing-style monochrome UI · passkey login   │  Tailwind
        └───────────────────────┬────────────────────────┘
                                │ HTTPS / JSON — admin API, session cookie
        ┌───────────────────────▼────────────────────────┐
        │  Server (server/)                               │  Rust · Axum · SQLx
        │  passkey auth · policy engine · command queue   │  Postgres · multi-tenant
        │  event log · SSH broker · anti-cheat checks     │  serves the built SPA
        └───────────────────────┬────────────────────────┘
                                │ HTTPS + WebSocket — agent API, device-token bearer
        ┌───────────────────────▼────────────────────────┐
        │  Agent (client/)                                │  Rust · static binary
        │  DNS + firewall · screen-time · tamper resist   │  systemd · per-user
        │  usage ledger · lockout UI · reverse-SSH end    │  headless by default
        └─────────────────────────────────────────────────┘

        policy/ — the shared Policy document type, a path dependency of BOTH
                  server and client, so the wire contract can't drift.
```

| Path      | What it is                                              | Stack                      |
|-----------|---------------------------------------------------------|----------------------------|
| `server/` | Backend API, auth, policy engine, SSH broker, anti-cheat| Rust, Axum, SQLx, Postgres |
| `web/`    | Admin control center (the "Nothing" UI)                 | Bun, React, Vite, Tailwind |
| `client/` | Linux device agent                                      | Rust                       |
| `policy/` | Shared `Policy` document (used by server **and** client)| Rust                       |

The `policy/` crate is the load-bearing detail: because both sides depend on the
same Rust type, a policy written by the server deserializes into the exact same
structure the agent enforces. `web/src/types.ts` is a hand-maintained mirror of
those shapes (no codegen) — keep it in step when the Rust changes.

---

## Components

### Server (`server/`)

Axum over SQLx/Postgres, multi-tenant, single origin. It also serves the built
web SPA itself (`static_web.rs`, `SENTINEL_WEB_DIR`, SPA fallback), so there's
one origin and no production CORS. Responsibilities:

- **Admin auth** — passkeys only (WebAuthn/FIDO2 via `webauthn-rs`). No
  passwords anywhere. Sessions are DB-backed (`admin_sessions`), the cookie
  carries a random token and the DB stores its sha256 (`auth.rs`).
- **Device identity** — enrollment mints a one-time token; the agent exchanges
  it for a long-lived `device_token` (sha256-at-rest) it sends as a bearer.
- **Policy engine** — profiles hold a `Policy`; devices/users resolve to an
  effective policy. Edits enqueue `apply_policy`.
- **Command queue** — server→agent actions (`commands` table), delivered over
  the WS bus immediately when connected, else pulled on the next heartbeat.
- **Event log** — the agent's telemetry and the server's own audit trail
  (`events` table); it's the record, and it isn't auto-pruned.
- **SSH broker** — bridges a browser xterm terminal to a reverse tunnel from
  the agent (`GET /api/ssh/:id/ws`), so an operator can reach a shell behind NAT.
- **Anti-cheat checks** — cross-checks each heartbeat against known state and
  records an `evasion` event when they disagree (see [Anti-cheat](#anti-cheat)).
- **Parent companion surface** — a scoped, revocable bearer token
  (`parent_access_tokens`) an admin mints from Settings, accepted only on the
  narrow `/api/parent/*` routes (list pending time requests, approve/deny, read
  alerts). It is not a session and not tied to a passkey; it cannot reach
  policy, devices, SSH, or admin settings. This is the auth the tray
  parent-mode uses.
- **Phone alerts** — an optional background worker (`alerts.rs`) that sends
  one-way chat-bot messages (Discord/Slack webhook or Telegram) on confirmed
  tamper, device lockdown, and new time requests. Send-only: no inbound webhook,
  no bot polling. Configured via env; a no-op when unset.

The extractors in `state.rs` are the auth "middleware": a handler that takes
`AuthAdmin` gets admin-session auth for free; one that takes `AgentAuth` gets
device-token auth. Rate limiting is a fixed-window in-memory limiter keyed per
scope (`auth`, `enroll`, `dist`), applied as a route layer.

### Agent (`client/`)

A single static Rust binary, run as root by systemd, headless by default. The
`run` loop (`runner.rs`) is the orchestrator: connect the WS bus (falling back
to heartbeat polling), pull per-user policy, and run an **enforcement tick**
every 10s that accounts screen time, evaluates lockouts, re-asserts tamper
defenses, and reports usage. Enforcement primitives:

- **DNS** (`enforce/dns.rs`) — pins `/etc/resolv.conf` to a local resolver
  (dnsmasq), sets the immutable bit, re-pins on drift.
- **Firewall** (`enforce/firewall.rs`) — an `nft` table (`inet sentinel`),
  default-deny, applied atomically (`add; delete; table{}` in one `nft -f` so a
  bad rule can never leave the host with no table).
- **Screen time** (`enforce/screentime.rs`) — per-Linux-user active-seat
  accounting via `loginctl` (idle sessions excluded), enforced by the cgroup-v2
  freezer. A persistent [usage ledger](#the-usage-ledger) survives restarts.
- **Tamper resistance** (`tamper.rs`) — polkit masking of power/stop controls,
  a watchdog heartbeat file, NM disconnect guard, and re-assertion of DNS/nft
  drift. See [TAMPER.md](TAMPER.md) for the honest threat model.

Two Cargo features gate optional local UI, both off by default so the fleet
build stays minimal:

- `gui` (eframe/egui) — the full-screen lockout overlay, spawned as a detached
  `__lockout` subprocess so its blocking event loop never stalls the tick.
- `tray` (ksni + notify-rust) — a per-user StatusNotifierItem companion that
  reads the world-readable status snapshot and surfaces desktop notifications.
  It runs as the desktop user, never root.

### Web control center (`web/`)

React + Vite + Tailwind, the "Nothing" monochrome design language (see
[DESIGN.md](DESIGN.md)). `api.ts` is a typed fetch client with session-cookie
auth; every list/detail endpoint unwraps a named envelope (`{ devices: [...] }`,
`{ profile: {...} }`). Mock data (`mock.ts`) is served **only** when
`VITE_USE_MOCK=1` — there is no transport-failure fallback, a dead backend fails
loudly. Pages: Devices, Device detail, Profiles, Approvals, Events, Settings.

---

## Data flows

### Enrollment

```
admin clicks ADD DEVICE ─► server mints one-time enroll_token (TTL) ─► one-liner
   │                                                                      │
   │   curl install.sh | sudo SENTINEL_TOKEN=… sh                         ▼
   │                                          installer downloads + sha256-verifies
   ▼                                          the agent, runs `enroll`
agent POSTs enroll_token ─► server issues device_token ─► agent writes
   /etc/sentinel/agent.toml (0600) and installs the systemd unit.
```

Registration of the **first admin** is open; the moment an admin exists it locks
(`403 registration_closed`). Recovery is a deliberate, temporary
`SENTINEL_OPEN_REGISTRATION=1` window (see [OPERATIONS.md](OPERATIONS.md)).

### Policy propagation

```
admin edits profile ─► PUT /api/profiles/:id ─► enqueue apply_policy to
                                                 affected devices
WS-connected agent:  receives the command, re-pulls, re-applies.
poll-mode agent:     notices policy_version changed on its next heartbeat,
                     re-pulls, re-applies.
```

The agent caches the last-applied policy to `/etc/sentinel/policy_cache.json`
so the offline PIN-unlock path works with no server and no running agent.

### Heartbeat, usage, and commands

```
every ~15s (poll) or per-tick (WS):
  agent ─► { status, per-user used_minutes_today } ─► server
                                                       │
   server: last_seen = now();  ledger = GREATEST(recorded, reported);
           regression check (see Anti-cheat);  returns queued commands
                                                       │
  agent ◄──────────────────── commands (lock, unlock, apply_policy, credit_time,
                                        deny_earn, set_tamper_level, discover, ssh)
  agent ─► ack ─► server updates command + (for lock/unlock) device.status
```

A background sweep flips any `online` device whose `last_seen` is older than
3 minutes to `offline`. "Gone dark" (offline ≥ 7 days) is computed in the UI.

### Events

The agent buffers events in memory (cap 512, oldest dropped) and POSTs the whole
batch every tick; on failure the batch is kept and retried, so an offline tamper
event survives to reconnect (`Agent::flush_events`). The server also writes its
own audit events (lock/unlock decisions, earn approvals, SSH open/close,
anti-cheat findings).

### Reverse-SSH

The agent opens a PTY and streams it as base64 WS frames to the server, which
bridges them to a browser xterm terminal. Single-admin family use; the tray
discloses an open shell to the person at the machine. Known edges (reconnect can
leak a PTY; a second admin tears down the first) are documented in
[TAMPER.md](TAMPER.md).

---

## The enforcement model

Everything the agent enforces flows through the **enforcement tick** (10s). The
tick is idempotent by design: it re-asserts the desired state rather than
reacting to edges, so drift (a flushed firewall, an un-pinned resolv.conf, an
unfrozen user) is corrected within one tick regardless of how it happened.

Screen-time freezing is a cgroup-v2 freeze — reversible, and it never terminates
a session over a time limit (unsaved work is sacred). A whole-device **lock** is
different: it's an explicit parent/tamper response and may fall back to ending
the session if the freezer is unavailable.

Three things can lock the whole device, and they share one code path
(`decide_freeze` + the freeze branch of the tick), differing only in the message
shown:

| Source                | Trigger                                          | Cleared by                    |
|-----------------------|--------------------------------------------------|-------------------------------|
| Admin `lock` command  | Operator locks from the console                  | `unlock` command / parent PIN |
| Offline hard-lockdown  | No server contact for `offline_lockdown_days`    | server contact / parent PIN   |
| Confirmed tamper       | Sustained, verified evasion (see below)          | `unlock` / parent PIN at device |

The **parent PIN always wins**, offline, at the machine — a dead VPS can never
permanently brick the family laptop. The PIN is verified against the cached
policy by `sentinel-agent unlock`, which also tears down the nft table and
un-pins resolv.conf.

---

## Anti-cheat

Screen-time enforcement is only meaningful if the accounting can't be trivially
reset. Sentinel treats this like an anti-cheat problem with checks on **both**
ends, and — importantly — it distinguishes a real evasion attempt from a
transient technical blip before doing anything drastic.

### The usage ledger

The per-user counters (`UsageTracker`) persist to
`/var/lib/sentinel/usage_ledger.json` every tick and reload on boot. This closes
the **restart cheat**: without persistence a `systemctl restart` (crash,
watchdog, self-update, or a kid who found the trick) dropped the in-memory
counters to zero and granted a fresh daily budget.

The day boundary is **forward-only**. `roll_day` resets the counters only when
the wall clock has genuinely advanced past the accounting day; a clock set
*backward* (to earlier today or yesterday) keeps the existing day and its usage.
This closes the **clock set-back cheat**, which used to wipe the counter by
making "today" look like a different day. The readers agree: usage is reported
as long as the clock hasn't crossed into a later day, so a set-back can't zero
the reported minutes either.

### Verify, then lock

A single anomalous signal is often benign — a dropped packet, a firewall flush
from `firewalld` touching every table, an NTP correction, a laptop resuming from
suspend. Locking a device on the first blip would feel unfair and would fire on
false positives. So the agent **confirms** before it escalates (`TamperMonitor`):

- A monitored signal must persist across consecutive enforcement ticks (with a
  boot-grace window) before it counts as *confirmed*.
- Today the one signal that escalates to a whole-device lockdown is **sustained
  nftables tampering**. Our table is root-owned, exclusively ours, and rebuilt
  atomically every tick — so if it's *still* gone a tick later, something with
  root is deleting it faster than we heal it. That's a real attack, not
  collateral.
- Deliberately **not** auto-locked: `clock_skew` (suspend/resume and RTC-less
  boot look identical to a clock-set — the ledger defuses that cheat instead),
  `nm_disconnect` (roaming, a dropped packet), and `resolv_conf_drift`
  (systemd-resolved and DHCP legitimately rewrite it; we just re-pin).

A confirmed attempt sets `tamper_lockdown`, freezes every user with an honest
`TAMPERING DETECTED` full-screen notice, and emits a `critical` event. A parent
PIN at the device, or an admin `unlock`, lifts it.

### Server-side cross-check

The client is not trusted to be honest, so the server checks its story. Each
heartbeat's reported per-user total is compared against the recorded total
*before* the monotonic `GREATEST` clamp hides a drop. A heartbeat reporting
materially less than the server already recorded — a wiped or rolled-back client
ledger — records an `evasion` event. The clamp still neutralizes the cheat (the
recorded total can't go down); the event makes it visible instead of silent.

### What this does not claim

On a device where the user has **physical access and root**, shutdown and
network disconnection can never be made truly impossible — only expensive and
detectable. Booting a live USB, pulling the disk, or holding the power button
are outside software's reach and Sentinel says so. The anti-cheat layer raises
the cost of the *casual* cheats (restart, clock, DNS bypass, firewall tamper)
and makes the rest loud. See [TAMPER.md](TAMPER.md) for the full boundary.

---

## Trust boundaries

```
UNTRUSTED                            SEMI-TRUSTED                 TRUSTED
─────────                            ────────────                 ───────
managed Linux user  ── device_token ─►  agent (root)  ── passkey ─► operator
(the person managed)   over TLS          on the device    session     (the console)
                                            │
                                            └─ policy_cache + parent_pin_hash
                                               (offline authority at the machine)
```

- The **managed user** is untrusted by design. They may have a local login but
  not root; enforcement assumes they'll try to get around it.
- The **agent runs as root** and is the local authority. It authenticates to the
  server with a device token over TLS and holds the cached policy + PIN hash so
  it can make correct decisions with no network.
- The **operator** is trusted, and proves it with a passkey — a phishing- and
  password-database-resistant credential. There is no password to steal.
- A managed user **with root** collapses the first boundary. Sentinel detects
  and reports sustained tampering and preserves a `sentinel-admin` recovery path
  at every tamper level, but does not pretend root can't eventually win.

---

## Where to go next

- Deploy and operate: [DEPLOY.md](DEPLOY.md), [OPERATIONS.md](OPERATIONS.md)
- The agent in detail: [AGENT.md](AGENT.md), [TAMPER.md](TAMPER.md)
- The wire contract: [API.md](API.md), [DATA_MODEL.md](DATA_MODEL.md)
- The policy document and presets: [PROFILES.md](PROFILES.md)
- Build and test locally: [DEVELOPMENT.md](DEVELOPMENT.md)
