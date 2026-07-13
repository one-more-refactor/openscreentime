# API Contract

Two surfaces:

- **Admin API** (`/api/*`) — used by the web control center. Authenticated with a session
  cookie issued after a passkey login.
- **Agent API** (`/agent/*`) — used by the Linux agent. Authenticated with a bearer
  `device_token` (except enrollment, which uses a one-time `enroll_token`).

All request/response bodies are JSON. Errors use `{ "error": { "code": string, "message": string } }`
with an appropriate HTTP status.

Base URL in dev: `http://localhost:8080`.

---

## Auth (passkey / WebAuthn + optional OIDC SSO)

Uses `webauthn-rs`. Registration is invite/first-run only in the skeleton (any email can
register the first admin of a new tenant; hardened later).

| Method | Path                        | Body / Notes                                            |
|--------|-----------------------------|---------------------------------------------------------|
| GET    | `/api/auth/config`          | public → `{ auth: { oidc: bool, oidc_name } }`          |
| POST   | `/api/auth/register/start`  | `{ email, display_name }` → `CreationChallengeResponse` |
| POST   | `/api/auth/register/finish` | `{ email, credential }` → sets session, `{ admin }`     |
| POST   | `/api/auth/login/start`     | `{ email }` → `RequestChallengeResponse`                |
| POST   | `/api/auth/login/finish`    | `{ credential }` → sets session cookie                  |
| GET    | `/api/auth/oidc/start`      | 302 to the provider's authorize URL                     |
| GET    | `/api/auth/oidc/callback`   | `?code&state` → session + redirect `/` (see below)      |
| POST   | `/api/auth/logout`          | clears session (deletes the DB row)                     |
| GET    | `/api/me`                   | → `{ admin, tenant }`                                   |
| GET    | `/api/me/passkeys`          | → `{ passkeys: [{ id, nickname, created_at, last_used_at }] }` |
| DELETE | `/api/me/passkeys/:id`      | → `{ ok: true }`; 409 if it's the last credential and OIDC is disabled |

Sessions are DB-backed (`admin_sessions`, sha256-hashed token, 30-day TTL) and carried in the
`sentinel_session` cookie: `HttpOnly`, `SameSite=Lax`, `Secure` unless
`SENTINEL_INSECURE_COOKIES=1`. WebAuthn *challenge* state is held server-side in a short-TTL
in-memory store keyed by a temporary cookie.

### OIDC SSO (e.g. Authentik)

Enabled when `SENTINEL_OIDC_ISSUER`, `SENTINEL_OIDC_CLIENT_ID` and `SENTINEL_OIDC_CLIENT_SECRET`
are all set (`SENTINEL_OIDC_NAME` optionally labels the login button, default "SSO"). Endpoints
are discovered at startup from `<issuer>/.well-known/openid-configuration`; authorization-code
flow with scopes `openid email profile`; redirect URI is
`<SENTINEL_PUBLIC_URL>/api/auth/oidc/callback` (`SENTINEL_PUBLIC_URL` falls back to `RP_ORIGIN`).
The callback matches the verified userinfo email against existing admins (any tenant). Fresh
installs (no admins at all) bootstrap a tenant + admin; an unknown email on a non-empty install
redirects to `/login?error=sso_unknown_account` (no auto-provisioning); other failures redirect
to `/login?error=sso_failed`.

### Rate limiting

Fixed-window, in-memory, per client IP (first `X-Forwarded-For` value when
`SENTINEL_TRUST_PROXY=1`, else the peer address). Over-limit requests get a 429 error envelope.

- auth attempt endpoints (register/login/OIDC start + finish): 10 req / 60 s / IP
- `/agent/enroll`: 5 req / 60 s / IP

---

## Devices

| Method | Path                          | Notes                                                        |
|--------|-------------------------------|-------------------------------------------------------------|
| GET    | `/api/devices`                | list devices for tenant (+status, last_seen, users)         |
| GET    | `/api/devices/:id`            | detail incl. device_users, recent events                    |
| POST   | `/api/devices`                | `{ name }` → creates `pending` device + `enroll_token`      |
| PATCH  | `/api/devices/:id`            | rename, set `tamper_level`                                   |
| POST   | `/api/devices/:id/lock`       | enqueue `lock` command                                      |
| POST   | `/api/devices/:id/unlock`     | enqueue `unlock` command                                    |
| POST   | `/api/devices/:id/ssh`        | open remote shell session → `{ session: { id, device_id, broker_port, status:"opening", created_at } }` |
| DELETE | `/api/devices/:id`            | de-enroll                                                    |

