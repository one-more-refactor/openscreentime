# API contract

Every route is registered in `server/src/main.rs`. Three surfaces:

- **Console API** (`/api/*`) — the web console. A session cookie from any
  sign-in door (docs/AUTH.md).
- **Agent API** (`/agent/*`) — the Linux agent. Bearer `device_token`, except
  enrollment, which spends a one-time `enroll_token`.
- **Companion API** (`/api/parent/*`) — a paired companion (the tray's parent
  mode). Bearer parent access token.

All bodies are JSON. Errors are `{ "error": { "code": string, "message": string } }`
with a matching status. Any other path under `/api/` or `/agent/` is a JSON
`404`, never the console's HTML.

Base URL in dev: `http://localhost:8080`.

`GET /health` — unauthenticated. `200 { "status": "ok", "service":
"openscreentime-server", "version": "x.y.z", "db": "ok" }` while the server and
its database answer; `503` with `"status": "degraded", "db": "unreachable"`
when Postgres does not (checked with a 2 s timeout, cached for 2 s).

---

## Auth (docs/AUTH.md)

Two doors — a name and a code shown on your own computer, or a passkey — plus
OIDC SSO when configured. First run (zero accounts) creates the household; it
needs the setup code (`OST_BOOTSTRAP_TOKEN`) when the server has one, and
refuses with **403 `registration_closed`** once an account exists.

| Method | Path                        | Body / Notes                                            |
|--------|-----------------------------|---------------------------------------------------------|
| GET    | `/api/auth/config`          | public → `{ needs_setup, setup_code_required, auth: { oidc, oidc_name } }` |
| POST   | `/api/auth/register/start`  | first run: `{ name, setup_token? }` → `CreationChallengeResponse` (resident key required) |
| POST   | `/api/auth/register/finish` | `{ credential, setup_token? }` → household + session, `{ admin }` |
| POST   | `/api/auth/code/start`      | `{ name, code_challenge }` → `{ request_id, expires_in_secs }`; the code goes to that person's own computer (identical answer for unknown names) |
| POST   | `/api/auth/code/verify`     | `{ request_id, code_verifier, code }` → session, `{ ok, role }`; `401 wrong_code` (type again) or `410 code_expired` (5 min / 5 tries) |
| POST   | `/api/auth/login/start`     | → `RequestChallengeResponse` for a discoverable passkey (no name) |
| POST   | `/api/auth/login/finish`    | `{ credential }` → session, `{ admin }`                 |
| GET    | `/api/auth/oidc/start`      | 302 to the provider's authorize URL                     |
| GET    | `/api/auth/oidc/callback`   | `?code&state` → session + redirect `/` (see below)      |
| GET    | `/api/auth/oidc/setup/:token` | SSO first run: the parked identity behind a `/welcome` link; 404 once used or expired |
| POST   | `/api/auth/oidc/setup/:token` | `{ username, display_name?, setup_token? }` → creates the first account and a session |
| POST   | `/api/auth/voucher`         | `{ voucher }` → session (`ost login`, 7 days)           |
| POST   | `/api/auth/link`            | `{ token }` → session (recovery link from `openscreentime-server recover`) |
| POST   | `/api/auth/logout`          | clears session (deletes the DB row)                     |
| GET    | `/api/auth/confirm`         | → `{ armed_until, passkey, computer }`                  |
| POST   | `/api/auth/confirm/passkey/start` / `finish` | a passkey assertion → `{ armed_until }` |
| POST   | `/api/auth/confirm/code/start` | a code to your own computer → `{ request_id, expires_in_secs }`; 409 if none is online |
| POST   | `/api/auth/confirm/code/verify` | `{ request_id, code }` → `{ armed_until }`         |
| GET    | `/api/me/passkeys`          | → `{ passkeys: [{ id, nickname, created_at, last_used_at }] }` |
| POST   | `/api/me/passkeys/new/start` / `finish` | add a passkey to your account          |
| DELETE | `/api/me/passkeys/:id`      | → `{ ok: true }`; 409 if it's the last credential and OIDC is disabled |

### Confirm it's you

Signing in is the proof; ordinary changes need nothing more. The **sensitive
corner** — unlock codes, recovery codes, passkeys, pairing tokens, Telegram
pairing, `assign-account`, `enroll-token`, VPN configs — answers **`428
step_up_required`** until the session has a live 15-minute confirm window,
opened by a fresh sign-in, a passkey, or a code from your own computer. It's a
layer (`server/src/confirm.rs`), so routes added later are guarded
automatically. Confirming rotates the session token, keeping the old one valid
for 2 minutes so in-flight requests and second tabs survive.

### Agent side of sign-in

