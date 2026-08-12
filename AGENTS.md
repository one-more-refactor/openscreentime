# AGENTS.md — OpenScreenTime Device Management Platform

A guide for AI agents working in this repository. Documents the architecture, commands, patterns, conventions, and gotchas that aren't obvious from reading individual files.

---

## Project Overview

**OpenScreenTime** is a zero-trust device management platform for families/small organizations. Three components + shared policy crate:

| Path | Role | Stack |
|------|------|-------|
| `server/` | Backend API, auth, policy engine, agent WS hub | Rust, Axum, SQLx, Postgres, `webauthn-rs` |
| `web/` | Admin control center (Nothing-style monochrome UI) | Bun, React 18, Vite, Tailwind, `@simplewebauthn/browser` |
| `client/` | Linux device agent (root, systemd, nftables, DNS) | Rust, Tokio, `tokio-tungstenite`, `nix` |
| `policy/` | Shared `Policy` document (jsonb, serde) | Rust only |

**Architecture**: Web (HTTPS) → Server (API + WS) ← Agent (HTTPS + WS). Agent dials out; no inbound listeners. There is no remote shell (removed in v0.4) — all parent actions go through the UI.

---

## Essential Commands

### Development (see `docs/DEVELOPMENT.md`)

```bash
# 1. Server + Postgres
cd server
docker compose up -d db
cp .env.example .env        # edit: DATABASE_URL, RP_ID, RP_ORIGIN, etc.
sqlx migrate run
cargo run                   # serves :8080

# 2. Web control center
cd web
bun install
bun run dev                 # :5173, proxies /api and /agent → :8080

# 3. Linux agent (on a device to manage)
cd client
cargo build --release
sudo ./target/release/openscreentime enroll --server http://localhost:8080 --token <ENROLL_TOKEN>
sudo ./target/release/openscreentime run
sudo ./target/release/openscreentime install-service
```

### Production Deploy (see `docs/DEPLOY.md`)

```bash
git clone <repo> openscreentime && cd openscreentime
cp .env.example .env        # set POSTGRES_PASSWORD, RP_ID, RP_ORIGIN, OST_PUBLIC_URL
deploy/build.sh             # builds server+web image from Containerfile
podman-compose up -d
```

### Key Environment Variables

| Var | Purpose | Required |
|-----|---------|----------|
| `DATABASE_URL` | Postgres connection string | Yes (server) |
| `RP_ID` | WebAuthn relying party ID (bare domain) | Yes |
| `RP_ORIGIN` | WebAuthn origin (`https://...`) | Yes |
| `OST_PUBLIC_URL` | Public HTTPS base URL (OIDC redirect, falls back to RP_ORIGIN) | Prod |
| `OST_INSECURE_COOKIES=1` | Allow non-Secure cookies (dev only) | Dev only |
| `OST_TRUST_PROXY=1` | Rate limiter keys on last X-Forwarded-For hop | Behind proxy |
| `OST_OIDC_ISSUER/CLIENT_ID/CLIENT_SECRET` | OIDC SSO (all three required to enable) | Optional |
| `OST_OFFLINE_GRACE_SECS` | Agent fail-closed grace period (default 900s) | Optional |

---

## Code Organization & Architecture

### Server (`server/src/`)

```
main.rs       → AppState, router, middleware stack, static web fallback
state.rs      → AppState, Hub (agent WS), AuthAdmin/AgentAuth extractors
error.rs      → AppError (error envelope), AppResult
auth.rs       → Passkey (WebAuthn) registration/login, session cookies, token hashing
auth_oidc.rs  → OIDC SSO (Authentik), discovery at startup, admin matching by email
db.rs         → sqlx pool + embedded migrations
agent.rs      → Agent API: enroll, heartbeat, policy pull, events push, command ack, WS bus
devices.rs    → Device CRUD, lock/unlock, device-user listing
profiles.rs   → Profile CRUD, preset seeding, policy updates
presets.rs    → Hardcoded JSON for kids/teen/default presets (mirrors docs/PROFILES.md)
earn.rs       → Earn-time request flow (agent → server → web approval → credit_time cmd)
events.rs     → Event listing (paginated, filtered)
rate_limit.rs → Fixed-window in-memory limiter (auth: 10/60s, enroll: 5/60s)
static_web.rs → SPA fallback serving + 404→200 HTML rewrite middleware
```

