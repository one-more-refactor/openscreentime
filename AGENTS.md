# AGENTS.md — OpenScreenTime

A short guide for AI agents working in this repo: what's where, how to run and
test it, and the traps that aren't obvious from one file. The docs index
([`docs/README.md`](docs/README.md)) says which doc owns each question; the
code beats every doc.

---

## What it is

**The house clock.** A self-hosted screen-time clock for a whole household —
children, teens, and adults keeping time for themselves. *Set it once. It
keeps time.* Allow by default: nothing is blocked until someone blocks it, and
what is blocked is blocked for real. Calm, honest, silent unless a person is
needed. There is no remote shell.

| Path | Role | Stack |
|---|---|---|
| `server/` | API, sign-in, rules, agent bus, the appliance's health | Rust, Axum, SQLx, Postgres, `webauthn-rs` |
| `web/` | The console (Family, a person, Computers, Settings, Me) | Bun, React 18, Vite, Tailwind, `@simplewebauthn/browser` |
| `client/` | Linux agent: counts time, stops the screen, blocks, reports | Rust, Tokio, egui (`gui`), ksni (`tray`) |
| `policy/` | The shared rules document **and the one rules function** | Rust, serde |
| `brand/` | The brand board, the mark, the icon set | SVG, Python generators |
| `deploy/` | The appliance (setup, update, backup, restore) and the test harnesses | sh, Podman/Docker, QEMU |

Web (HTTPS) → server (API + WS) ← agent (HTTPS + WS). The agent only dials
out; nothing listens on a managed computer.

---

## Commands

### Develop (`docs/DEVELOPMENT.md`)

```bash
cd server && docker compose up -d db                 # Postgres on :5432
cp .env.example .env && cargo run                    # :8080, migrates on start
cd web && bun install && bun run dev                 # :5173, proxies /api and /agent
VITE_USE_MOCK=1 bun run dev                          # no backend; ?mock=solo|empty for other households
cd client && cargo build --release [--features gui,tray]
sudo OST_TOKEN=<TOKEN> ./target/release/openscreentime enroll --server http://localhost:8080
sudo ./target/release/openscreentime --dry-run --time-accel 60 run   # logs instead of enforcing
```

### Test

```bash
cd policy && cargo test                 # rules function + shared schedule vectors
cd server && cargo test                 # DB-backed tests need a Postgres (below)
cd client && cargo test && cargo test --features gui,tray
cd web && bun run check                 # tsc + bun test; then bun run build
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings   # per crate
```

- **DB-backed server tests** (`server/src/tests_auth.rs`, `tests_rules.rs`,
  `ledger.rs`) make a throwaway database per test from
  `OST_TEST_DATABASE_URL`, else `DATABASE_URL`, and skip when neither is set
  (a failure instead with `OST_REQUIRE_TEST_DB`, as in CI).
- **Server ↔ console shapes.** `server/src/tests_shapes.rs` records what the
  person page's endpoints really send in `web/src/test/server-shapes.json`;
  `web/src/test/shapes.test.ts` holds `types.ts` and the mock to it (a
  declared field the server doesn't send, or a mock field it doesn't, fails).
  Changed a response? `OST_WRITE_SHAPES=1 cargo test shapes`, then `bun test`.
- **Container harness** (`deploy/test/run.sh build|up|sh|status|dns|offline|online|logs|down`):
  a rootless Debian box with systemd and the musl agent — enroll, WS, DNS
  sinkhole, PAM. No display, no real freeze.
- **VM harness** (`deploy/test/vm.sh up|install <token>|seat|view|watch|type|shot|relock|thaw|reset|down`,
  with `seat-setup.sh`): a disposable Arch VM on an overlay disk with a
  managed `mia` and a `rescue` user. The only place that proves the real
  freeze and the lock on a real seat. `deploy/test/gnome-vm.sh` is a
  persistent Debian 12 GNOME "child's laptop" to enroll by hand.
- CI: `.github/workflows/ci.yml` (fmt, clippy, tests per crate — the server
  job runs the DB tests on its Postgres, the client tests default and
  `gui,tray` — web typecheck, `bun test`, build; every step `bash -eo
  pipefail`) and `build.yml` (agent builds
  incl. the glibc 2.35 floor, the image, screenshots of the mock console).

