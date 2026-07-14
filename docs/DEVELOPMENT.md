# Development

## Prerequisites
- Rust (stable, 1.85+) + `cargo`
- Bun (1.1+)
- Docker (for Postgres) or a local Postgres 15+
- `sqlx-cli` (`cargo install sqlx-cli --no-default-features --features postgres`)

## Ports
- Server API + agent bus: `:8080`
- Web dev server (Vite): `:5173` (proxies `/api` and `/agent` → `:8080`)
- Postgres: `:5432`

## Server
```bash
cd server
docker compose up -d db          # postgres on :5432
cp .env.example .env             # DATABASE_URL, RP_ID, RP_ORIGIN, etc.
sqlx migrate run
cargo run                        # serves :8080
```

Key env:
- `DATABASE_URL=postgres://sentinel:sentinel@localhost:5432/sentinel`
- `RP_ID=localhost` / `RP_ORIGIN=http://localhost:5173` (WebAuthn relying party)
- `BIND_ADDR=0.0.0.0:8080`
- `SENTINEL_PUBLIC_URL` — public base URL of the control center (OIDC redirect URI + post-login redirects); falls back to `RP_ORIGIN`.
- `SENTINEL_INSECURE_COOKIES` — session cookies are Secure by default; set to `1` only for plain-http dev.
- `SENTINEL_TRUST_PROXY` — set to `1` behind a reverse proxy so the rate limiter keys on the first `X-Forwarded-For` value instead of the peer address.
- `SENTINEL_OIDC_ISSUER` / `SENTINEL_OIDC_CLIENT_ID` / `SENTINEL_OIDC_CLIENT_SECRET` / `SENTINEL_OIDC_NAME` — OIDC SSO (e.g. Authentik); off unless issuer/client id/secret are all set, endpoints are discovered at startup.
- `RUST_LOG` — log filter, e.g. `sentinel_server=debug,tower_http=info,info`.

## Web
```bash
cd web
bun install
bun run dev                      # :5173, proxies to server
```

### Mock / design-review mode
`VITE_USE_MOCK=1 bun run dev` serves the UI from bundled sample data with no backend running at
all — useful for design review. The gate lives in `web/src/api.ts`: the `read()` helper checks
the `VITE_USE_MOCK` env var *before* making any network call and returns fabricated data
directly; it is not a fallback triggered by a failed request. Under the dev proxy, a dead
backend produces an HTTP 500 (or a connection error), and neither one is caught to trigger mock
data — so without the explicit env var, a dead backend just fails loudly instead of falling back.

## Client agent
```bash
cd client
cargo build --release
# enroll against a running server (needs root for enforcement primitives)
sudo ./target/release/sentinel-agent enroll --server http://localhost:8080 --token <ENROLL_TOKEN>
sudo ./target/release/sentinel-agent run    # or install the systemd unit
sudo ./target/release/sentinel-agent install-service   # writes + enables the hardened unit
sudo ./target/release/sentinel-agent status   # enrollment/service status — the natural post-install check
sudo ./target/release/sentinel-agent unlock --pin <PARENT_PIN>   # parent-PIN recovery: suspends enforcement (default 60 min, --minutes to override)
```

Dev tip: the agent supports `--dry-run` so it logs the nft/DNS/lockout actions it *would* take
without touching the host, for developing on your own machine.

### Cargo features
Always build from within `client/`. Feature flags (`client/Cargo.toml`):
- default (no features enabled): headless, enforcement-complete — this is what the server's
  agent-dist image ships. Lockout falls back to `wall` broadcasts since there's no display.
- `--features gui`: adds the egui full-screen lockout overlay in place of the `wall` fallback.
- `--features tray`: adds the `sentinel-agent tray` subcommand, a per-user tray companion
  (time left, connection state, remote-shell transparency).

## End-to-end smoke test (the vertical slice)
1. Start server + db, run migrations (seeds nothing until a tenant exists).
2. `bun run dev`, open `:5173`, register the first admin with a passkey.
3. Create a device → copy the enroll token.
4. Run the agent with `--dry-run` and that token → device appears `online`, its OS users show up,
   each assigned the `default` profile.
5. Assign the `kids` profile to a user → run the agent with `--time-accel 60` (1 real second = 1
   simulated minute) so the screen-time budget is reachable in a dev session; it pulls policy,
   logs the zero-trust DNS/firewall it would apply, and shows the lockout overlay once the
   accelerated screen-time runs out.
6. Click **Lock** → agent shows full-screen lock. Click **SSH** → tunnel session opens.

## Testing
- Client: `cd client && cargo test` — 28 tests (enforcement runner, lockout challenges, PIN
  hashing, tamper levels, SSH PTY, self-update ordering, and more).
- Server: `cd server && cargo test` — 7 tests, including
  `presets::tests::presets_round_trip_through_policy_without_loss` — the preset drift canary.
  Any new field added to `Policy` must round-trip through the presets without loss, or this test
  fails; treat a failure here as "a Policy field isn't wired into presets," not a flaky test.
- Web: `cd web && bun run typecheck` (`tsc -b --noEmit`) and `bun run build` (`tsc -b && vite
  build`) — both must be clean.

## Repo conventions
- Rust: `cargo fmt` + `cargo clippy` clean. Errors via `anyhow`/`thiserror`. Async on Tokio.
- Web: TypeScript strict, Tailwind, components in `web/src/components`, design tokens from
  `DESIGN.md` in `web/src/theme.css`.
- Keep the shared `Policy` type identical across server (`serde`) and web (`types.ts`). The
  agent deserializes the same shape.
- Every new command/event type goes in `docs/API.md` first, then all three components.
