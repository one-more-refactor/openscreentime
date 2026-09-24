# API Contract

Two surfaces:

- **Admin API** (`/api/*`) — used by the web control center. Authenticated with a session
  cookie issued after a passkey login.
- **Agent API** (`/agent/*`) — used by the Linux agent. Authenticated with a bearer
  `device_token` (except enrollment, which uses a one-time `enroll_token`).

All request/response bodies are JSON. Errors use `{ "error": { "code": string, "message": string } }`
with an appropriate HTTP status.

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
| POST   | `/api/auth/code/verify`     | `{ request_id, code_verifier, code }` → session; `401 wrong_code` (type again) or `410 code_expired` (5 min / 5 tries) |
| POST   | `/api/auth/login/start`     | → `RequestChallengeResponse` for a discoverable passkey (no name) |
| POST   | `/api/auth/login/finish`    | `{ credential }` → session, `{ admin }`                 |
| GET    | `/api/auth/oidc/start`      | 302 to the provider's authorize URL                     |
| GET    | `/api/auth/oidc/callback`   | `?code&state` → session + redirect `/` (see below)      |
| POST   | `/api/auth/voucher`         | `{ voucher }` → session (`ost login`, 7 days)           |
| POST   | `/api/auth/link`            | `{ token }` → session (recovery link from `openscreentime-server recover`) |
| POST   | `/api/auth/logout`          | clears session (deletes the DB row)                     |
| GET    | `/api/me`                   | → `{ account, household, admin, tenant }`               |
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

### Agent

