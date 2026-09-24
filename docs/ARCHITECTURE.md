# OpenScreenTime architecture

The technical map: the parts, how they talk, how a computer counts and stops,
and the trust boundaries. Written against the code; where something is
limited, it says so. The docs index ([`README.md`](README.md)) points to the
doc that owns each detail.

## The shape of it

A self-hosted **house clock**: one server holds the household's people, rules
and history; each Linux computer runs a root agent that counts time, stops the
screen and blocks what was blocked — and keeps doing all of that when the
server is unreachable. **Allow by default**: nothing is blocked until someone
blocks it. Everything runs on the operator's own machine.

```
  ┌──────────────────────────────────────────┐
  │ Console (web/)                           │  React · Vite · Tailwind
  │ Family · a person · Computers · Settings │  session cookie
  └────────────────────┬─────────────────────┘
                       │ HTTPS / JSON
  ┌────────────────────▼─────────────────────┐
  │ Server (server/)                         │  Rust · Axum · SQLx · Postgres
  │ sign-in · rules · requests · usage       │  serves the console too
  │ command queue · events · alerts · ops    │
  └────────────────────┬─────────────────────┘
                       │ HTTPS + WebSocket — the agent dials out, device-token bearer
  ┌────────────────────▼─────────────────────┐
  │ Agent (client/), one per computer        │  Rust · systemd · root
  │ measure · rules · the lock · DNS + nft   │  app window + companion (desktop build)
  └──────────────────────────────────────────┘

  policy/ — the rules document and rules::evaluate, a dependency of server AND agent;
            the console checks the same schedule vectors (policy/tests/schedule-vectors.json).
```

`web/src/types.ts` mirrors the Rust shapes by hand; keep it in step.

## Server (`server/`)

Axum over SQLx/Postgres, one origin: it serves the built console itself
(`static_web.rs`), so there's no production CORS beyond `OST_PUBLIC_URL`, from
which the passkey domain, origin and cookie security are derived
(`settings.rs`). Migrations run on start.

- **Sign-in** — two doors (docs/AUTH.md): your name → a 6-digit code on your
  own computer (`login_code.rs`), or a discoverable passkey (`auth.rs`); SSO
  when configured (`auth_oidc.rs`); one-time vouchers from `ost login` and
  recovery links (`voucher.rs`, `recover.rs`). Sessions are DB-backed, sha256
  at rest. Inside, only the keys ask again: `confirm.rs` is a layer over
  `/api` that answers `428 step_up_required` until a 15-minute window is open.
- **People and rules** — every person is an account with a role and an age
  bracket (`members.rs`); their rules are their own profile, seeded from the
  bracket preset (`presets.rs`, `profiles.rs`). A member session reaches `/me`
  only. Adults' own rules (`/api/me/rules`) are not the hub's.
- **Computers** — enrollment spends a one-time token for a long-lived device
  token (`agent.rs`), links each OS login to a person (`members.rs`
  `link_os_user`), and holds the unlock-code secret (`unlock_code.rs`).
- **Command queue** — `commands` rows, pushed over the WebSocket when the
  agent is connected, else pulled on its next heartbeat. Pause and Resume are
  `lock` / `unlock`; the console reads the result from the agent's ack and
  `state` frame, never from its own intention.
- **Usage** — the agent reports its own seconds per login under its
  device-local day; the server files them (`ledger.rs`) and answers with what
  the same person used elsewhere, so one daily limit spans all their
  computers. Where the time went (`usage.rs`) is stored per hour and shown
  according to the person's bracket.
- **Events** — the agent's reports and the server's audit, ingested
  idempotently by the agent's event id (`events.rs`).
- **Alerts** — a webhook and/or a Telegram bot for the moments a person is
  needed; the bot also answers time requests (`alerts.rs`, `telegram.rs`).
  A paired companion has its own narrow bearer API (`parent.rs`).
- **The appliance** — `/health` checks the database (`ops.rs`); background
  loops restart themselves (`supervise.rs`); a retention sweep prunes
  sessions, codes, 21-day usage slices and 90-day events; operator problems
  alert once per incident. `deploy/` starts it at boot, backs it up nightly
  and updates it daily with a rollback (docs/DEPLOY.md).
- **Agent builds** — the image bundles a headless (musl) and a desktop
  (glibc, `gui,tray`) agent; `/install.sh` and `/api/agent/*` serve them
  (`agent_dist.rs`).

Every handler takes an extractor (`AuthAdmin`, `AgentAuth`, `ParentAuth` in
`state.rs`) that authenticates and carries `tenant_id`; queries filter on it.
A fixed-window limiter guards the unauthenticated surfaces (`rate_limit.rs`).

## Agent (`client/`)

One Rust binary, root, under systemd (`openscreentime-agent.service`, a
watchdog timer). `runner.rs` orchestrates:

