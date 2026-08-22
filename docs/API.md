# API Contract

Two surfaces:

- **Admin API** (`/api/*`) — used by the web control center. Authenticated with a session
  cookie issued after a passkey login.
- **Agent API** (`/agent/*`) — used by the Linux agent. Authenticated with a bearer
  `device_token` (except enrollment, which uses a one-time `enroll_token`).

All request/response bodies are JSON. Errors use `{ "error": { "code": string, "message": string } }`
with an appropriate HTTP status.

Base URL in dev: `http://localhost:8080`.

`GET /health` — unauthenticated liveness check → `{ "status": "ok", "service":
"openscreentime-server" }`.

---

## Auth (passkey / WebAuthn + optional OIDC SSO)

Uses `webauthn-rs`. Registration is first-boot only: while zero admins exist, any email can
register the first admin (bootstrapping the tenant). Once at least one admin exists,
`register/start` and `register/finish` refuse with **403 `{ error: { code:
"registration_closed" } }`** unless `OST_OPEN_REGISTRATION=1` is set (see
docs/DEPLOY.md). A logged-in admin adding another passkey to their *own* account (the
Settings page reuses the register ceremony) is always allowed.

| Method | Path                        | Body / Notes                                            |
|--------|-----------------------------|---------------------------------------------------------|
| GET    | `/api/auth/config`          | public → `{ auth: { oidc: bool, oidc_name } }`          |
| POST   | `/api/auth/register/start`  | `{ email, display_name }` → `CreationChallengeResponse` |
| POST   | `/api/auth/register/finish` | `{ email, credential }` → sets session, `{ admin }`     |
| POST   | `/api/auth/login/start`     | `{ email }` → `RequestChallengeResponse`                |
| POST   | `/api/auth/login/finish`    | `{ credential }` → sets session cookie, `{ admin }`     |
| GET    | `/api/auth/oidc/start`      | 302 to the provider's authorize URL                     |
| GET    | `/api/auth/oidc/callback`   | `?code&state` → session + redirect `/` (see below)      |
| POST   | `/api/auth/logout`          | clears session (deletes the DB row)                     |
| GET    | `/api/me`                   | → `{ admin, tenant }`                                   |
| GET    | `/api/me/2fa`               | → `{ totp_enrolled, email_available, locked_until }`     |
| POST   | `/api/me/2fa/totp/start`    | → `{ secret, otpauth_uri }`; 409 once an authenticator is confirmed |
| POST   | `/api/me/2fa/totp/confirm`  | `{ code }` → `{ ok, expires_at }` — confirming is itself a step-up |
| POST   | `/api/auth/stepup/email/start` | sends a single-use code (dev: server log; prod: `OST_STEPUP_WEBHOOK`) |
| POST   | `/api/auth/stepup/verify`   | `{ method: "totp"\|"email", code }` → `{ method, expires_at }`, rotates the session |
| POST   | `/api/auth/voucher`         | `{ voucher }` → session (device-voucher autologin); the session can read but never starts stepped up |

### Step-up 2FA

Reading is free; **every mutating `/api/*` request needs a live step-up grant**,
enforced by a layer (`server/src/stepup.rs`) rather than per-handler, so routes
added later are guarded automatically. Without a grant: **`428
step_up_required`** — the client's contract is to run a step-up flow and retry
the same request. Exempt (they are how a grant is obtained): the register/login
ceremonies, logout, `/api/auth/voucher`, the two `/api/me/2fa/totp/*` calls and
both `/api/auth/stepup/*` calls.

A grant lasts 5 minutes and is bound to the session row. Verifying rotates the
session token, keeping the old one valid for 2 minutes so in-flight requests and
second tabs survive. TOTP codes are single-use (a spent counter is dead even
inside its window); five wrong factors start a doubling lockout, capped at 15
minutes, counted in the database so a restart does not clear it.

### Agent