**Request flow**: Every handler uses extractors (`AuthAdmin`, `AgentAuth`) that scope queries to `tenant_id`. No handler manually checks tenant — the extractor does it.

**WS Hub**: `Hub` tracks live agent connections (`device_id → mpsc::Sender<Value>`). Commands are enqueued in DB + pushed via WS if online. Agent acks via HTTP or WS.

### Web (`web/src/`)

```
App.tsx           → React Router, SessionProvider, ToastProvider, protected routes
main.tsx          → Entry, Vite HMR
api.ts            → Typed admin API client (credentials: "include"), mock mode (VITE_USE_MOCK=1)
types.ts          → TypeScript mirrors of Policy + entities (MUST stay in sync with policy crate)
lib/session.tsx   → Session context (passkey login/register, getMe, logout)
lib/theme.tsx     → Dark/light toggle (localStorage + :root data-theme)
lib/toast.tsx     → Toast notifications
lib/useAsync.ts   → useAsync hook for data fetching
lib/validate.ts   → Zod-ish validators for forms
components/       → Design system: Panel, Stat, StatusLed, DeviceCard, PolicyEditor, etc.
layout/Shell.tsx  → Left rail + top bar + fleet strip, ambient polling
pages/            → Login, Devices, DeviceDetail, Profiles, Approvals, Events, Settings
theme.css         → CSS variables from docs/DESIGN.md (monochrome, accent red, dot grid)
```

**Vite proxy** (`vite.config.ts`): Proxies `/api` and `/agent` → `http://localhost:8080` with WS upgrade.

**Mock mode**: `VITE_USE_MOCK=1` at build time → `api.ts` returns bundled sample data. Never silent in prod.

### Client (`client/src/`)

```
main.rs       → CLI (enroll, run, install-service, status, unlock), dry-run + tamper-max globals
runner.rs     → Main loop: WS bus (fallback heartbeat), policy pull, enforcement tick, command dispatch
client.rs     → HTTP client for enrollment/heartbeat/policy/earn-request
config.rs     → AgentConfig (TOML at /etc/openscreentime/agent.toml, 0600), AgentCtx (dry-run, root check)
protocol.rs   → Wire types: Command, Event, UsageReport, WS envelope (tagged JSON)
enforce/      → DNS, firewall, screentime (applies most restrictive across active users)
lockout.rs    → Full-screen overlay (eframe, optional `gui` feature)
tamper.rs     → Watchdog, NM D-Bus signals, polkit masking, config integrity
earn.rs       → Earn-time offers + the screen_time_earned event
unlock.rs     → Parent PIN CLI (suspends enforcement for N minutes)
sysusers.rs   → OS user enumeration (libc + users crate)
```

**AgentCtx** is `Arc` threaded everywhere. `Exec` wrapper honors `--dry-run` (logs instead of running).

**Enforcement model**: DNS/firewall are host-global; agent applies *most restrictive* policy across active users. Per-user network isolation is future work.

### Policy Crate (`policy/src/lib.rs`)

Single source of truth for `Policy` document. All components serialize/deserialize this exact shape.

**Critical serde patterns**:
- All sub-objects `#[serde(default)]` → forward-compat with unknown fields
- `lockdown` and `parent_pin_hash` use `skip_serializing_if` → absent when default/None, so preset JSON stays byte-identical
- `NetworkLockdown::is_default()` gates serialization omission

---

## Data Flow & Control Flow

### Enrollment
1. Admin creates device via `POST /api/devices` → returns `enroll_token`
2. Agent runs `enroll --server --token` → `POST /agent/enroll` with `enroll_token`, hostname, OS users
3. Server consumes token, creates `device_token` (hashed), `device_users` rows (assigned `default` profile), returns `device_id`, `device_token`, `poll_interval`
4. Agent writes `/etc/openscreentime/agent.toml` (0600), then `run`

### Policy Application
1. Agent WS/heartbeat → `GET /agent/policy` → `{ policy_version, users: [{ os_username, profile_kind, policy }] }`
2. Agent merges per-user policies, applies most restrictive network policy (DNS + firewall)
3. Screen-time runs every 10s tick (`screentime::UsageTracker`), freezes users via cgroup freezer when limit hit
4. Lockout overlay shows earn offers → user picks → `POST /agent/earn-request`