| POST   | `/agent/voucher`            | mint a one-time (2 min) voucher for a local surface on that machine to exchange at `/api/auth/voucher` |
| POST   | `/agent/enroll/preview`     | `{ enroll_token }` → `{ owner, owner_is_parent }` without using the token (so `ost enroll` can ask which login is the owner's) |

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
The callback matches the verified userinfo email against existing admins (any tenant). Fresh
installs (no admins at all) bootstrap a tenant + admin; an unknown email on a non-empty install
redirects to `/login?error=sso_unknown_account` (no auto-provisioning); other failures redirect
to `/login?error=sso_failed`.

### Rate limiting

Fixed-window, in-memory, per client IP (**last** `X-Forwarded-For` value when
`OST_TRUST_PROXY=1`, else the peer address). A trusted reverse proxy appends the real peer
IP to the end of XFF, so the last hop is the only element the client can't forge — keying on the
first value would let an attacker rotate `X-Forwarded-For` per request and land each one in a
fresh bucket, defeating the limiter entirely. Over-limit requests get a 429 error envelope.
`OST_TRUST_PROXY` defaults to `1` in the prod compose stack (`compose.yaml`), since the
supported deploy always sits behind the bundled reverse proxy.

- auth attempt endpoints (register/login/OIDC start + finish): 10 req / 60 s / IP
- `/agent/enroll`: 5 req / 60 s / IP
- agent distribution (`/install.sh`, `/api/agent/latest`, `/api/agent/download/:file`): 30 req / 60 s / IP

---

## Agent distribution (public, no auth)

The production image bundles the headless musl-static agent under `/app/agent`
(`OST_AGENT_DIR`); a dev `cargo run` has no bundle and these return 404. The binary is
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

The script verifies the manifest's sha256 before installing to
`/usr/local/bin/openscreentime`, then runs `enroll` + `install-service`. The installed agent
self-updates from `/api/agent/latest` daily (agent.toml `auto_update = true` by default;
`OST_NO_SELF_UPDATE=1` disables) — trust model in docs/CONTRACT-PROD.md §13.

---

## Devices

| Method | Path                          | Notes                                                        |
|--------|-------------------------------|-------------------------------------------------------------|
| GET    | `/api/devices`                | list devices for tenant (+status, last_seen, users, per-device `online: bool`) |
| GET    | `/api/devices/:id`            | detail incl. device_users, recent events, `online: bool`     |
| POST   | `/api/devices`                | `{ name, account_id? }` → creates `pending` device + 24 h TTL enroll token → `{ device, enroll_token }`; for a parent's own computer (`account_id` = a parent) `428` unless the confirm window is open |
| PATCH  | `/api/devices/:id`            | rename, set `tamper_level`                                   |
| POST   | `/api/devices/:id/enroll-token` | regenerate the one-time enroll token (fresh 24 h TTL) → `{ device, enroll_token }`; 409 unless status is `pending` |
| POST   | `/api/devices/:id/lock`       | enqueue `lock` command → `{ command_id, queued: true, delivered: bool }` |
| POST   | `/api/devices/:id/unlock`     | enqueue `unlock` command → same response shape as lock       |
| DELETE | `/api/devices/:id`            | de-enroll                                                    |

### Truthful lock state

`devices.status` only flips to `locked`/`online` when the lock/unlock actually takes effect:
immediately when the command was pushed to a live agent WS (`delivered: true`), otherwise the
command stays queued (`delivered: false`) and the status flips when the agent reconnects and
**acks** the command. The UI shows a "LOCK PENDING" chip for queued locks.

### Offline sweeper

A background task (every 60 s) marks devices `offline` whose `status = 'online'` and
`last_seen` is older than 3 minutes — this catches dead poll-mode agents that never had a WS
disconnect. `locked` and `pending` are never touched. The web UI escalates devices offline
for 7+ days to a red "GONE DARK Nd" badge (tamper signal).

## Remote SSH — removed

The remote-shell feature (browser terminal, `/api/devices/:id/ssh`, `/api/ssh/*` routes)
was removed in v0.4 — everything a parent can do is UI-only now. Historical events of
`type = 'ssh'` remain readable in the event log as the record of past sessions.

## Device users & profile assignment

| Method | Path                                         | Notes                              |
|--------|----------------------------------------------|------------------------------------|
| GET    | `/api/devices/:id/users`                     | → `{ users: [{ id, device_id, os_username, display_name, profile_id, profile_name, profile_kind, used_minutes_today, earned_minutes_today }] }` (today's minutes joined from `screen_time_ledger`) |
| POST   | `/api/device-users/:id/assign-profile`       | `{ profile_id }` → `{ ok: true }`  |
| POST   | `/api/device-users/:id/credit-time`          | `{ minutes: 1..=240 }` → `{ ok: true, minutes }`; parent grants extra screen time today: credits `screen_time_ledger.earned_seconds` and enqueues a `credit_time` command `{ os_username, minutes, request_id: null }`; audited as an `earn_request` event with `action: "granted"` |

## Earn-time requests

Filed by the agent when a user picks an earn offer on the lockout screen; decided by a parent
in the web UI. One open request per (user, task) per day (agent-side duplicates return the
existing pending row). Requests and decisions are audited with `earn_request` events.

| Method | Path                              | Notes                                             |
|--------|-----------------------------------|---------------------------------------------------|
| GET    | `/api/earn-requests`              | `?status=pending` → `{ requests: [...] }` (joined with device name + user display name) |
| POST   | `/api/earn-requests/:id/approve`  | → `{ request }`; credits `screen_time_ledger.earned_seconds` and enqueues a `credit_time` command `{ os_username, minutes, request_id }` |
| POST   | `/api/earn-requests/:id/deny`     | → `{ request }`; enqueues a `deny_earn` command `{ os_username, task_id, request_id }` so the agent clears its once-per-day dedupe and replaces the stale "WAITING FOR APPROVAL" copy with an honest answer |

A request: `{ id, device_id, device_name, device_user_id, os_username, user_display_name, task_id,
task_label, minutes, status, created_at, decided_at }` with `status` one of
`pending | approved | denied` (409 when deciding an already-decided request).

## Profiles

| Method | Path                    | Notes                                         |
|--------|-------------------------|-----------------------------------------------|
| GET    | `/api/profiles`         | list (3 presets + custom)                     |
| POST   | `/api/profiles`         | `{ name, kind:"custom", policy, parent_pin? }` — `parent_pin` (string, min 4 chars) is optional; hashed server-side (Argon2) into `policy.parent_pin_hash`; omitted = no PIN |
| GET    | `/api/profiles/:id`     |                                               |
| PUT    | `/api/profiles/:id`     | update policy (presets are cloneable, editable); accepts optional `parent_pin` — omitted preserves the existing hash, empty string `""` clears it, non-empty (min 4 chars) sets a new hash |
| DELETE | `/api/profiles/:id`     | custom only                                   |

A self-managed person's own profile (an adult or `self_managed` member's
rules, see "My rules" below) is not the hub's: `GET /api/profiles` and
`/api/family` leave it out, and `GET|PUT|DELETE /api/profiles/:id` on it →
`403 forbidden_for_member` "their rules are their own".

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

## Discovery

| Method | Path                    | Notes                                                       |
|--------|-------------------------|-------------------------------------------------------------|

## Events / audit

| Method | Path                | Notes                                             |
|--------|---------------------|---------------------------------------------------|
| GET    | `/api/events`       | `?device_id=&type=&severity=&limit=` paginated    |

---

## Agent API

Auth: `Authorization: Bearer <device_token>` unless noted.

### Enrollment
```
POST /agent/enroll
Body: { enroll_token, hostname, os, agent_version, os_users: [{ username, display_name }] }
→ 200 { device_id, device_token, poll_interval_secs }
```
The `enroll_token` is consumed (single use) and expires 24 h after issue
(`devices.enroll_token_expires_at`); an expired token is rejected exactly like a consumed one
(401). While the device is still `pending`, an admin can regenerate a fresh token via
`POST /api/devices/:id/enroll-token`. Server creates `device_users` rows for reported
`os_users`, each assigned the tenant's **default** profile until an admin changes it.

### Heartbeat (poll model, fallback for WS)
```
POST /agent/heartbeat
Body: { status, public_ip?, os_users: [...],
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
for N minutes. `unlock` accepts an optional `{ minutes }` or `{ until: "end_of_day" }` (and
`os_username`) to hold the screen-time rules off; without them, whoever a rule is stopping gets
30 minutes.

### Earn-time request
```
POST /agent/earn-request
Body: { os_username, task_id, task_label, minutes }   // 1 <= minutes <= 240
→ 200 { request: { id, status: "pending", ... } }
```
Deduped per (user, task, day): a repeat while today's request is still pending returns the
existing row.

### Policy pull
```
GET /agent/policy
→ 200 { policy_version, device_tamper_level, users: [{ os_username, profile_kind, policy: Policy }] }
```

### Events push
```
POST /agent/events
Body: { events: [{ type, severity, device_user?, payload }] }
→ 202
```
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
- agent → server: `event { event }` (accepted for compatibility; the agent now sends events over
  HTTP, see below), `ack { ack }`, `state { … }`, `heartbeat { usage }` (same `usage` entries as
  the HTTP heartbeat, every 30 s), `pong`

Falls back to heartbeat polling if WS is unavailable.

---

## Policy (the jsonb document)

This is the single most important shared type. Server stores it, web edits it, agent enforces it.

```jsonc
{
  "version": 1,
  "dns": {
    "mode": "default_deny",          // zero-trust: block unless allowed
    "allowlist": ["school.edu", "wikipedia.org"],
    "blocklist": [],                 // extra explicit blocks (redundant under default_deny)
    "safe_search": true,
    "upstream": "1.1.1.2"            // filtered upstream resolver
  },
  "firewall": {
    "mode": "default_deny",
    "allow_outbound_ports": [53, 80, 443],
    "allow_inbound_ports": []
  },
  "screen_time": {
    "enabled": true,
    "daily_limit_minutes": 120,
    "schedule": [                     // allowed windows, per weekday (0=Sun)
      { "days": [1,2,3,4,5], "start": "15:00", "end": "20:00" },
      { "days": [0,6],       "start": "09:00", "end": "21:00" }
    ],
    "bedtime": { "start": "21:00", "end": "07:00" }
  },
  "gamification": {
    "earn_time": {
      "enabled": true,
      "tasks": [
        { "id": "reading", "label": "Read for 20 min", "reward_minutes": 15 }
      ]
    },
    "lockout": {
      "enabled": true,
      "unlock_challenge": "math"      // "math" | "wait" | "parent_pin"
    }
  },
  "focus": {                          // optional; a self-managed person's own (/api/me/rules)
    "sites": ["reddit.com"],          // blocked for themselves…
    "hours": { "days": [1,2,3,4,5], "start": "09:00", "end": "12:00" }  // …inside these; null = all day
  }
}
```

`focus` is absent when empty. The agent adds `focus.sites` to the host's
blocks while `rules::focus_blocking` holds and removes them when the window
ends; a focus window never stops a screen.

Every component MUST treat unknown fields leniently (forward-compat). The Rust side models this
with `#[serde(default)]` on optional sub-objects.


## 0.4 additions (docs/CONTRACT-0.4.md)

**Accounts.** `admins` rows carry `role` (`owner|parent|member`), `age_bracket`
(`little|kid|younger_teen|older_teen|adult`), `birthdate`, `theme`
(`playful|calm|plain`, null = auto by bracket), `self_managed`, `profile_id`.

- `GET /api/me` → `{ account: {id, household_id, display_name, email, role,
  age_bracket, birthdate, theme, effective_theme, self_managed, profile_id,
  created_at}, household: {id, name, created_at}, admin, tenant }` (the last two
  are deprecated aliases).
- `GET /api/members` (hub) → `{ members: [account…] }`
- `POST /api/members {display_name, birthdate?, age_bracket?, theme?, email?}`
  → `{ member }` — rules cloned from the bracket preset into a profile owned by
  the person. Bracket is derived from `birthdate` when given; default `kid`.
- `PATCH /api/members/{id} {display_name?, birthdate?, age_bracket?, theme?,
  profile_id?}` → `{ member }`. `profile_id` re-points all of the person's
  `device_users` and queues `apply_policy` on their devices.
- `DELETE /api/members/{id}` (members only).
- `GET /api/me/today` → `{ used_minutes, earned_minutes, limit_minutes|null,
  left_minutes|null, rules, locked, devices:[{id,name,status,locked}], blocks,
  blocked_apps:[app id], bracket, theme, can_ask, pending_request, bedtime,
  windows, display_name }`. "Today" is each device's own local day (the day
  its agent enforces); `left_minutes` is the person's budget left computed
  like the device does (seconds, rounded up). `rules` = `{ allowed, reason:
  "limit"|"bedtime"|"outside_hours"|null, minutes_left, stop_at, resume_at }`
  from the agent's own rules function — when screens stop, whichever of the
  budget, bedtime or the window end comes first. `GET /api/family` children
  carry the same `left_minutes` and `rules`.