| Method | Path | Notes |
|--------|------|-------|
| POST   | `/agent/voucher`            | mint a one-time (2 min) voucher for a local surface on that machine to exchange at `/api/auth/voucher` |
| POST   | `/agent/enroll/preview`     | `{ enroll_token }` → `{ owner, owner_is_parent, machine_salt }` without using the token (so `ost enroll` can ask which login is the owner's, and key its machine identity for this household) |

`POST /agent/enroll` also takes `installer` (the login the install ran from)
and `owner_login` (the one the installer picked), and answers with `users:
[{ os_username, person, parent }]`. Commands include **`login_code`**
`{ request_id, name, os_users, code, purpose, site, expires_in_secs }` — show
the code to exactly those OS logins (docs/AUTH.md). It is sent only to an
agent that lists `"login_code"` in `features` (its `state` frame, or the
heartbeat body's `features`); it is never redelivered, and never shown in
`GET /api/devices/:id/commands`.

Sessions are DB-backed (`admin_sessions`, sha256-hashed token, 30-day TTL) and carried in the
`ost_session` cookie: `HttpOnly`, `SameSite=Lax`, `Secure` unless
`OST_INSECURE_COOKIES=1`. WebAuthn *challenge* state is held server-side in a short-TTL
in-memory store keyed by a temporary cookie.

### OIDC SSO (e.g. Authentik)

Enabled when `OST_OIDC_ISSUER`, `OST_OIDC_CLIENT_ID` and `OST_OIDC_CLIENT_SECRET`
are all set (`OST_OIDC_NAME` optionally labels the login button, default "SSO"). Endpoints
are discovered at startup from `<issuer>/.well-known/openid-configuration`; authorization-code
flow with scopes `openid email profile`; redirect URI is
`<OST_PUBLIC_URL>/api/auth/oidc/callback` (`OST_PUBLIC_URL`; `RP_ORIGIN` if only that is set).
The callback needs `email_verified: true` and matches that email against an
account's username or email (case-insensitive, any household). On a server
with no accounts it parks the identity and redirects to `/welcome?setup=<token>`,
where the first parent picks a name (`/api/auth/oidc/setup/:token`, which needs
the setup code like the passkey first run). An unknown email on a server that
has accounts → `/login?error=sso_unknown_account` (no auto-provisioning); other
failures → `/login?error=sso_failed`.

### Rate limiting

Fixed-window, in-memory, per client IP: the **last** `X-Forwarded-For` value,
unless `OST_TRUST_PROXY=0`, when it's the peer address (`rate_limit.rs`; trust
is on by default because the supported deploy sits behind a proxy). A trusted reverse proxy appends the real peer
IP to the end of XFF, so the last hop is the only element the client can't forge — keying on the
first value would let an attacker rotate `X-Forwarded-For` per request and land each one in a
fresh bucket, defeating the limiter entirely. Over-limit requests get a 429 error envelope.

- `auth` — 10 req / 60 s: register, code start/verify, passkey login, confirm
  code start/verify, voucher, link, OIDC start/callback/setup
- `enroll` — 5 req / 60 s: `/agent/enroll`, `/agent/enroll/preview`
- `dist` — 30 req / 60 s: `/install.sh`, `/api/agent/latest`, `/api/agent/download/:file`
- `parent` — 60 req / 60 s: `/api/parent/*`

---

## Agent distribution (public, no auth)

The production image bundles two agent builds under `/app/agent`
(`OST_AGENT_DIR`): headless (musl, static) and desktop (glibc, `gui,tray`). A
dev `cargo run` has no bundle; `install.sh` then falls back to GitHub releases. The binary is
not a secret — enrollment (one-time token) is the auth boundary.

| Method | Path                        | Notes                                                     |
|--------|-----------------------------|-----------------------------------------------------------|
| GET    | `/api/agent/latest`         | → `{ version, artifacts: [{ target, features, url, sha256 }] }` |
| GET    | `/api/agent/download/:file` | the artifact bytes (`application/octet-stream`); `:file` must be a bare filename (no `/` or `..`) |
| GET    | `/install.sh`               | POSIX installer (embedded from `server/install.sh`)       |

Install one-liner (shown in the web enroll modal; the `OST_TOKEN` env form keeps the
token out of argv/shell history):

```
curl -fsSL https://HOST/install.sh | sudo OST_TOKEN=<ENROLL_TOKEN> sh -s -- --server https://HOST
```

A console served over plain `http://` (trying it out at home) shows the same
command with `--insecure-http` on the end, and says why — `install.sh` refuses
plain http without it.

The script picks the desktop build when the machine has a graphical session
(`--headless` / `--desktop` force it), verifies the manifest's sha256, installs
to `/usr/local/bin/openscreentime`, then runs `enroll` + `install-service`. The
agent then updates itself from the same `/api/agent/latest` (docs/AGENT.md →
Self-update).

---

## Computers (devices)

| Method | Path                          | Notes                                                        |
|--------|-------------------------------|-------------------------------------------------------------|
| GET    | `/api/devices`                | → `{ devices: [...] }`; each with `status` (presence), `locked`, `lock_pending`, `last_state`, `owner_account_id`, `recovery_codes_unused`, users |
| GET    | `/api/devices/:id`            | detail incl. device users and recent events                  |
| POST   | `/api/devices`                | `{ name, account_id? }` → a `pending` device + a 24 h one-time enroll token → `{ device, enroll_token }`. `account_id` = "this is that person's computer". For a parent's own computer `428` unless the confirm window is open |
| PATCH  | `/api/devices/:id`            | `{ name?, tamper_level? }` — `tamper_level` is 1 or 3        |
| DELETE | `/api/devices/:id`            | remove it. Its device token is kept as a tombstone (`retired_devices`), so the agent hears `410 device_retired` and takes itself off the computer (see Agent API → Retirement). Each person's usage on it is kept (`retired_usage`): today stays true |
| POST   | `/api/devices/:id/enroll-token` | a fresh one-time token (24 h) → `{ device, enroll_token }`; 409 unless `pending`. Confirm-gated |
| POST   | `/api/devices/:id/lock`       | Pause: enqueue `lock` → `{ command_id, queued: true, delivered: bool }` |
| POST   | `/api/devices/:id/unlock`     | Resume: enqueue `unlock` (payload `{}`) → same shape         |
| POST   | `/api/devices/:id/ping`       | enqueue `ping`; the console reads the round trip off the command list ("Is it answering?") |
| PUT    | `/api/devices/:id/offline-window` | `{ minutes \| null }` — allowed to be offline for that long (null ends it); such a computer isn't flagged on the Family page |
| GET    | `/api/devices/:id/users`      | → `{ users: [...] }` (see below)                             |
| GET    | `/api/devices/:id/commands`   | the queue, pending first, then recent history. `login_code` commands are never listed |
| POST   | `/api/commands/:id/cancel`    | withdraw a command that hasn't been acked (best-effort once `sent`) |
| GET    | `/api/devices/:id/unlock-code` | confirm-gated → `{ code, seconds_left, period: 30, device_name }` |
| POST   | `/api/devices/:id/unlock-code/rotate` | confirm-gated; a new secret, recovery codes cleared → same shape + `recovery_codes_cleared: true` |
| GET    | `/api/devices/:id/recovery-codes` | confirm-gated → `{ unused, total, generated_at }`       |
| POST   | `/api/devices/:id/recovery-codes` | confirm-gated; replaces the set → `{ codes: ["1234 5678", …8], generated_at }`, plaintext exactly once |
| GET    | `/api/devices/:id/vpn`        | confirm-gated; the computer's VPN profiles, secrets masked   |
| POST   | `/api/devices/:id/vpn`        | confirm-gated; store a named WireGuard/OpenVPN profile (inactive) |
| PUT / DELETE | `/api/vpn-profiles/:id` | confirm-gated; edit through the mask / delete           |
| POST   | `/api/vpn-profiles/:id/activate`, `/deactivate` | confirm-gated; one active profile per computer |

The console has no tamper-level or VPN screens; those routes are API-only.

### Presence and pause state

`status` is presence only: `pending | online | offline`. A WS open marks the
computer online and a close offline at once; a sweep every 30 s marks `online`
computers whose `last_seen` is older than 90 s offline. `locked` is what the
agent last reported (its `state` frame: a lock intended **and** every present
managed user frozen); `lock_pending` means a `lock`/`unlock` is queued or sent.
Pause and Resume never flip anything themselves — the agent's ack or `state`
frame does, so the console shows "Pausing…" until the computer confirms.

## Device users & profile assignment

| Method | Path                                         | Notes                              |
|--------|----------------------------------------------|------------------------------------|
| GET    | `/api/devices/:id/users`                     | → `{ users: [{ id, device_id, os_username, display_name, profile_id, profile_name, profile_kind, used_minutes_today, earned_minutes_today }] }` (today's minutes joined from `screen_time_ledger`) |
| POST   | `/api/device-users/:id/assign-profile`       | `{ profile_id }` → `{ ok: true }`  |
| POST   | `/api/device-users/:id/assign-account`       | `{ account_id }` → `{ ok, removed_person }` — Who's who: move this OS login to another person; it takes that person's own rules (a parent or adult with none yet gets theirs now, from their bracket — an adult's enforce nothing). Confirm-gated. Pointing it at the computer's owner makes it the owner's login (`devices.owner_os_username`). The person the login was before, if now left with no login and no computer, is removed when the server made them up for an unsorted login and nobody has touched them since (`removed_person: true`); anyone else is kept, with no computer |
| POST   | `/api/device-users/:id/credit-time`          | `{ minutes: 1..=240 }` → `{ ok: true, minutes, answered: [request ids] }`; Give time: credits `screen_time_ledger.earned_seconds` and enqueues `credit_time` `{ os_username, minutes, request_id: null, day }`; audited as an `earn_request` event, `action: "granted"`. Giving time answers the person's asks: every request of theirs still pending (on any of their logins) becomes `approved` — without crediting it again — and is listed in `answered` |
| GET    | `/api/device-users/:id/usage`                | `?days=` (default 30, max 90) → per-day `{ day, used_minutes, earned_minutes }` for that login, plus a computed `streak` (the console doesn't show it) |

## Earn-time requests

Filed by the agent when someone asks for more time on their computer (the
request names the first earn task, else a plain 15 minutes), or by `/api/me/ask`
from their own page; answered by a parent on the Family page, from Telegram, or
through the companion API. One open request per (user, task) per day (agent-side duplicates return the
existing pending row). Requests and decisions are audited with `earn_request` events.

| Method | Path                              | Notes                                             |
|--------|-----------------------------------|---------------------------------------------------|
| GET    | `/api/earn-requests`              | `?status=pending` → `{ requests: [...] }` (joined with device name + user display name) |
| POST   | `/api/earn-requests/:id/approve`  | → `{ request }`; credits `screen_time_ledger.earned_seconds` and enqueues `credit_time` `{ os_username, minutes, request_id, day }`; the person's other pending requests are answered with it (approved, not credited again) |
| POST   | `/api/earn-requests/:id/deny`     | → `{ request }`; enqueues `deny_earn` `{ os_username, task_id, request_id }` so the agent clears its once-per-day dedupe and says "not this time" instead of "waiting" |

A request: `{ id, device_id, device_name, device_user_id, os_username, user_display_name, task_id,
task_label, minutes, status, created_at, decided_at }` with `status` one of
`pending | approved | denied` (409 when deciding an already-decided request).

## Profiles

| Method | Path                    | Notes                                         |
|--------|-------------------------|-----------------------------------------------|
| GET    | `/api/profiles`         | list: the five bracket presets, each person's own rules, custom and legacy rows |
| POST   | `/api/profiles`         | `{ name, kind:"custom", policy, parent_pin? }`; `parent_pin` (≥ 4 chars) is hashed (Argon2) into `policy.parent_pin_hash`, a legacy **backup code** the console no longer sets |
| GET    | `/api/profiles/:id`     |                                               |
| PUT    | `/api/profiles/:id`     | update the policy; `parent_pin` omitted keeps the hash, `""` clears it. Invalid windows, a whole-day bedtime or a non-IP `dns.upstream` → 400 |
| DELETE | `/api/profiles/:id`     | custom only                                   |

Someone's own profile — a parent's (the hub included) for themselves, or an
adult or `self_managed` member's (see "My rules" below) — is only theirs:
for anyone else, another parent included, `GET /api/profiles` and
`/api/family` leave it out, and `GET|PUT|DELETE /api/profiles/:id` on it →
`403 forbidden_for_member` "their rules are their own". They change it through
`/api/me/rules`.

At startup the server opens any pre-0.6 closed-network profile
(`dns.mode` or `firewall.mode` = `default_deny`) to `allow_all` (+ the `*`
allowlist, no outbound port list), keeping every block, and bumps its
`updated_at` so agents re-pull. Idempotent.

## My rules (self-control)

The hub for themselves, and any adult or `self_managed` member, sets their own
rules. A managed child or teen → `403 forbidden_for_member` "your rules are set
by a parent". Member sessions may reach both routes.

| Method | Path            | Notes |
|--------|-----------------|-------|
| GET    | `/api/me/rules` | `{ daily_limit_minutes, focus_hours, sites }` |
| PUT    | `/api/me/rules` | same shape in (a full replace), the normalized rules out |

```jsonc
{
  "daily_limit_minutes": 180,          // 0 = no limit; ≤ 1440
  "focus_hours": {                     // null = the sites are blocked all day
    "days": [1,2,3,4,5], "start": "09:00", "end": "12:00"
  },
  "sites": ["reddit.com", "youtube.com"]  // lower-cased, deduped; ≤ 200
}
```

Stored in the person's own profile (a preset or shared profile is copied first):
`screen_time.daily_limit_minutes` (+ `enabled`), and `policy.focus =
{ sites, hours }`. Focus hours read exactly like allowed hours (`00:00` end =
midnight, `00:00 – 00:00` = all day, an end before the start crosses midnight);
a window that can't mean anything → `400` with a plain message, as does a site
that isn't a domain. Their devices get `apply_policy`; the agent enforces it
like any policy. The audit event (`member`, `own_rules_changed`) records that
it changed — `daily_limit`/`focus_hours` as booleans and a count of `sites` —
never the sites themselves. `GET /api/me/today` adds `self_managed` and
`focus: { hours, sites }`.

## Events / audit

| Method | Path                | Notes                                             |
|--------|---------------------|---------------------------------------------------|
| GET    | `/api/events`       | `?device_id=&type=&severity=&limit=` → newest first; `limit` default 100, max 500 (no paging) |

Events under the login of someone a parent sees only the minutes of (an
adult, a co-parent, anyone self-managed — `usage.rs` `hub_exposure`) are left
out for everyone but that person, here and in `GET /api/devices/:id`'s
`recent_events`; events with no login (the computer's own) stay. Events
older than 90 days are pruned. `where the time went` and the rest of a
person's day are under "People" below.

## Companion API (`/api/parent/*`)

For a paired companion (the tray's parent mode, `ost pair`). A scoped bearer
token minted in the console; it reaches only these routes — not rules,
computers or settings.

| Method | Path | Notes |
|--------|------|-------|
| GET / POST | `/api/parent-tokens` | (console, confirm-gated) list / mint → the raw token exactly once |
| DELETE | `/api/parent-tokens/:id` | (console, confirm-gated) revoke |
| GET    | `/api/parent/earn-requests` | (bearer) pending requests |
| POST   | `/api/parent/earn-requests/:id/approve`, `/deny` | (bearer) answer one |
| GET    | `/api/parent/alerts` | (bearer) recent warnings and criticals |

## Telegram

| Method | Path | Notes |
|--------|------|-------|
| GET    | `/api/me/telegram` | confirm-gated; pairing state |
| POST   | `/api/me/telegram/pair` | confirm-gated; a short single-use code and a deep link to the bot |
| DELETE | `/api/me/telegram` | confirm-gated; unpair every chat of this account |

---

## Agent API

Auth: `Authorization: Bearer <device_token>` unless noted.

### Enrollment
```
POST /agent/enroll
Body: { enroll_token, hostname, os, agent_version, os_users: [{ username, display_name }],
        installer?, owner_login?, machine_id? }
→ 200 { device_id, device_token, poll_interval_secs, users: [{ os_username, person, parent }] }
```
The `enroll_token` is spent once and expires 24 h after issue; an expired or
spent token is a 401. A retry with the same token from the same host within
15 minutes gets the same enrollment back (a lost reply doesn't strand the
install). Each reported login is linked to a person — see docs/AUTH.md "Whose
login is whose"; an unsorted login gets Kid rules on a child's computer and
rules that enforce nothing on a parent's own.

**The same machine, enrolled again.** `machine_id` says which machine this
is: HMAC-SHA256 keyed with `/etc/machine-id`, over `openscreentime-machine:`
and the household's `machine_salt` from the preview — never the id itself,
and nothing another household can match (64 lower-case hex; anything else is
ignored). When the household already has a record of that machine (the
one-liner run again with a new token — "Add my computer" on what was "Mia's
computer"), the older record is **folded into the new one** instead of
standing beside it: each login keeps its person (a login sorted by hand
stays sorted; the new owner's login goes to the new owner), each login's
ledger days move over (the larger count wins — the agent's own ledger has the
same minutes), and so do where the time went, the moments and any ask still
waiting. The older record goes; its token becomes a tombstone that points at
the new record, so the agent still running with it until the installer
restarts it is heard as the new record — never `410` (see below), which would
make it take itself off the computer it was just enrolled on. The `enrolled`
event lists what it replaced (`took_over: [name]`). A person's day is counted
once. An agent that sends no `machine_id` (older than 0.7) enrolls as before.

### Retirement (a removed computer)

Every `/agent/*` call — and the `/agent/ws` upgrade — with the token of a
computer that was removed (`DELETE /api/devices/:id`) is answered

```
410 { "error": { "code": "device_retired", "message": "…" }, "retired": true }
```

and never with a plain 401. On that answer, from its configured server and
confirmed by a second request, the agent thaws everyone, takes the lock down,
removes its nft table, the resolv.conf pin (the computer's previous DNS comes
back — systemd-resolved's link included), its dnsmasq include, the polkit
rule, the unlock-code sudo and its cached secrets; then (`ost __retire`,
outside the sandbox) it disables and removes its units, stops the companion
and any app window for everyone signed in, and removes the rest of it: the
config and enrollment, the state (ledger, unlock-code state), the runtime
files and the binary. Packages it installed (dnsmasq, nftables, cage) stay,
with their own config back; a dnsmasq it brought is left disabled. A 401, a
network error, or a 410 without `"retired": true` never does this — the
agent keeps its last rules. Installing again (the one-liner) starts afresh. The token of a
record that was folded into a newer one (above) answers as that newer record
until it is removed; then it too is `410`.

Removing a computer doesn't take anyone's day with it: its ledger is kept
under each person (`retired_usage`) and still counts on the console, in their
week and as time used elsewhere on their other computers. Enrolling the same
machine again takes those minutes back onto its logins.

### Heartbeat (poll model, fallback for WS)
```
POST /agent/heartbeat
Body: { status, public_ip?, os_users: [...], features?, state?,
        usage: [{ os_username, used_minutes_today, used_seconds_today?, day?, utc_offset_secs? }] }
→ 200 { commands: [Command...], policy_version: string,
        usage: [PersonDay...], server_time: RFC3339 }

PersonDay = { os_username, day, used_elsewhere_secs, earned_elsewhere_secs, earned_here_secs }
```
`usage` is **this device's own** use today. It is filed in `screen_time_ledger` under the
**device-local `day`** the agent enforces (0.7+; an implausible or missing day falls back to the
device's local date from `utc_offset_secs`, else UTC), `GREATEST`-clamped within that day. A
drop of more than 300 s within the *same* day raises one critical `evasion` / `usage_regression`
event per device user per day (never for an agent that doesn't send `day`). The reply's
`PersonDay` is what the same person used and was granted on their **other** logins that day —
a daily limit is one budget per person — plus the grants on record for this login
(`earned_here_secs`; the agent takes the larger of that and its own count). `server_time` is a
clock the agent trusts when its own isn't NTP-synchronized. See `docs/TRACKING.md`.
Agent acks commands via `POST /agent/commands/:id/ack { status, result }`.

`credit_time` commands carry `{ os_username, minutes, request_id, day }` (`day` = the device-local
day the grant was filed under). The agent applies a grant once per command id (a redelivery is
acked `{ credited: true, duplicate: true }`), ignores one for an earlier day
(`{ credited: false, stale_day }`), and turns it into N minutes on today's budget plus an override
for N minutes. The agent's `unlock` also understands an explicit grant, `{ os_username, minutes }`
or `{ os_username, until: "end_of_day" }`, but the console's Resume always sends `{}`: it ends the
pause and gives nobody time — someone whose own rules stop them (time's up, bedtime) stays stopped.
Time is given with `credit_time`.

**Commands** (`commands.type`): `lock` (`{}`, or `{ reason, grace_secs }`
when an account is suspended), `unlock`, `apply_policy`, `set_tamper_level
{ level }`, `credit_time`, `deny_earn`, `login_code` (below), `ping`.

### Earn-time request
```
POST /agent/earn-request
Body: { os_username, task_id, task_label, minutes }   // 1 <= minutes <= 240
→ 200 { request: { id, status: "pending", ... } }
```
Deduped per (user, task, day): a repeat while today's request is still pending returns the
existing row. "Ask for more time" (the lock, the app, the companion, `ost ask`) files a plain
ask — `task_id: "ask"`, `task_label: "Asked for more time"`, 15 minutes — the same words as
`/api/me/ask`, never an earn task the person didn't pick.

### Policy pull
```
GET /agent/policy
→ 200 { policy_version, device_tamper_level,
        users: [{ os_username, profile_kind, policy: Policy }],
        parent_code: { totp_secret, recovery_codes: [{ id, mac }] },
        vpn: { … } | null }
```
`parent_code` is what the agent checks unlock codes against offline (unused
recovery codes only; `mac` = hex HMAC-SHA256 keyed by the decoded secret over
the 8 digits). An agent event `parent_code_backup_used { recovery_id }` retires
one. `vpn` is the computer's active VPN profile, if any.

### Usage slices
```
POST /agent/usage
Body: { slices: [{ os_username, hour, kind: "app" | "site", key, amount }] }   // ≤ 500
```
Where the time went: seconds an app was open per login per hour (`kind: "app"`),
and lookups per site per hour for the whole computer (`kind: "site"`,
`os_username: ""`). Summed server-side, kept 21 days.

### Events push
```
POST /agent/events
Body: { events: [{ id?, type, severity, device_user?, payload }] }   // ≤ 100 per batch
→ 202
```
`id` is the agent's own event id; a redelivered event with the same id is
stored once.
The agent posts *all* events this way, in both WS and poll mode — there is no separate "event
delivery only over WS" path. Batches that fail to POST (server unreachable, etc.) are buffered in
memory (`client/src/runner.rs` `flush_queued`, capped) and retried by the network loop rather than
dropped. The WS `event` frame (see below) is still accepted by the server for compatibility but is
not how the current agent sends events.

### WebSocket bus (preferred transport)
```
GET /agent/ws   (Upgrade)
```
Bidirectional JSON frames, tagged with `"type"`:

- server → agent: `command { command }`, `ping` (keepalive),
  `usage { server_time, users: [PersonDay...] }` (the reply to each `heartbeat` frame — see the
  HTTP heartbeat above; agents before 0.7 ignore it)
- agent → server: `ack { ack }`, `state { state: { locked, lock_intent, frozen_users,
  enforcing, gaps, agent_version, active_users, features } }` (on connect, on change,
  and at least every 60 s), `heartbeat { usage }` (same entries as the HTTP heartbeat,
  every 30 s), `pong`. `event { event }` is still accepted but the agent sends events
  over HTTP.

The server pings every few seconds and drops a socket silent for 60 s. The
agent falls back to heartbeat polling while the WS is down.

---

## Policy (the jsonb document)

The shared type (`policy/src/lib.rs`); field by field in docs/PROFILES.md.
The Kid preset, as the server seeds it:

```jsonc
{
  "version": 1,
  "dns": { "mode": "allow_all", "allowlist": ["*"], "blocklist": [],
           "safe_search": true, "upstream": "1.1.1.3" },   // upstream: a literal IP
  "firewall": { "mode": "allow_all", "allow_outbound_ports": [], "allow_inbound_ports": [22] },
  "screen_time": {
    "enabled": true,
    "daily_limit_minutes": 60,                              // 0 = no limit
    "schedule": [                                           // allowed windows; 0 = Sunday
      { "days": [1,2,3,4,5], "start": "07:00", "end": "20:00" },
      { "days": [0,6],       "start": "09:00", "end": "20:00" }
    ],
    "bedtime": { "start": "20:00", "end": "07:00" }
  },
  "gamification": {
    "earn_time": { "enabled": true, "tasks": [
      { "id": "reading", "label": "Read for 20 min", "reward_minutes": 15 },
      { "id": "chores",  "label": "Finish chores",   "reward_minutes": 15 } ] },
    "lockout": { "enabled": true, "unlock_challenge": "parent_pin" }  // ignored by the agent
  },
  "lockdown": { "force_dns": true, "block_doh": true, "block_dot": true,
                "block_tor": true, "block_vpn": false, "offline_lockdown_days": 0 },
  "blocks": { "apps": [], "categories": ["adult","gambling","dating","proxies"],
              "custom_domains": [] }
}
```

`lockdown`, `blocks`, `focus` (a self-managed person's own sites and hours)
and `parent_pin_hash` are absent when empty. The agent adds `focus.sites` to
the computer's blocks while `rules::focus_blocking` holds; a focus window never
stops a screen. `mode: "default_deny"` still parses, but the server opens any
profile using it at startup.

Every component MUST treat unknown fields leniently (forward-compat). The Rust side models this
with `#[serde(default)]` on optional sub-objects.


## People

**Accounts.** `admins` rows carry `role` (`owner|parent|member`), `age_bracket`
(`little|kid|younger_teen|older_teen|adult`), `birthdate`, `theme`
(`playful|calm|plain`, null = auto by bracket), `self_managed`, `profile_id`.

- `GET /api/me` → `{ account: {id, household_id, tenant_id, display_name,
  username, email, role, age_bracket, birthdate, theme, effective_theme,
  self_managed, profile_id, avatar, goal_minutes, blocked, created_at},
  household: {id, name, created_at}, admin, tenant }` (the last two are
  deprecated aliases).
- `GET /api/members` (hub) → `{ members: [account…] }`
- `POST /api/members {display_name, birthdate?, age_bracket?, theme?, email?}`
  → `{ member }` — rules cloned from the bracket preset into a profile owned by
  the person. Bracket is derived from `birthdate` when given; default `kid`.
- `PATCH /api/members/{id} {display_name?, birthdate?, age_bracket?, theme?,
  profile_id?}` → `{ member }`. `profile_id` re-points all of the person's
  `device_users` and queues `apply_policy` on their devices.
- `DELETE /api/members/{id}` (members only).
- `POST /api/members/{id}/block` / `unblock` (members only) — suspend an
  account: it can't sign in, live sessions end, and its computers get `lock`
  `{ reason: "paused_by_parent", grace_secs: 120 }`. Unblock doesn't resume the
  computers. The console only offers "Lift the block" for an old block.
- `GET /api/me/today` → `{ used_minutes, earned_minutes, limit_minutes|null,
  left_minutes|null, rules, locked, devices:[{id,name,status,locked,gaps?}], blocks,
  blocked_apps:[app id], bracket, theme, can_ask, pending_request, bedtime,
  windows, display_name, utc_offset_secs }`. `gaps` (only for someone who
  sets their own rules): what an online computer says it can't do right now
  (`last_state.gaps`), so their page never promises a block a computer can't
  keep. The day includes computers that were removed (`retired_usage`).
  "Today" is each device's own
  local day (the day its agent enforces); `utc_offset_secs` is that
  computer's clock, for everything the console says about its day (focus
  hours, the week, the hours strip). `left_minutes` is **time left**, the
  number the computer shows: the agent's own rules function with the same
  inputs — the day's use and grants and the override the computer reports in
  its `state` frame — so minutes until the screen stops (the budget,
  bedtime, the end of the hours or of an override, whichever first); `0` =
  stopped, `null` = no limit. `rules` = `{ allowed, reason:
  "limit"|"bedtime"|"outside_hours"|null, minutes_left, stop_at, resume_at,
  override_until, utc_offset_secs }` is the same verdict, times in the
  computer's offset; when `stop_at` is `override_until`, an unlock code or a
  grant is what keeps them going ("Unlocked until 00:27"); plus `goal_minutes`, and
  for a self-managed person `self_managed` and `focus: { hours, sites }`.
  `parent_sees: { apps, sites }` is what a parent sees of this person's day
  besides the minutes — the same rule `/api/usage/where` enforces
  (`usage.rs` `hub_exposure`); both false = minutes only. The page's "What
  can a parent see?" is said from it.
  `GET /api/family` children carry the same `left_minutes`, `rules` and
  `utc_offset_secs`.
- `GET /api/me/history` → the last 14 days `{ days: [{ day, used_minutes,
  earned_minutes }], today_by_device: [{ name, used_minutes }], goal_minutes,
  goal_streak }` (the console shows neither goal nor streak).
- `POST /api/me/goal { minutes }` (0 or absent clears it) — the person's own daily goal (API
  only; the console no longer sets one).
- `GET /api/me/where` — where your own time went today (below).
- `POST /api/me/ask {minutes, reason?}` → `{ request }` (an `earn_request`
  with `task_id: "ask"`, one open per day; no confirm).
- `GET /api/catalog` → `{ categories:[{id,name,blurb,app_ids}],
  apps:[{id,name,category,has_native_client}] }`.
- **Member sessions** may reach only `/api/me`, `/api/me/today`,
  `/api/me/history`, `/api/me/where`, `/api/me/goal`, `/api/me/ask`,
  `/api/me/rules`, `/api/catalog` and `/api/auth/*`. Anything else under
  `/api/` → `403 forbidden_for_member` (a layer; fails closed for new routes).
- **A suspended (blocked) account** can read but not change anything (`403`).

**Where the time went.** `GET /api/usage/where?account_id=` (the hub) and
`GET /api/me/where` (yourself) → `{ apps: [{ key, seconds }], sites: [{ key,
hits }], hours: [{ hour, amount }], sites_hidden_shared, sites_hidden_age }`,
today, top 12 each. What the hub gets depends on the person's bracket
(`usage.rs` `Exposure`): little, kid and younger teen → apps, hours and sites;
older teen → apps and hours, `sites_hidden_age: true`; adult or self-managed →
`403 forbidden_for_member`. Your own view hides the site list when one of your
computers is shared with someone else (`sites_hidden_shared`).

**Unlock codes.** Each computer's TOTP secret is held by the server and its
agent only; a parent reads the live code off the console (the routes under
Computers above), and rotating the secret clears the recovery codes, which are
keyed by it. Both queue `apply_policy`. A profile's `parent_pin_hash` is still
served as a legacy backup code.

**Voucher.** `POST /agent/voucher {os_username}` → voucher bound to the account
linked to that OS login (`404 no_account` if none). `POST /api/auth/voucher
{voucher}` → session for that account, `{ ok, via, account_id, role }`.

**Family.** `GET /api/family` → `{ children, devices, profiles, requests,
server_time }` — the whole home screen in one request. `children` are the
**members** (`key` = `account_id` = `id`, the account id every per-person call
addresses) with the account fields plus `name, avatar,
used_minutes, earned_minutes, limit_minutes, left_minutes, rules, goal_minutes,
profile_name, devices:[{device_user_id,id,name,status,locked,lock_pending,
os_username}], pending_requests, locked, blocked, blocks, blocked_apps,
can_ask, managed, self_managed`. `devices` carry `pending_commands` and
`unsorted_logins`. Parents aren't in `children`.
For an adult or `self_managed` member the hub gets their minutes but not their
rules: `limit_minutes`, `left_minutes` and `rules` are null, `blocks` and
`blocked_apps` empty, and their profile is not in `profiles`.