### Earn-Time Approval
1. Agent → `POST /agent/earn-request` (deduped per user/task/day server-side)
2. Web: `GET /api/earn-requests?status=pending` → parent clicks approve/deny
3. Approve: upserts `screen_time_ledger.earned_seconds` + enqueues `credit_time` command
4. Agent handles `credit_time` → `UsageTracker::add_earned` + emits `screen_time_earned` event

### Fail-Closed Offline (Agent)
- Tracks `last_contact` (any WS message or successful heartbeat)
- Beyond grace period (`OST_OFFLINE_GRACE_SECS`, default 900s): `ContactState::OfflineFailClosed`
- Re-asserts last-known policy every tick, emits `network_offline` tamper event once
- **Does NOT blackhole traffic** — device stays usable under strict allowlist

---

## Key Patterns & Conventions

### Rust (Server + Client + Policy)

| Convention | Detail |
|------------|--------|
| Errors | `anyhow` for internal, `thiserror` for typed (`AppError` enum), envelope: `{ "error": { "code", "message" } }` |
| Async | Tokio, `async`/`await` everywhere |
| SQL | `sqlx::query_as` with tuple structs for row mapping (see `DeviceRow` in `devices.rs`) |
| UUIDs | `uuid` crate v4 + serde, all DB ids are `uuid` |
| Time | `chrono` with `timestamptz`, `Utc::now()` |
| Logging | `tracing` + `tracing-subscriber`, `EnvFilter` from `RUST_LOG` |
| Config | `dotenvy` at startup, env vars for all settings |
| Serialization | `serde` + `serde_json`, `#[serde(default)]` on all optional sub-objects |
| Tenant isolation | Every query filters `WHERE tenant_id = $1` via `AuthAdmin` extractor |
| Token hashing | `sha2::Sha256` → hex, stored at rest (device tokens, session cookies) |

### TypeScript (Web)

| Convention | Detail |
|------------|--------|
| Types | `web/src/types.ts` mirrors `policy/src/lib.rs` + `docs/DATA_MODEL.md` exactly |
| API client | `api.ts` class with typed methods, `credentials: "include"` for cookies |
| Errors | `ApiError` class with `code`, `status`, `message` |
| State | React Context (`SessionProvider`) + `useSession()` hook |
| Styling | Tailwind + CSS variables from `theme.css` (monochrome, dot grid, accent red) |
| Components | `web/src/components/index.ts` exports all; design system per `docs/DESIGN.md` |
| Mock | `VITE_USE_MOCK=1` at build time only — never silent in prod |

### Database

- **Migrations**: `server/migrations/` (SQLx, `NNNN_description.sql`)
- `0001_init.sql` → base tables
- `0002_prod.sql` → `admin_sessions`, `earn_requests`, extended CHECK constraints
- Migrations run automatically on server startup (`db::migrate` in `main.rs`)
- Row-level tenant isolation enforced in application layer (every query has `tenant_id`)

### Naming

| Scope | Convention |
|-------|------------|
| Rust modules | `snake_case` |
| Rust types | `PascalCase` |
| Rust functions/vars | `snake_case` |
| DB columns | `snake_case` |
| JSON fields | `snake_case` (serde default) |
| TypeScript types | `PascalCase` |
| TypeScript vars | `camelCase` |
| CSS variables | `--kebab-case` |
| Env vars | `SCREAMING_SNAKE_CASE` |

---

## Testing & Quality

### Rust
```bash
cargo fmt --all
cargo clippy --all-targets --all-features
cargo test --all
```
- Server: integration-style tests in modules (see `rate_limit.rs` tests)
- Policy: round-trip test for presets (asserts serialize(parse(preset)) == normalize(preset))
- Client: `--dry-run` makes most enforcement testable without root

### Web
```bash
cd web
bun run typecheck      # tsc -b --noEmit
bun run build          # tsc -b && vite build
```
- No unit test framework configured yet; manual E2E via dev loop

---

## Gotchas & Non-Obvious Patterns

### Server

1. **Extractor = tenant scoping**: Never write `WHERE tenant_id = ...` manually. Use `AuthAdmin` / `AgentAuth` extractors — they embed `tenant_id` and return 401 if session/token invalid.