- `POST /api/me/ask {minutes, reason?}` → `{ request }` (an `earn_request`
  with `task_id: "ask"`, one open per day; not step-up guarded).
- `GET /api/catalog` → `{ categories:[{id,name,blurb,app_ids}],
  apps:[{id,name,category,has_native_client}] }`.
- **Member sessions** may reach only `/api/me`, `/api/me/today`, `/api/me/ask`,
  `/api/me/rules`, `/api/catalog`, `/api/auth/*` (and a few more `/api/me/*` reads). Anything else under `/api/` →
  `403 forbidden_for_member` (a layer; fails closed for new routes).

**Unlock code (per-device TOTP) and recovery codes.** The secret behind the
code is held by the server and the agent only; a parent reads codes off the
console (0.5, `docs/CONTRACT-0.5.md` §1).
- `POST /api/devices {name, account_id?}` → `{ device, enroll_token }`.
  `account_id` = "this is <person>'s computer": OS logins without a name match
  link to that person on enroll.
- `GET /api/devices/{id}/unlock-code` (sensitive read → 428 without change
  mode) → `{ code, seconds_left, period: 30, device_name }` — the 6 digits that
  open that computer right now.
- `POST /api/devices/{id}/unlock-code/rotate` → same shape plus
  `recovery_codes_cleared: true`; new secret, recovery codes deleted (they are
  keyed by it), queues `apply_policy`.
