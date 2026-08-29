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
- `DATABASE_URL=postgres://openscreentime:openscreentime@localhost:5432/openscreentime`
- `RP_ID=localhost` / `RP_ORIGIN=http://localhost:5173` (WebAuthn relying party)
- `BIND_ADDR=0.0.0.0:8080`
- `OST_PUBLIC_URL` — public base URL of the control center (OIDC redirect URI + post-login redirects); falls back to `RP_ORIGIN`.
- `OST_INSECURE_COOKIES` — session cookies are Secure by default; set to `1` only for plain-http dev.
- `OST_TRUST_PROXY` — set to `1` behind a reverse proxy so the rate limiter keys on the first `X-Forwarded-For` value instead of the peer address.
- `OST_OIDC_ISSUER` / `OST_OIDC_CLIENT_ID` / `OST_OIDC_CLIENT_SECRET` / `OST_OIDC_NAME` — OIDC SSO (e.g. Authentik); off unless issuer/client id/secret are all set, endpoints are discovered at startup.
- `RUST_LOG` — log filter, e.g. `openscreentime_server=debug,tower_http=info,info`.

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
sudo ./target/release/ost enroll --server http://localhost:8080 --token <ENROLL_TOKEN>
sudo ./target/release/ost run    # or install the systemd unit
sudo ./target/release/ost install-service   # writes + enables the hardened unit
sudo ./target/release/ost status   # enrollment/service status — the natural post-install check
sudo ./target/release/ost unlock --pin <PARENT_PIN>   # parent-PIN recovery: suspends enforcement (default 60 min, --minutes to override)
```

Dev tip: the agent supports `--dry-run` so it logs the nft/DNS/lockout actions it *would* take
without touching the host, for developing on your own machine.

### Cargo features
Always build from within `client/`. Feature flags (`client/Cargo.toml`):
- default (no features enabled): headless, enforcement-complete — this is what the server's
  agent-dist image ships. Lockout falls back to `wall` broadcasts since there's no display.
- `--features gui`: adds the egui full-screen lockout overlay in place of the `wall` fallback.
- `--features tray`: adds the `ost tray` subcommand, a per-user tray companion
  (time left, connection state, managed-device disclosure).

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
6. Click **Lock** → agent shows full-screen lock.

## Testing
- Client: `cd client && cargo test` (enforcement runner, usage ledger +
  clock-set-back defense, tamper confirmation monitor, lockout challenges, PIN hashing,
  tamper levels, self-update ordering, and more). Add `--features tray` for
  the tray notification selection tests too.
- Server: `cd server && cargo test` — 11 tests, including
  `presets::tests::presets_round_trip_through_policy_without_loss` — the preset drift canary.
  Any new field added to `Policy` must round-trip through the presets without loss, or this test
  fails; treat a failure here as "a Policy field isn't wired into presets," not a flaky test.
- Web: `cd web && bun run typecheck` (`tsc -b --noEmit`) and `bun run build` (`tsc -b && vite
  build`) — both must be clean.

### Testing an agent without a real host
The agent enforces on the host (nftables, DNS, cgroup freezer), so don't run real
enforcement on your workstation. Three tiers, cheapest first:

- **`--dry-run`** — the agent logs every action it *would* take (`WOULD RUN: nft …`,
  `WOULD WRITE /etc/resolv.conf …`) and touches nothing. Safe as non-root, anywhere.
- **A throwaway container as root** — exercises the *full protocol* (enroll → policy pull →
  DNS/firewall decisions → heartbeat → events). But a container usually has **no cgroup-v2
  freezer**, so it can't prove the *lock* — the agent will report `screen_time_no_freezer`.
  Good for the network/DNS half. `enroll` needs the tenant to have a `default` preset, or 404s:
  ```bash
  podman run --rm --network host -v "$PWD/client/target/debug/openscreentime":/usr/local/bin/openscreentime:ro \
    docker.io/library/archlinux bash -c '
      ost enroll --server http://127.0.0.1:8080 --token <ENROLL_TOKEN> &&
      ost --dry-run --time-accel 60 run'
  ```
- **A disposable Arch VM** — the only way to prove the real cgroup-v2 freeze on a genuine
  systemd seat, safely. `deploy/test/vm.sh` boots one on an overlay disk (instant rollback via
  `vm.sh reset`), with a managed `mia` user and an unmanaged `rescue` user so a lock can never
  strand you. It's an Arch (not Ubuntu) cloud image on purpose: the agent is built against the
  host's rolling glibc, newer than any Ubuntu LTS ships, so an Arch guest runs the ordinary
  release binary while an Ubuntu one can't. The loop:
  ```
  vm.sh up                    # boot (SSH forwarded on :28022; host reachable inside as ost-host.local:8080)
  # register a parent + add a device on the console, copy the enroll token
  vm.sh install <token>       # build + copy + enroll + install the hardened service
  # give mia's Kid profile a 1-minute daily limit in the console
  vm.sh seat                  # give mia a real GRAPHICAL local seat (Weston) + accelerate the agent
  vm.sh view                  # watch mia's SCREEN in your browser (noVNC) — see the overlay land
  vm.sh watch                 # (text) poll mia's cgroup.freeze until it flips
  vm.sh relock [accel]        # reset to a clean slate and re-arm, to watch the lock again
  vm.sh thaw                  # rescue path: stop the agent + unfreeze
  vm.sh shot [file]           # headless screenshot (QMP screendump → PNG), for eyeballing/CI
  ```
  `install` builds the agent with `--features gui`, so the lock is the real fullscreen egui
  **overlay** ("Time's up", a "SCREEN PAUSES IN Ns" countdown, the unlock-code field), not the
  headless `wall` broadcast. `up` boots with a VNC display (localhost only) + a QMP socket; `seat`
  starts a Weston (Wayland, pixman/CPU renderer — GL hangs on the emulated GPU) session for mia via
  a systemd service (seatd + linger, not a login shell — see `deploy/test/seat-setup.sh` for why);
  `view` serves a bundled noVNC client that points at QEMU's built-in VNC-over-websocket. Set
  mia's Kid daily limit small in the console first (e.g. 1 min).

  Gotchas the harness encodes so you don't trip on them: (1) the agent only counts **local seat**
  sessions (`loginctl Active=yes && Remote=no`) as screen time — an SSH login is `Remote` and never
  accrues, so `vm.sh seat` (a Weston seat), not `vm.sh ssh`, drives a lock; (2) the lock is
  **sticky** — hitting the daily limit locks mia for the day, and dropping back under budget does
  *not* auto-thaw (that takes an unlock code / earn-time grant, or `vm.sh thaw`); (3) the agent
  re-reads policy only on (re)start, so a live console/DB limit change needs an agent restart —
  `vm.sh relock` does that as part of resetting for another watch. The software-rendered desktop is
  heavy, so an occasional `ssh` step returns 255 under load — just re-run it. Keep tamper at Level 1
  while testing.

For the child **UI** with zero risk (no agent, no device), run the console in mock mode:
`cd web && VITE_USE_MOCK=1 bun run dev` renders all three `/me` looks from sample data.

## Repo conventions
- Rust: `cargo fmt` + `cargo clippy` clean. Errors via `anyhow`/`thiserror`. Async on Tokio.
- Web: TypeScript strict, Tailwind, components in `web/src/components`, design tokens from
  `DESIGN.md` in `web/src/theme.css`.
- Keep the shared `Policy` type identical across server (`serde`) and web (`types.ts`). The
  agent deserializes the same shape.
- Every new command/event type goes in `docs/API.md` first, then all three components.