2. **Session cookies are DB-backed**: `admin_sessions` table, sha256-hashed token, 30-day TTL (not sliding). `OST_INSECURE_COOKIES=1` disables `Secure` flag for plain-http dev.

3. **Rate limiter keys on LAST X-Forwarded-For hop** when `OST_TRUST_PROXY=1`. First hop is spoofable; last hop = real peer appended by trusted proxy. See `rate_limit.rs:67-78` + test.

3. **WS command push is best-effort**: `Hub::push` returns `false` if agent not connected. Command stays `queued` in DB; agent pulls on next heartbeat.

4. **Static web fallback middleware**: `spa_ok` rewrites 404→200 ONLY for `text/html` responses. API 404s (JSON) stay 404.

5. **CORS allows only `RP_ORIGIN` with credentials**: Set in `main.rs:95-110`. Dev server proxies, so browser talks to `:5173` which proxies to `:8080`.

### Web

1. **Types must match policy crate exactly**: `types.ts` ↔ `policy/src/lib.rs`. `lockdown?` and `parent_pin_hash?` are optional in TS (absent = all off / no PIN).

2. **Mock mode is build-time only**: `VITE_USE_MOCK=1` at `bun run build` or `bun run dev`. Production builds never include mock data.

3. **Ambient fleet polling**: `Shell.tsx` polls `/api/devices` + `/api/earn-requests?status=pending` every 20s. Failures keep last snapshot. Pages handle their own errors.

4. **Design tokens in `theme.css`**: All colors, spacing, typography from `docs/DESIGN.md` as CSS variables. Tailwind config extends these.

5. **Passkey flow**: `@simplewebauthn/browser` for credential creation/assertion. Server endpoints: `/api/auth/register/start|finish`, `/api/auth/login/start|finish`.

### Client (Agent)

1. **`--dry-run` is mandatory for non-root dev**: `Exec::run` logs instead of executing. ALL enforcement modules use `Exec`. Refuses real enforcement without root.

2. **Root check at CLI top level**: `AgentCtx::require_root_for_enforcement()` called before `run`/`install-service`/etc.

3. **Config at `/etc/openscreentime/agent.toml` 0600**: `set_owner_only_600` best-effort. Token stored in plaintext (root-only readable).

4. **Heartbeat file at `/run/openscreentime/heartbeat`**: Used by systemd watchdog (level 1 tamper). Agent touches it each tick.

5. **Most restrictive network policy wins**: DNS/firewall are host-global; agent unions active users' policies and applies strictest. Per-user isolation is future work.

6. **Tamper level = max(device.tamper_level, --tamper-max)**: CLI flag raises ceiling to 3. `AgentCtx.tamper_max` propagates.

7. **Earn request dedup**: Client-side `requested_earn` HashMap avoids daily spam; server also dedupes.

### Policy Crate

1. **Preset JSON must stay byte-identical**: `lockdown` and `parent_pin_hash` omitted when default via `skip_serializing_if`. Tests assert round-trip equality.

2. **`app_limits` and streak nudges are gone**: both removed (migration 0013). `app_limits` was accepted but never enforced; streak nudges were engagement bait the product brief forbids. Wind-down warnings ("2 min left") survive in `runner::maybe_warn` and deliberately emit no event.

3. **Wildcard DNS `allowlist: ["*"]`**: Means "forward everything to filtered upstream" (used by `default` profile). Zero-trust mode stays `default_deny` structurally.

---

## Common Tasks

### Add a New API Endpoint

1. **Server**: Add route in `main.rs` router, create handler in appropriate module, use `AuthAdmin`/`AgentAuth` extractor.
2. **Contract**: Update `docs/API.md` with method, path, body, response.
3. **Web**: Add typed method to `api.ts`, update `types.ts` if new shapes.
4. **Agent** (if agent-facing): Add wire type to `protocol.rs`, handler in `runner.rs` or new module.

### Add a Policy Field

1. **Policy crate**: Add to `Policy` struct + sub-structs in `lib.rs`, with `#[serde(default)]`.
2. **Presets**: Update `presets.rs` + `docs/PROFILES.md` + `docs/API.md` example.
3. **Web**: Add to `types.ts`, update `PolicyEditor.tsx` form.
4. **Agent**: Handle in `enforce/` modules, apply in `runner.rs` tick.