## Remote SSH (browser terminal)

A session stays `opening` until the agent's first `ssh_data` frame confirms the shell, then
becomes `open`. All SSH activity is audited with events of `type = 'ssh'`.

| Method | Path                        | Notes                                                    |
|--------|-----------------------------|----------------------------------------------------------|
| GET    | `/api/ssh/:session_id/ws`   | cookie-authenticated WebSocket upgrade for the terminal  |
| POST   | `/api/ssh/:session_id/close`| → `{ ok: true, session_id }`; sends `ssh_close` to agent |

Browser WS protocol: browser → server **binary frames** are raw keystroke bytes, **text
frames** are JSON `{"type":"resize","cols":N,"rows":N}`. Server → browser binary frames are
raw terminal output; a final text frame `{"type":"closed","exit_code":N|null}` precedes the
close. Closing the browser WS also closes the session.

## Device users & profile assignment

| Method | Path                                         | Notes                              |
|--------|----------------------------------------------|------------------------------------|
| GET    | `/api/devices/:id/users`                     | → `{ users: [{ id, device_id, os_username, display_name, profile_id, profile_name, profile_kind, used_minutes_today, earned_minutes_today }] }` (today's minutes joined from `screen_time_ledger`) |
| POST   | `/api/device-users/:id/assign-profile`       | `{ profile_id }`                   |

## Earn-time requests

Filed by the agent when a user picks an earn offer on the lockout screen; decided by a parent
in the web UI. One open request per (user, task) per day (agent-side duplicates return the
existing pending row). Requests and decisions are audited with `earn_request` events.

| Method | Path                              | Notes                                             |
|--------|-----------------------------------|---------------------------------------------------|
| GET    | `/api/earn-requests`              | `?status=pending` → `{ requests: [...] }` (joined with device name + user display name) |
| POST   | `/api/earn-requests/:id/approve`  | → `{ request }`; credits `screen_time_ledger.earned_seconds` and enqueues a `credit_time` command `{ os_username, minutes, request_id }` |
| POST   | `/api/earn-requests/:id/deny`     | → `{ request }`                                   |

A request: `{ id, device_id, device_name, device_user_id, os_username, display_name, task_id,
task_label, minutes, status, created_at, decided_at }` with `status` one of
`pending | approved | denied` (409 when deciding an already-decided request).

## Profiles

| Method | Path                    | Notes                                         |
|--------|-------------------------|-----------------------------------------------|
| GET    | `/api/profiles`         | list (3 presets + custom)                     |
| POST   | `/api/profiles`         | `{ name, kind:"custom", policy }`             |
| GET    | `/api/profiles/:id`     |                                               |
| PUT    | `/api/profiles/:id`     | update policy (presets are cloneable, editable)|
| DELETE | `/api/profiles/:id`     | custom only                                   |

## Discovery

| Method | Path                    | Notes                                                       |
|--------|-------------------------|-------------------------------------------------------------|
| POST   | `/api/discovery/scan`   | `{ device_id }` — ask an enrolled agent to scan its LAN     |
| GET    | `/api/discovery/results`| recent `discovery_result` events (found hosts, open ports)  |

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
The `enroll_token` is consumed (single use). Server creates `device_users` rows for reported
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

### WebSocket bus (preferred transport)
```
GET /agent/ws   (Upgrade)
```
Bidirectional JSON frames, tagged with `"type"`:

- server → agent: `command { command }`, `ssh_data { session_id, data_b64 }`,
  `ssh_resize { session_id, cols, rows }`, `ssh_close { session_id }`, `ping`
- agent → server: `event { event }`, `ack { ack }`,
  `ssh_data { session_id, data_b64 }`, `ssh_closed { session_id, exit_code? }`, `pong`

`data_b64` carries base64-encoded raw terminal bytes in both directions. The agent's first
frame for a session flips it `opening` → `open`. Falls back to heartbeat polling if WS is
unavailable.

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
  "app_limits": [                    // DEPRECATED: not enforced by the agent; the field
    { "match": "steam", "daily_limit_minutes": 60 }   // stays in the policy crate for forward compat
  ],
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
    },
    "streaks": { "enabled": true, "nudges": ["bedtime", "breaks"] }
  }
}
```

Every component MUST treat unknown fields leniently (forward-compat). The Rust side models this
with `#[serde(default)]` on optional sub-objects.
