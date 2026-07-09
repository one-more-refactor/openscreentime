# Sentinel — Zero-Trust Device Management

A clean, [Nothing](https://nothing.tech)-style device management platform for families and
small organizations. Enroll devices, lock them down by default (zero-trust), enforce DNS &
screen-time policy per person, and nudge healthy habits with Duolingo-style full-screen
interruptions — all from one beautiful monochrome control center.

> **Status:** Early skeleton. End-to-end vertical slice is the current milestone.

---

## What it does

- **Passkey-only admin auth** — no passwords, ever. WebAuthn/FIDO2 via `webauthn-rs`.
- **Zero-trust by default** — every enrolled device is **default-deny** for DNS and firewall.
  Nothing is allowed until a policy explicitly allows it.
- **Per-user policy** — screen time, DNS, app limits and gamification are tracked **per Linux
  user account**, so shared family computers work correctly.
- **Preset profiles** — `kids`, `teen`, and `default`, plus custom profiles.
- **Duolingo-style host UX** — earn screen time via tasks, full-screen lockout when limits are
  hit, and streaks/habit nudges.
- **Device discovery & enrollment** — find devices on the network and enroll them with a token.
- **Remote lockdown** — lock, unlock, or freeze a device from the control center instantly.
- **Remote SSH** — reach a shell on any enrolled device through a server-brokered reverse
  tunnel, even when the device is behind NAT.
- **Tamper resistance** — a root-owned, systemd-hardened agent that resists casual kills,
  auto-restarts, masks user-level power controls, and reports tamper attempts in real time.

## Architecture

```
                 ┌───────────────────────────────┐
                 │   Web Control Center (Bun)     │   React + Tailwind
                 │   Nothing-style monochrome UI  │   Passkey login
                 └───────────────┬───────────────┘
                                 │ HTTPS / JSON  (admin API)
                 ┌───────────────▼───────────────┐
                 │        Server (Rust)           │   Axum + SQLx + Postgres
                 │  - Passkey auth (webauthn-rs)  │   Multi-tenant
                 │  - Device registry & policy    │   Command queue
                 │  - Reverse-tunnel SSH broker   │   WebSocket agent bus
                 └───────────────┬───────────────┘
                                 │ HTTPS + WS  (agent API)
                 ┌───────────────▼───────────────┐
                 │     Linux Agent (Rust)         │   Static binary, systemd
                 │  - Zero-trust DNS + firewall   │   Per-user enforcement
                 │  - Screen-time + gamification  │   Full-screen lockout UI
                 │  - Tamper resistance           │   Reverse SSH endpoint
                 └────────────────────────────────┘
```

## Monorepo layout

| Path        | What                                                        | Stack                     |
|-------------|-------------------------------------------------------------|---------------------------|
| `server/`   | Backend API, auth, policy engine, SSH broker                | Rust, Axum, SQLx, Postgres|
| `web/`      | Admin control center (the "Nothing" UI)                     | Bun, React, Vite, Tailwind|
| `client/`   | Linux device agent                                          | Rust                      |
| `policy/`   | Shared `Policy` document type (used by server + client)     | Rust                      |
| `docs/`     | Shared API contract, data model, design system, profiles    | Markdown                  |

## Getting started

See [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) for the full dev-loop. TL;DR:

```bash
# 1. Server + Postgres
cd server && docker compose up -d db && cargo run

# 2. Web control center
cd web && bun install && bun run dev

# 3. Linux agent (on a device you want to manage)
cd client && cargo build --release
sudo ./target/release/sentinel-agent enroll --server https://... --token <ENROLL_TOKEN>
```

## Platform support

| Platform | Status                                   |
|----------|------------------------------------------|
| Linux    | ✅ Supported (systemd)                   |
| Windows  | 🕗 Coming soon (Windows service + WFP)   |
| macOS    | 🕗 Coming soon (system extension / MDM)  |
| Android  | 🕗 Coming soon (DeviceOwner + VPN app)   |
| iOS      | 🕗 Coming soon (MDM profile)             |

## Security & honesty note

On a device where a user has **physical access and root**, shutdown and network disconnection
can never be made *truly* impossible — only expensive and detectable. Sentinel's tamper
resistance is **strong deterrence + real-time alerting** by default (level 1), with an opt-in
**maximum-lockdown** mode (level 3). See [`docs/TAMPER.md`](docs/TAMPER.md).

## License

TBD.
