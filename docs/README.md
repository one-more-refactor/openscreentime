# OpenScreenTime documentation

Pick your entry point by who you are. Every doc is written against the actual code —
where a limitation exists, the doc says so instead of rounding up.

## Running a family

| Doc | What it answers |
|---|---|
| [`PARENT-GUIDE.md`](PARENT-GUIDE.md) | The day-to-day console: enrolling devices, profiles, screen time, granting time, the parent PIN, locking, gone-dark devices. |
| [`TRANSPARENCY.md`](TRANSPARENCY.md) | **For the person being managed**: exactly what your parents can and cannot see and do on your machine. The honest contract. The kid also sees a short, skippable version of this as a first-run intro in the device companion itself (`gui`+`tray` build) — this doc is the fuller reference. |
| [`PROFILES.md`](PROFILES.md) | The policy document and the kids / teen / default presets, field by field. |

## Running the server

| Doc | What it answers |
|---|---|
| [`DEPLOY.md`](DEPLOY.md) | First-time install: `deploy/setup.sh`, reverse proxy, `.env`, first admin. |
| [`OPERATIONS.md`](OPERATIONS.md) | Day 2: updating, backup/restore, monitoring, recovering lost passkeys/PINs, common failures, uninstalling a device. |
| [`AGENT.md`](AGENT.md) | The device agent: CLI, every file it writes, systemd units, enforcement mechanics, self-update, offline behavior, troubleshooting. |
| [`TAMPER.md`](TAMPER.md) | The tamper threat model — what's enforced, what's detected, and what we deliberately do not claim. |

## Building on it

| Doc | What it answers |
|---|---|
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | The technical map: the four components, data flows, the enforcement model, the anti-cheat design, and the trust boundaries. Start here. |
| [`DEVELOPMENT.md`](DEVELOPMENT.md) | The dev loop: server + web + agent locally, mock mode, cargo features, tests. |
| [`API.md`](API.md) | Every HTTP endpoint and WebSocket frame, request/response shapes, auth, rate limits. |
| [`DATA_MODEL.md`](DATA_MODEL.md) | The Postgres schema, table by table, and the migration history. |
| [`DESIGN.md`](DESIGN.md) | The design system: Nothing-style monochrome, dot-matrix type, LED status dots. |
| [`CONTRACT-PROD.md`](CONTRACT-PROD.md) | Internal build contract for the v1 production push (kept for history/reference). |
