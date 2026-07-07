# Development

## Prerequisites
- Rust (stable, 1.75+) + `cargo`
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

## Web
```bash
cd web
bun install
bun run dev                      # :5173, proxies to server
```

## Client agent
```bash
cd client
cargo build --release
# enroll against a running server (needs root for enforcement primitives)
sudo ./target/release/sentinel-agent enroll --server http://localhost:8080 --token <ENROLL_TOKEN>
sudo ./target/release/sentinel-agent run    # or install the systemd unit
sudo ./target/release/sentinel-agent install-service   # writes + enables the hardened unit
```

Dev tip: the agent supports `--dry-run` so it logs the nft/DNS/lockout actions it *would* take
without touching the host, for developing on your own machine.

## End-to-end smoke test (the vertical slice)
1. Start server + db, run migrations (seeds nothing until a tenant exists).
2. `bun run dev`, open `:5173`, register the first admin with a passkey.
3. Create a device → copy the enroll token.
4. Run the agent with `--dry-run` and that token → device appears `online`, its OS users show up,
   each assigned the `default` profile.
5. Assign the `kids` profile to a user → agent pulls policy, logs the zero-trust DNS/firewall it
   would apply, and shows the lockout overlay when the (accelerated, dev) screen-time runs out.
6. Click **Lock** → agent shows full-screen lock. Click **SSH** → tunnel session opens.

## Repo conventions
- Rust: `cargo fmt` + `cargo clippy` clean. Errors via `anyhow`/`thiserror`. Async on Tokio.
- Web: TypeScript strict, Tailwind, components in `web/src/components`, design tokens from
  `DESIGN.md` in `web/src/theme.css`.
- Keep the shared `Policy` type identical across server (`serde`) and web (`types.ts`). The
  agent deserializes the same shape.
- Every new command/event type goes in `docs/API.md` first, then all three components.
