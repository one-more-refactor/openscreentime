# Development

## Prerequisites
- Rust 1.89+ (`rust-version` in each `Cargo.toml`) + `cargo`
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
cp .env.example .env             # DATABASE_URL, OST_PUBLIC_URL, etc.
cargo run                        # serves :8080; runs the migrations on start
```

Key env:
- `DATABASE_URL=postgres://openscreentime:openscreentime@localhost:5432/openscreentime`
- `OST_PUBLIC_URL` — the address the browser uses. RP ID, origin, CORS and cookie security derive from it; unset = `http://localhost:5173` (the Vite dev server), which gives `RP_ID=localhost` and non-Secure cookies.
- `BIND_ADDR=0.0.0.0:8080`
- `RP_ID` / `RP_ORIGIN` / `OST_INSECURE_COOKIES` — optional overrides of the derived values (`OST_INSECURE_COOKIES=1` forces non-Secure cookies, `0` forces Secure).
- `OST_TRUST_PROXY` — on unless `0`: the rate limiter keys on the **last** `X-Forwarded-For` value (what your proxy appended). Set `0` only when nothing sits in front of the server.
- `OST_BOOTSTRAP_TOKEN` — unset in dev, so first run ("Create your household") is open and the server logs that it is.
- `OST_OIDC_ISSUER` / `OST_OIDC_CLIENT_ID` / `OST_OIDC_CLIENT_SECRET` / `OST_OIDC_NAME` — OIDC SSO (e.g. Authentik); off unless issuer/client id/secret are all set; endpoints are discovered in the background (retried; the SSO button stays hidden until then).
- `RUST_LOG` — log filter, e.g. `openscreentime_server=debug,tower_http=info,info`.

## Web
```bash
cd web
bun install
bun run dev                      # :5173, proxies to server
```

### Mock / design-review mode
`VITE_USE_MOCK=1 bun run dev` serves the UI from bundled sample data with no backend running at
all — useful for design review. It is always signed in as the sample parent; add `?mock=solo`
(a household of one) or `?mock=empty` (nobody yet) to the URL for the other households. The gate lives in `web/src/api.ts`: the `read()` helper checks
the `VITE_USE_MOCK` env var *before* making any network call and returns fabricated data
directly; it is not a fallback triggered by a failed request. Under the dev proxy, a dead
backend produces an HTTP 500 (or a connection error), and neither one is caught to trigger mock
data — so without the explicit env var, a dead backend just fails loudly instead of falling back.

## Client agent

```bash
cd client
cargo build --release                       # headless: what most computers run
cargo build --release --features gui,tray   # desktop: the app window, the graphical lock, the companion
sudo OST_TOKEN=<ENROLL_TOKEN> ./target/release/openscreentime enroll --server http://localhost:8080
sudo ./target/release/openscreentime --dry-run --time-accel 60 run   # log, don't enforce; 1 s = 1 min
sudo ./target/release/openscreentime status
```

`--dry-run` makes every enforcement action log `WOULD RUN: …` / `WOULD WRITE …` instead of
touching the host; without root it's the only mode enforcement will run in. `enroll` accepts
plain `http://` only for loopback and `.local` hosts. `install-service` also creates the `ost`
symlink the rest of the docs use.

Features (`client/Cargo.toml`, all off by default):
- none — headless and complete: DNS, firewall, screen time, the text lock on its own VT.
- `gui` — the graphical lock (cage + egui as `ost-lock`) and `ost app`.
- `tray` — `ost tray`, the per-user companion: warnings at 15/5/1 minute, notifications.

## Tests

```bash
cd policy && cargo test                      # the rules function + policy/tests/schedule-vectors.json
cd server && cargo test                      # unit tests; the DB-backed ones need a Postgres (below)
cd client && cargo test && cargo test --features gui,tray   # the headless and the desktop build
cd web && bun run check                      # tsc + bun test (Ring, Icon, Login, Person, Me, …)
cd web && bun run build
```

