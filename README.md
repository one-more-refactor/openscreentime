<div align="center">

<img src="docs/assets/deploy-demo.gif" alt="Deploy Sentinel and enroll a device in two commands" width="820">

### Zero-trust device management for families — self-hosted, honest, and yours.

Enroll a device, lock it down by default, enforce screen-time and DNS per person,
and approve requests from your phone or your desk. One monochrome control center,
on **your** infrastructure — no cloud, no accounts, no telemetry.

![Rust](https://img.shields.io/badge/Rust-1.85+-0a0a0a?style=flat-square&logo=rust&logoColor=white)
&nbsp;![Postgres](https://img.shields.io/badge/Postgres-16-0a0a0a?style=flat-square&logo=postgresql&logoColor=white)
&nbsp;![Self-hosted](https://img.shields.io/badge/self--hosted-rootless%20Podman-0a0a0a?style=flat-square)
&nbsp;![Auth](https://img.shields.io/badge/auth-passkey--only-0a0a0a?style=flat-square)
&nbsp;![Default deny](https://img.shields.io/badge/default-deny-d71921?style=flat-square)

</div>

---

## Two commands

Everything below is the whole first run — a server, then a managed device.

```bash
# 1 · stand up the server (writes .env, builds, starts, waits for health)
git clone <this-repo-url> sentinel && cd sentinel
deploy/setup.sh --domain sentinel.example.com

# 2 · on the device you want to manage — the console hands you this line
curl -fsSL https://sentinel.example.com/install.sh | sudo sh
```

Open `https://sentinel.example.com`, register the first admin with a passkey
(registration locks the moment you do), and the device shows up online within a
minute. That's it. See [`docs/DEPLOY.md`](docs/DEPLOY.md) for the production details.

## What it does

**Enforcement, per person, on real Linux devices**
- **Zero-trust by default** — every enrolled device is **default-deny** for DNS and
  firewall (nftables). Nothing is allowed until a policy says so.
- **Per-Linux-user policy** — screen time, DNS, and app rules are tracked per user
  account, so a shared family computer just works.
- **Screen time that can't be gamed** — a persistent usage ledger survives restarts,
  and the day boundary is forward-only, so neither a reboot nor a clock set-back
  hands out free time. When time's up the screen pauses, with a 60-second
  save-your-work countdown.

**Fair to both sides**
- **Request & approve** — the kid asks for more time from their tray; a parent
  approves from the web console, a **paired tray on their own machine**, or a
  one-way **phone alert** (Discord / Slack / Telegram — send-only, nobody writes
  back to a bot).
- **Radically transparent** — a first-run intro tells the kid exactly what a parent
  can and can't see, and the tray always shows when a remote shell is open. What the
  software can't do, it says so.

**Anti-cheat, both ends**
- The agent **confirms** tampering before it reacts — a sustained attack on the
  firewall locks the device with an honest "tampering detected" screen; a transient
  blip does not. The server independently flags a client under-reporting its usage.

**Operate it like you mean it**
- **Passkey-only admin auth** (WebAuthn/FIDO2) — no passwords to steal. Optional OIDC SSO.
- **One-liner enrollment**, sha256-verified, with daily self-updates and a kept-`.bak` rollback.
- **Remote lockdown & SSH** — lock/unlock from the console, or reach a shell on any
  device through a server-brokered reverse tunnel, even behind NAT.

## Architecture

```
                 ┌───────────────────────────────┐
                 │   Web Control Center           │   React + Tailwind (Bun)
                 │   Nothing-style monochrome UI  │   Passkey login
                 └───────────────┬───────────────┘
                                 │ HTTPS / JSON  (admin + parent API)
                 ┌───────────────▼───────────────┐
                 │        Server (Rust)           │   Axum + SQLx + Postgres
                 │  passkey auth · policy engine  │   multi-tenant
                 │  command queue · SSH broker    │   WebSocket agent bus
                 │  anti-cheat · phone alerts     │
                 └───────────────┬───────────────┘
                                 │ HTTPS + WS  (agent API, device-token bearer)
                 ┌───────────────▼───────────────┐
                 │     Linux Agent (Rust)         │   static binary, systemd
                 │  zero-trust DNS + firewall     │   per-user enforcement
                 │  screen-time + usage ledger    │   full-screen lockout UI
                 │  tamper resistance             │   reverse-SSH endpoint · tray
                 └────────────────────────────────┘
```

Full technical map (data flows, enforcement model, anti-cheat design, trust
boundaries): **[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)**.

## Monorepo layout

| Path        | What                                                        | Stack                     |
|-------------|-------------------------------------------------------------|---------------------------|
| `server/`   | Backend API, auth, policy engine, SSH broker, anti-cheat    | Rust, Axum, SQLx, Postgres|
| `web/`      | Admin control center (the "Nothing" UI)                     | Bun, React, Vite, Tailwind|
| `client/`   | Linux device agent                                          | Rust                      |
| `policy/`   | Shared `Policy` document (used by server **and** client)    | Rust                      |
| `docs/`     | Full documentation — see the [docs index](docs/README.md)   | Markdown                  |

## Documentation

Organized by audience in [`docs/README.md`](docs/README.md):

- **Start here** — [`ARCHITECTURE.md`](docs/ARCHITECTURE.md): how it all fits together.
- **Parents** — [the day-to-day guide](docs/PARENT-GUIDE.md): profiles, screen time,
  granting time, the parent PIN, locking, gone-dark devices.
- **The person being managed** — [`TRANSPARENCY.md`](docs/TRANSPARENCY.md): exactly what
  your parents can and cannot see. (The kid also gets a short version as a first-run
  intro on the device.)
- **Operators** — [deploy](docs/DEPLOY.md), [day-2 operations](docs/OPERATIONS.md), and
  the [agent reference](docs/AGENT.md).
- **Contributors** — [development](docs/DEVELOPMENT.md), [API](docs/API.md),
  [data model](docs/DATA_MODEL.md), [tamper threat model](docs/TAMPER.md).

## Develop it locally

See [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) for the full loop. TL;DR:

```bash
cd server && docker compose up -d db && cargo run   # server + Postgres
cd web && bun install && bun run dev                # control center
cd client && cargo build --release                  # the Linux agent
```

Testing an agent without touching a real host? Run it `--dry-run` (it logs every
enforcement action instead of applying it), or drop it in a throwaway container as
root — see [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md).

## Platform support

| Platform | Status                                   |
|----------|------------------------------------------|
| Linux    | ✅ Supported (systemd)                   |
| Windows  | 🕗 Coming soon (Windows service + WFP)   |
| macOS    | 🕗 Coming soon (system extension / MDM)  |
| Android  | 🕗 Coming soon (DeviceOwner + VPN app)   |
| iOS      | 🕗 Coming soon (MDM profile)             |

## Honesty note

On a device where someone has **physical access and root**, shutdown and network
disconnection can never be made *truly* impossible — only expensive and detectable.
Sentinel's tamper resistance is **strong deterrence + real-time alerting** by default,
with an opt-in **maximum-lockdown** mode. It never claims otherwise, and it always
keeps a recovery path. See [`docs/TAMPER.md`](docs/TAMPER.md).

## License

TBD.
