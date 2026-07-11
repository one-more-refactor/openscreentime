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

## Auth (passkey / WebAuthn)

Uses `webauthn-rs`. Registration is invite/first-run only in the skeleton (any email can
register the first admin of a new tenant; hardened later).

| Method | Path                        | Body / Notes                                            |
|--------|-----------------------------|---------------------------------------------------------|
| POST   | `/api/auth/register/start`  | `{ email, display_name }` → `CreationChallengeResponse` |
| POST   | `/api/auth/register/finish` | `{ email, credential }` → sets session, `{ admin }`     |
| POST   | `/api/auth/login/start`     | `{ email }` → `RequestChallengeResponse`                |
| POST   | `/api/auth/login/finish`    | `{ credential }` → sets session cookie                  |
| POST   | `/api/auth/logout`          | clears session                                          |
| GET    | `/api/me`                   | → `{ admin, tenant }`                                   |
| GET    | `/api/me/passkeys`          | → `{ passkeys: [{ id, nickname, created_at, last_used_at }] }` |

Challenge state is held server-side in a short-TTL store keyed by a temporary cookie.

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
| POST   | `/api/devices/:id/ssh`        | open reverse-tunnel session → `{ ssh_session, connect_cmd }`|
| DELETE | `/api/devices/:id`            | de-enroll                                                    |

## Device users & profile assignment

| Method | Path                                         | Notes                              |
|--------|----------------------------------------------|------------------------------------|
| GET    | `/api/devices/:id/users`                     | list OS users on device            |
| POST   | `/api/device-users/:id/assign-profile`       | `{ profile_id }`                   |

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
Body: { status, public_ip?, metrics?, os_users: [...] }
→ 200 { commands: [Command...], policy_version: string }
```
Agent acks commands via `POST /agent/commands/:id/ack { status, result }`.

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
Bidirectional. Server pushes `Command` messages; agent pushes `Event` + `CommandAck`. Also
carries reverse-SSH tunnel data frames (see TAMPER.md / SSH section). Falls back to heartbeat
polling if WS unavailable.

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
  "app_limits": [
    { "match": "steam", "daily_limit_minutes": 60 }
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