### Deploy (`docs/DEPLOY.md`, `docs/OPERATIONS.md`)

```bash
deploy/setup.sh --domain ost.example.com   # .env, image, stack, first backup, boot/backup/update units; prints the setup link
deploy/update.sh                           # pull, back up, swap, health-check, roll back (a daily timer runs it)
deploy/backup.sh / deploy/restore.sh
podman exec openscreentime-server /app/openscreentime-server recover <name>   # lost every passkey
```

A computer joins with the line the console gives:
`curl -fsSL https://HOST/install.sh | sudo OST_TOKEN=<token> sh -s -- --server https://HOST`.

### Settings that matter

| Var | Where | Purpose |
|---|---|---|
| `OST_PUBLIC_URL` | server | The one setting. RP ID, origin, CORS and cookie security derive from it (`server/src/settings.rs`). |
| `DATABASE_URL` | server | Postgres (compose derives it). |
| `OST_BOOTSTRAP_TOKEN` | server | The first-run setup code; `setup.sh` writes it and prints `https://<host>/#setup=<code>`. Unset = open first run (local only). |
| `OST_TRUST_PROXY` | server | **On unless `0`.** The rate limiter keys on the *last* `X-Forwarded-For` hop. |
| `OST_OIDC_ISSUER/CLIENT_ID/CLIENT_SECRET[/NAME]` | server | SSO; all three to enable. |
| `OST_ALERT_WEBHOOK`, `OST_TELEGRAM_BOT_TOKEN/CHAT_ID` | server | Phone alerts. |
| `RP_ID`, `RP_ORIGIN`, `OST_INSECURE_COOKIES`, `BIND_ADDR`, `OST_WEB_DIR`, `OST_AGENT_DIR` | server | Overrides you rarely need. |
| `OST_OFFLINE_GRACE_SECS` (900), `OST_NO_SELF_UPDATE`, `OST_CONFIG` | agent | Offline alert, self-update off, config path for tests. |

---

## Where things are

### Server (`server/src/`)

```
main.rs         router, middleware, background loops, retention sweep, /health
settings.rs     everything derived from OST_PUBLIC_URL
state.rs        AppState, the agent WS hub, extractors (AuthAdmin, AgentAuth)
error.rs        AppError → { "error": { code, message } }
auth.rs         passkeys, first run, sessions (sha256 at rest)
login_code.rs   door one: your name → a code on your own computer
confirm.rs      "confirm it's you" — the layer over /api that 428s the keys
voucher.rs      one-time sign-in links in a URL fragment (ost login, recover)
recover.rs      `openscreentime-server recover <name>`
auth_oidc.rs    SSO
members.rs      accounts, brackets, /api/me*, the member guard (a member sees /me only)
family.rs       GET /api/family — the whole home screen in one request
devices.rs      computers, pause/resume, who's who, enroll tokens
unlock_code.rs  per-computer unlock code (TOTP) + recovery codes
profiles.rs     rules CRUD; adults' own rules are theirs; legacy default_deny opened at startup
presets.rs      the five bracket starting rules
agent.rs        /agent/*: enroll, heartbeat, policy, events, acks, the WS bus
usage.rs        per-device-day usage in, "where the time went" out (age-gated)
ledger.rs       the per-person, device-local-day ledger
earn.rs         requests for more time, grants (credit_time)
commands.rs     the command queue, seen from the console
events.rs       event ingest (idempotent by client id) and listing
agent_dist.rs   /install.sh and the bundled agent builds (/api/agent/*)
alerts.rs, telegram.rs, parent.rs   phone alerts, the Telegram bot, the paired-companion API
ops.rs, supervise.rs                operator health, loops that survive their own bugs
vpn.rs          per-computer VPN profiles (API only; not in the console)
rate_limit.rs   fixed-window limiter: auth, enroll, dist, parent
static_web.rs   serves the built console as the fallback
tests_auth.rs, tests_rules.rs       DB-backed tests
```

Every handler scopes by tenant through its extractor (`AuthAdmin`,
`AgentAuth`, `ParentAuth`) — the extractor carries `tenant_id`; the query
still filters on it.

### Web (`web/src/`)

