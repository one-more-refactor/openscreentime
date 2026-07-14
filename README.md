# Sentinel — Zero-Trust Device Management

A clean, [Nothing](https://nothing.tech)-style device management platform for families and
small organizations. Enroll devices, lock them down by default (zero-trust), enforce DNS &
screen-time policy per person, and nudge healthy habits with Duolingo-style full-screen
interruptions — all from one beautiful monochrome control center.

> **Status:** Working v1, self-hosted. Linux agents (x86_64), under active development.

---

## What it does

- **Passkey-only admin auth** — no passwords, ever. WebAuthn/FIDO2 via `webauthn-rs`.
- **Zero-trust by default** — every enrolled device is **default-deny** for DNS and firewall.
  Nothing is allowed until a policy explicitly allows it.
- **Per-user policy** — screen time, DNS, app limits, and gamification are tracked **per Linux
  user account**, so shared family computers work correctly.
- **Preset profiles** — `kids`, `teen`, and `default`, plus custom profiles.
- **Screen time with parent controls** — per-user limits, full-screen lockout with pre-warnings,
  60-second grace, parent PIN override, and daily time credits (earn/grant minutes).
- **One-liner enrollment** — sha256-verified agent installer served by your own server, with
  daily self-updates (the previous binary is kept as `.bak` for rollback).
- **Device tray companion** — desktop status indicator showing policy state, remote-shell
  transparency, and gone-dark alerts.
- **Remote lockdown & SSH** — lock, unlock, or freeze devices from the console, or reach a
  shell on any enrolled device through a server-brokered reverse tunnel, even behind NAT.
- **Tamper resistance** — root-owned, systemd-hardened agent that resists casual kills,
  auto-restarts, masks user-level power controls, and reports tamper attempts in real time.

## Quick start

### Server (5 minutes)

```bash
git clone <this-repo-url> sentinel && cd sentinel
deploy/setup.sh --domain sentinel.example.com
```

The script generates a `.env` file with random secrets, builds the server and database, and
starts them. Point your reverse proxy (Caddy, nginx, etc.) at `127.0.0.1:8080`, then open
`https://sentinel.example.com` — register the first admin with a passkey. After that,
registration is locked. See [`docs/DEPLOY.md`](docs/DEPLOY.md) for production details.

### Enroll a device (30 seconds)

In the web console, click **ADD DEVICE**. A one-liner appears:

```bash
curl -fsSL https://sentinel.example.com/install.sh | \
  sudo SENTINEL_TOKEN=<token> sh -s -- --server https://sentinel.example.com
```

Paste it on the target machine (Linux, x86_64). The installer downloads and verifies the
agent (sha256), enrolls, and installs a systemd service. The device appears online within
a minute.

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

For running Sentinel in production on a VPS (rootless Podman compose stack), see [`docs/DEPLOY.md`](docs/DEPLOY.md).

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