### Modify Database Schema

1. Create new migration: `sqlx migrate add description` in `server/`
2. Edit generated `.sql` file
3. Run `sqlx migrate run` (or let server auto-migrate on startup)
4. Update `docs/DATA_MODEL.md` table definitions

### Deploy Changes

```bash
# On VPS
cd openscreentime
deploy/build.sh --pull    # git pull --ff-only + rebuild
podman-compose up -d      # rolling restart
```

---

## File References (Quick Navigation)

| Need | File |
|------|------|
| Server entry + router | `server/src/main.rs` |
| Auth (passkey + sessions) | `server/src/auth.rs` |
| OIDC SSO | `server/src/auth_oidc.rs` |
| Agent WS + commands | `server/src/agent.rs` |
| Rate limiting | `server/src/rate_limit.rs` |
| State + extractors | `server/src/state.rs` |
| Device CRUD | `server/src/devices.rs` |
| Profiles + presets | `server/src/profiles.rs`, `server/src/presets.rs` |
| Earn flow | `server/src/earn.rs` |
| Events | `server/src/events.rs` |
| Web API client | `web/src/api.ts` |
| Web types | `web/src/types.ts` |
| Web session | `web/src/lib/session.tsx` |
| Web layout | `web/src/layout/Shell.tsx` |
| Design tokens | `web/src/theme.css` |
| Agent main loop | `client/src/runner.rs` |
| Agent config | `client/src/config.rs` |
| Wire protocol | `client/src/protocol.rs` |
| Enforcement | `client/src/enforce/` |
| Tamper | `client/src/tamper.rs` |
| Policy document | `policy/src/lib.rs` |
| API contract | `docs/API.md` |
| Data model | `docs/DATA_MODEL.md` |
| Profiles | `docs/PROFILES.md` |
| Tamper details | `docs/TAMPER.md` |
| Deploy guide | `docs/DEPLOY.md` |
| Dev guide | `docs/DEVELOPMENT.md` |

---

## Memory: Commands to Remember

```bash
# Server dev
cd server && docker compose up -d db && cargo run

# Web dev
cd web && bun install && bun run dev

# Agent build + enroll (dry-run safe)
cd client && cargo build --release
sudo ./target/release/openscreentime enroll --server http://localhost:8080 --token <TOKEN>
sudo ./target/release/openscreentime run --dry-run --time-accel 60

# Full test cycle (DEVELOPMENT.md § "End-to-end smoke test")
# 1. Server + migrations
# 2. Web dev, register admin (passkey)
# 3. Create device, copy enroll token
# 4. Agent --dry-run with token → device online, users appear
# 5. Assign kids profile → agent applies policy, lockout triggers
# 6. Click Lock → full-screen lock

# Lint/check
cargo fmt --all && cargo clippy --all-targets --all-features
cd web && bun run typecheck && bun run build

# Production build
deploy/build.sh

# Migration
cd server && sqlx migrate add name_of_change
# edit generated file
cargo run  # auto-migrates
```

---

## Things NOT to Do

- ❌ Don't manually add `tenant_id` checks in handlers — extractors do it
- ❌ Don't serialize `lockdown` or `parent_pin_hash` when default/None — `skip_serializing_if` handles it
- ❌ Don't run enforcement without root outside `--dry-run` — `Exec` and `AgentCtx` enforce this
- ❌ Don't use `X-Forwarded-For` first hop for rate limiting — use LAST hop (see `rate_limit.rs`)
- ❌ Don't add CORS for origins other than `RP_ORIGIN` — single-origin deployment
- ❌ Don't shadow API routes with SPA fallback — `static_web.rs` only mounts on unmatched paths
- ❌ Don't silently use mock data in prod builds — `VITE_USE_MOCK=1` is build-time opt-in only
- ❌ Don't change preset JSON without updating both `presets.rs` AND `docs/PROFILES.md` — drift test catches it
- ❌ Don't assume agent has inbound connectivity — agent ONLY dials out (HTTPS + WS)
- ❌ Don't hardcode ports/paths — use constants and env vars (`BIND_ADDR`, `OST_WEB_DIR`, etc.)