```
App.tsx          routes: /login /welcome / /child/:key /child/:key/rules /computers /add /settings /me
api.ts, mock.ts  typed client (credentials: include); VITE_USE_MOCK=1 = bundled sample data, build-time only
types.ts         mirrors policy/src/lib.rs and the API by hand — keep in step
lib/             session (two doors), confirm (428 → confirm it's you), family store, schedule, format, theme
components/      Icon (brand/icons), Ring, AvatarRing, Button, Modal, PauseEverything, UnlockCodePanel, WhereTheTime, Moments, …
layout/Shell.tsx the rail (Family · Computers · Settings · Me, then "Today"), a drawer below 1024 px
pages/           Login, Welcome, Family, Person (PersonToday, PersonRules), Computers, AddChild, Settings, Me
theme.css        the brand board's one token set, light + dark; page styles in styles/*.css
```

### Client (`client/src/`)

```
main.rs         CLI: enroll run install-service status time ask code login pair tray app recover unlock uninstall
runner.rs       the loop: WS bus (poll fallback), 10 s tick on its own timer, verdicts, the lock, commands
clock.rs        the trusted clock (NTP wall, else the server's, else boottime from the last anchor)
enforce/        activity (what counts), screentime (ledger, freeze), dns, firewall, apps, vpn
lock/           the lock: its own session on VT 13 (cage as ost-lock) or the text lock; socket.rs checks codes
warn.rs         15 / 5 / 1-minute warnings
parentcode.rs   unlock code (offline TOTP), recovery codes, legacy backup code (pin.rs)
pam.rs          sudo on a managed computer asks for the unlock code
unlock.rs       `ost unlock` (with a code) and `ost recover` (root)
login*.rs       `ost login` voucher, `ost code` sign-in code, the broker for non-root users
attrib.rs       which apps were open, which sites were looked up
update.rs       self-update from the enrolled server, verified, with automatic rollback
tamper.rs       watchdog heartbeat, polkit, level 3, verify-then-lock
app.rs, tray.rs, ui.rs   the app window and the companion (gui / tray features)
service.rs      install-service: units, polkit, the `ost` symlink, the lock unit + cage on gui builds
```

### Policy (`policy/src/`)

`lib.rs` is the rules document every component reads. `rules.rs` is **the
one rules function** (`rules::evaluate`): allowed now?, why not?, when is the
next stop? The agent, the server and the console all ask it the same
questions, checked against `policy/tests/schedule-vectors.json`.
`catalog.rs` is the app and category catalog behind one-click blocks.

---

## How it works (the parts that bite)

- **Allow by default.** Presets ship `dns.mode` and `firewall.mode` =
  `allow_all`: the family resolver filters, the catalog's blocks are
  sinkholed, and the firewall base policy is accept with targeted drops
  (DoH, DoT, Tor, forced DNS). `default_deny` still parses; the server opens
  any legacy profile using it at startup. DNS and firewall are host-wide, so
  on a shared computer the strictest active person's network rules apply.
- **Measurement.** A minute counts only for the foreground session on a seat
  with input or sound in the last 5 minutes. Billed on a 10 s tick on its
  own timer, from monotonic awake time (capped at 60 s a tick). The day is
  the computer's local day on the trusted clock; it only moves forward. A
  daily limit is one budget per person across all their computers. Every
  parent action writes the same **one override** per person.
- **The stop.** Warnings at 15, 5 and 1 minute. Then the lock starts on VT
  13 — `cage` running `ost __lockscreen` as `ost-lock` on a `gui` build,
  else a text lock drawn by the agent on VT 14 — the agent switches to it,
  *then* freezes the person's apps — never their session (compositor,
  `session.slice`, the login scope GDM re-authenticates through), and
  nothing of a desktop in its first minute. "Switch user" steps aside for the
  login screen; a stopped session coming back on screen meets the lock first. Unlock is the reverse. Codes typed at the lock
  go to the agent over `/run/openscreentime/lock.sock` (peer-checked); the
  lock holds no secret. No lock can be shown → nobody is frozen.
- **Keys.** The unlock code is a per-computer TOTP the agent checks offline;
  recovery codes are one-time. It opens the lock (30 minutes), `ost unlock`
  and `sudo` on a managed computer.