- `POST /api/devices/{id}/recovery-codes` → `{ codes: ["1234 5678", …8],
  generated_at }` — replaces the set; plaintext exactly once. Queues
  `apply_policy`. `GET` (sensitive read) → `{ unused, total, generated_at }`.
- Device JSON everywhere carries `recovery_codes_unused` (0 = none generated
  or all spent).
- Agent pull `GET /agent/policy` adds top-level
  `parent_code: { totp_secret, recovery_codes: [{id, mac}] }` (unused only;
  `mac` = hex HMAC-SHA256 keyed by the decoded secret over the 8 digits). An
  agent event `parent_code_backup_used {recovery_id}` retires that code. A
  profile-level `parent_pin_hash` is still served as a **backup code**; the
  enroll-time device recovery PIN is no longer minted.

**Presence.** Device JSON everywhere: `status` is presence only
(`pending|online|offline`); `locked` (bool) is what the agent last reported;
`lock_pending` (bool) = a `lock`/`unlock` command is queued or sent;
`last_state` = the agent's last `state` frame; `owner_account_id`. Lock/unlock
no longer flip any status — the agent's ack or `state` frame does.
- WS `{ type:"state", locked, frozen_users, enforcing, gaps, agent_version,
  active_users }` (also accepted nested under `state`, and as `state` inside
  an HTTP/WS `heartbeat`). WS open → online; WS close → offline immediately;
  sweep: `online` + `last_seen` older than 90 s → offline.

**Voucher.** `POST /agent/voucher {os_username}` → voucher bound to the account
linked to that OS login (`404 no_account` if none). `POST /api/auth/voucher
{voucher}` → session for that account, `{ ok, via, account_id, role }`.

**Family.** `GET /api/family` children are **members** (key = account id) with
the account fields plus `name, used_minutes, earned_minutes, limit_minutes,
profile_name, devices:[{device_user_id,id,name,status,locked,lock_pending,
os_username}], pending_requests, locked, blocks, blocked_apps, can_ask, managed`.
For an adult or `self_managed` member the hub gets their minutes but not their
rules: `limit_minutes`, `left_minutes` and `rules` are null, `blocks` and
`blocked_apps` empty, and their profile is not in `profiles`.

### `POST /api/device-users/{id}/assign-account` (0.4)

Body `{ "account_id": "<uuid>" }`. Moves an OS login to another person in the
household; the login takes that person's rules (`profile_id` follows) and the
agent is told to re-pull. Needs the confirm window (`428` without). Pointing
a login at the computer's owner makes it the owner's login
(`devices.owner_os_username`) — how a parent settles theirs on their own
computer.