| POST   | `/agent/voucher`            | mint a one-time (2 min) voucher for a local surface on that machine to exchange at `/api/auth/voucher` |
| GET    | `/api/me/passkeys`          | → `{ passkeys: [{ id, nickname, created_at, last_used_at }] }` |
| DELETE | `/api/me/passkeys/:id`      | → `{ ok: true }`; 409 if it's the last credential and OIDC is disabled |

Sessions are DB-backed (`admin_sessions`, sha256-hashed token, 30-day TTL) and carried in the
`ost_session` cookie: `HttpOnly`, `SameSite=Lax`, `Secure` unless
`OST_INSECURE_COOKIES=1`. WebAuthn *challenge* state is held server-side in a short-TTL
in-memory store keyed by a temporary cookie.

### OIDC SSO (e.g. Authentik)

Enabled when `OST_OIDC_ISSUER`, `OST_OIDC_CLIENT_ID` and `OST_OIDC_CLIENT_SECRET`
are all set (`OST_OIDC_NAME` optionally labels the login button, default "SSO"). Endpoints
are discovered at startup from `<issuer>/.well-known/openid-configuration`; authorization-code
flow with scopes `openid email profile`; redirect URI is
`<OST_PUBLIC_URL>/api/auth/oidc/callback` (`OST_PUBLIC_URL` falls back to `RP_ORIGIN`).
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
| POST   | `/api/devices`                | `{ name }` → creates `pending` device + 24 h TTL enroll token → `{ device, enroll_token }` |
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
Body: { status, public_ip?, usage: [{ os_username, used_minutes_today }], os_users: [...] }
→ 200 { commands: [Command...], policy_version: string }
```
`usage` is upserted into `screen_time_ledger.used_seconds` for today's row per device user.
Agent acks commands via `POST /agent/commands/:id/ack { status, result }`.

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
memory (`client/src/runner.rs` `flush_events`, capped) and retried on the next tick rather than
dropped. The WS `event` frame (see below) is still accepted by the server for compatibility but is
not how the current agent sends events.

### WebSocket bus (preferred transport)
```
GET /agent/ws   (Upgrade)
```
Bidirectional JSON frames, tagged with `"type"`:

- server → agent: `command { command }`,
  `ping` (reserved — accepted by the agent, not currently sent by the server)
- agent → server: `event { event }` (accepted for compatibility; the agent now sends events over
  HTTP, see below), `ack { ack }`, `pong`

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
  }
}
```

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
  left_minutes|null, locked, devices:[{id,name,status,locked}], blocks,
  blocked_apps:[app id], bracket, theme, can_ask, pending_request, bedtime,
  windows, display_name }`.
- `POST /api/me/ask {minutes, reason?}` → `{ request }` (an `earn_request`
  with `task_id: "ask"`, one open per day; not step-up guarded).
- `GET /api/catalog` → `{ categories:[{id,name,blurb,app_ids}],
  apps:[{id,name,category,has_native_client}] }`.
- **Member sessions** may reach only `/api/me`, `/api/me/today`, `/api/me/ask`,
  `/api/catalog`, `/api/me/2fa*`, `/api/auth/*`. Anything else under `/api/` →
  `403 forbidden_for_member` (a layer; fails closed for new routes).

**Parent code (per-device TOTP).**
- `POST /api/devices {name, account_id?}` → `{ device, enroll_token,
  parent_code: {secret, otpauth_uri} }`. `account_id` = "this is <person>'s
  computer": OS logins without a name match link to that person on enroll.
- `GET /api/devices/{id}/parent-code` (sensitive read → 428 without a step-up
  grant) → `{ parent_code }`; `POST /api/devices/{id}/parent-code/rotate`
  → `{ parent_code }` and queues `apply_policy`.
- Agent pull `GET /agent/policy` adds top-level `parent_code: { totp_secret }`.
  `parent_pin_hash` in each user policy is still served as the **backup code**.

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