- **Sign-in: two doors.** Your name → a 6-digit code on your own computer
  (only on that computer's owner login), or a passkey. SSO when configured.
  Inside, only the keys ask again (`428 step_up_required` → "confirm it's
  you", a 15-minute window). No TOTP app, no passwords, no email.
- **Who's who.** Each OS login is its own person. An unsorted login on a
  parent's own computer gets rules that enforce nothing until someone sorts
  it — a parent is never locked out by a guess.
- **What a parent sees** is age-gated on the server (`usage.rs` `Exposure`):
  apps + sites up to younger teen, apps only for older teens, minutes only
  for adults. Keep `docs/TRANSPARENCY.md` true when you touch it.
- **The appliance.** Starts at boot, backs up nightly, updates daily with a
  rollback; `/health` checks the database. Agents update from their own
  server (headless musl or desktop glibc builds), refuse a build that can't
  run, and the watchdog rolls a crash-looping update back.

---

## Brand and icons

- The look is [`brand/board.html`](brand/board.html) (open it in a browser);
  the words are its Voice section and [`docs/PRODUCT.md`](docs/PRODUCT.md).
- **`brand/icons/*.svg` is the only icon source.** Draw a new icon in
  `brand/gen.py`, run it, and import the file (`web/src/components/Icon.tsx`
  lists them; its test checks the set). Never draw an icon inline, never use
  emoji as icons (emoji are faces only).
- The ring means one thing: **time used today, clockwise from the tick at
  twelve.** Never a spinner, countdown, hold-to-confirm or code entry. Red is
  stop and a healthy day has none; a parent's pause is neutral (dashed).
- Figtree everywhere; Space Mono only for a literal code. Sentence case. The
  wordmark is `OpenScreenTime`. One word per concept: pause, block (content
  only), time's up, person, limit, goal, time left, unlock code, recovery
  code, Ask for more time, Online, Remove, computer.
- Regenerate with `python3 brand/gen.py && python3 brand/build.py` (needs
  `fontTools`).

---

## Gotchas

1. **Migrations run on server start** (`db::migrate`). They are numbered
   0001–0027, then 0030 — 0028 and 0029 don't exist. A new one is 0031+.
2. **Rules document compatibility.** Every sub-object is `#[serde(default)]`;
   `lockdown`, `blocks`, `focus` and `parent_pin_hash` are skipped when empty,
   so presets round-trip byte-identical (`presets::tests`). `types.ts` is a
   hand-kept mirror.
3. **`gamification.lockout.unlock_challenge` is dead.** Nothing reads it; the
   lock always takes the unlock code. `parent_pin_hash` is only a legacy
   backup code.
4. **A member session sees `/me` only** — the server 403s the rest
   (`members.rs` guard); the web redirects.
5. **Own rules are theirs**: a parent's rules for themselves, and an adult's
   or self-managed person's, are editable only by that person (`/api/me/rules`);
   anyone else — another parent included — gets 403 (`profiles.rs`
   `private_profile_ids`, `members::sets_own_rules`). A parent sees such a
   person's minutes only: `/api/usage/where` refuses them and `/api/events`
   leaves out their logins' events (`usage::hub_exposure`).
6. **WS push is best-effort**; a command stays queued and is pulled on the
   next heartbeat. Pause shows "Pausing…" until the agent confirms.
7. **`--dry-run` is required off-root.** Every enforcement action goes through
   `Exec`; `AgentCtx::require_root_for_enforcement` refuses otherwise.
8. **Don't name a CSS class `ring`** — Tailwind owns it.
9. **Mock mode is build-time only** (`VITE_USE_MOCK=1`), and always signed in.
10. **Tamper level is 1 or 3** (server-side; the console doesn't show it).
    Level 3 blocks stopping the agent and VT switching — keep test VMs at 1.

---

## Don't

- Don't block by default, ship an allowlist, or call it zero-trust.
- Don't add a password, a TOTP app, or a third sign-in door.
- Don't add an inbound listener or a remote shell to the agent.
- Don't freeze anyone without a lock on screen, or kill a session over a time limit.
- Don't draw an icon or a ring outside `brand/icons` / `Ring`.
- Don't show a wish as a fact: "Pausing…" until the computer says paused.
- Don't widen what a parent sees without changing `docs/TRANSPARENCY.md` in the same commit.
- Don't key the rate limiter on the first `X-Forwarded-For` hop.
- Don't silently use mock data in a prod build.
- Don't push, merge to `main` or open a PR without asking.