- **The bus** — a WebSocket to the server with a poll fallback; presence is
  the socket plus a `state` frame (paused? who's frozen? enforcing?) at least
  every 60 s.
- **The tick** — every 10 s on its own timer, whatever the network does:
  measure, decide, enforce, write the ledger, touch the watchdog heartbeat.
- **Measure** (`enforce/activity.rs`) — a minute counts only for the
  foreground session on a seat, with keyboard/mouse/touch/gamepad input or
  sound in the last 5 minutes. Billed from monotonic awake time, capped at
  60 s a tick. The day is the local date on a **trusted clock**
  (`clock.rs`: the wall clock while NTP-synced, else the server's time, else
  the last anchor carried forward on `CLOCK_BOOTTIME`) — a hand-set clock is
  ignored, and the day only moves forward. The ledger survives restarts.
- **Decide** — `openscreentime_policy::rules::evaluate` (the function the
  console uses too): allowed?, why not?, when is the next stop? One override
  per person (a grant, a code at the lock, a Resume) beats limit, bedtime and
  hours until it ends; a pause beats an override.
- **Stop** (`lock/`, `warn.rs`) — warnings at 15, 5 and 1 minute through the
  per-user companion (desktop notifications; terminals for someone without a
  desktop). At the stop the agent starts the lock on VT 13 — `cage` hosting
  `ost __lockscreen` as the unprivileged `ost-lock` user on a `gui` build, else
  a text lock it draws itself with VT switching locked — switches to it, and
  only then freezes the person's cgroup (`cgroup.freeze`). Unlock is the
  reverse. No lock can be shown → nobody is frozen. A stop that wasn't
  announced gets a save-your-work countdown first; a pause is immediate. A
  freeze never kills a session over a time limit. The lock's state is
  persisted, so a restarted agent adopts it.
- **Keys** (`parentcode.rs`) — the unlock code is a per-computer TOTP the
  agent verifies offline (single-use, wrong-code back-off); recovery codes
  arrive as HMACs. Codes typed at the graphical lock reach the agent over
  `/run/openscreentime/lock.sock`, which answers only `ost-lock`
  (`SO_PEERCRED`); the lock holds no secret. The same code opens `ost unlock`
  and `sudo` on a managed computer (`pam.rs`).
- **Block** — DNS: a local dnsmasq forwarding to the filtering upstream,
  blocked domains sinkholed, safe-search rewrites, `resolv.conf` pinned and
  made immutable (`enforce/dns.rs`). Firewall: one `inet openscreentime`
  nftables table applied atomically, base policy accept with targeted drops
  for bypasses — DoH, DoT, stray DNS, Tor, optionally VPN ports
  (`enforce/firewall.rs`). Apps: a blocked app's processes are closed for the
  user who blocks it (`enforce/apps.rs`). DNS and the firewall are
  host-wide, so the agent merges everyone's network rules on that computer,
  strictest field by field. A legacy `default_deny` profile is still honoured,
  though the server opens those at startup.
- **Report** — usage seconds per login, app-open seconds and site lookups per
  hour (`attrib.rs`), events.
- **Update** (`update.rs`) — from its own server's `/api/agent/latest`, same
  build flavour, sha256-checked, refused if the new binary can't run here;
  the watchdog puts the previous binary back if an update crash-loops.
- **Surfaces** — `ost app` (the app window, `gui`), `ost tray` (the companion,
  `tray`), and CLI answers for everyone: `ost time`, `ost ask`, `ost code`,
  `ost login`.

## Data flows

**First run and a computer joining.**
`deploy/setup.sh` prints `https://<host>/#setup=<code>` → the first parent
names the household and makes a passkey (first run then closes) → **Add a
computer** mints a 24-hour one-time token → the one-liner downloads and
verifies the agent, `ost enroll` spends the token (asking which login is whose
if it can't tell), `install-service` installs the units → online within a
minute.

**Rules.** A save in the console → the person's profile → `apply_policy` to
each of their computers → the agent re-pulls `/agent/policy` (every person's
rules on that computer, the unlock-code material, the VPN profile), applies
it, and caches the bundle root-only for offline use.

**Time.** Each tick the ledger grows locally. Every heartbeat carries this
computer's seconds for its local day; the reply carries the person's use
elsewhere and the server's clock. Grants (`credit_time`) are idempotent by
command id and refused for an earlier day.

**Requests.** "Ask for more time" (the app, the lock, `ost ask`, `/me`) →
`earn_requests` → a card on the Family page, a Telegram message → Give
(`credit_time`) or Not now (`deny_earn`).

## Anti-cheat

- **Restarts and the clock** — the ledger is persisted and the day follows
  the trusted clock, so a restart, a reboot or a clock set back hands out no
  time.
- **Verify, then lock** — a single odd signal is usually benign (a firewall
  reload, suspend, NTP). Only one escalates to stopping every screen: the
  agent's nftables table deleted again on consecutive ticks after it was put
  back (with a boot grace). Everything else is repaired and reported.
- **Server-side check** — a heartbeat reporting materially less than the
  server already recorded for the same day raises one `evasion` event; the
  recorded total never goes down.
- **Offline** — past `OST_OFFLINE_GRACE_SECS` (15 min) the agent reports it
  once it's back and keeps re-asserting its rules; an opt-in
  `offline_lockdown_days` (at least 3) stops the computer until it reaches the
  server again. The unlock code always opens it.

Root and the machine in hand beats all of this eventually; see
[`TAMPER.md`](TAMPER.md).

## Trust boundaries

```
UNTRUSTED                 SEMI-TRUSTED                     TRUSTED
the person at the  ─►  agent (root) on the computer  ─►  the server  ◄─  a parent
computer (no root)     device token over TLS;            validates         passkey / own
                       offline authority: the cached     everything        computer's code
                       rules and unlock-code material
```

- The person at the computer is untrusted by design: a login, no root.
- The agent is root and the local authority. The server doesn't trust its
  reports blindly: shapes are validated, totals only go up, under-reporting
  is flagged.
- A parent proves who they are at sign-in and again for the keys. No password
  exists anywhere.
- A person **with root** collapses the first boundary. OpenScreenTime makes
  that slow and visible, and keeps recovery paths (`ost recover` as root, the
  unlock code, and an `ost-admin` account that polkit always lets through if
  the household creates one).

There is no remote shell, and the agent listens on nothing.