**Database-backed server tests** (`server/src/tests_auth.rs`, `server/src/tests_rules.rs`,
`server/src/ledger.rs`) create a throwaway database per test on the Postgres at
`OST_TEST_DATABASE_URL`, else `DATABASE_URL`, and skip themselves (with a note) when neither is
set — point either at a server where the user may `CREATE DATABASE`. With `OST_REQUIRE_TEST_DB`
set (CI does), a skip is a failure.

`presets::tests` is the preset canary: every preset must round-trip through `Policy` byte for
byte, so a field added to `Policy` but not to the presets (or the other way round) fails there.

CI (`.github/workflows/ci.yml`) runs fmt, clippy and tests per crate — the server job with a
Postgres service — and the web typecheck and build. `build.yml` builds the two agent flavours
(musl headless; glibc desktop, checked against a glibc 2.35 floor), the image, and screenshots
of the mock console.

## Trying the agent without risking a real computer

The agent enforces on the host (nftables, DNS, the cgroup freezer, a VT), so don't run real
enforcement on your workstation. Cheapest first:

- **`--dry-run`**, anywhere, as anyone.
- **The container** (`deploy/test/run.sh`, `deploy/test/Containerfile`): a rootless Debian box
  with systemd, dnsmasq and nft, a kid and a parent user, and the musl agent. It proves the
  protocol — enroll, the WebSocket, policy, the DNS sinkhole, the `sudo` PAM hook — but has no
  display and usually no freezer.

  ```
  run.sh build [agent-binary]        # the image, and which agent binary to use
  run.sh up <server-url> <token>     # start and enroll (a dev server is http://ost.local:<port> inside)
  run.sh status | dns <domain> | offline | online | logs | sh | down
  ```

- **The VM** (`deploy/test/vm.sh`): a disposable Arch VM on an overlay disk (`vm.sh reset` is an
  instant rollback), with a managed `mia` and a `rescue` user who is never enrolled. The only
  place that proves the real freeze and the lock on a real seat.

  ```
  vm.sh up                  # boot (background)
  vm.sh install <token>     # build with --features gui, copy, enroll, install the service
  vm.sh seat [accel]        # give mia a graphical seat (Weston, via seat-setup.sh) + accelerate time
  vm.sh view | unview       # watch mia's screen in the browser (noVNC)
  vm.sh watch               # poll mia's cgroup.freeze
  vm.sh type <text>         # type at the seat (e.g. an unlock code at the lock)
  vm.sh shot [file]         # screenshot
  vm.sh relock [accel]      # reset the day and watch the lock again
  vm.sh thaw                # rescue: stop the agent, unfreeze
  vm.sh ssh | rescue | console | reset | down
  ```

  SSH sessions never count as screen time (no seat), so use `vm.sh seat` to make the lock bite.
  Keep tamper at level 1 in the VM.

- **A GNOME laptop** (`deploy/test/gnome-vm.sh up | view | unview | shot | ssh | console | status | down |
  nuke`): a persistent Debian 12 + GNOME VM with the child user `emma`, not enrolled — enroll it
  from your dev console (the host is `10.0.2.2:8080` inside). The realistic target for the app
  window and the companion without a tray.

For the child's **pages** with zero risk, run the console in mock mode (above).

## Repo conventions

- Rust: `cargo fmt` + `cargo clippy --all-targets --all-features -- -D warnings` clean. `anyhow`
  inside, `thiserror` at the edges. Tokio.
- Web: TypeScript strict; tokens only from `web/src/theme.css`; icons only from `brand/icons`
  (`components/Icon.tsx`); rings only through `components/Ring.tsx`.
- `web/src/types.ts` mirrors `policy/src/lib.rs` and the API by hand — change them together.
- A new command, event type or route goes in `docs/API.md` (and `docs/DATA_MODEL.md` for a CHECK
  list) in the same change.
