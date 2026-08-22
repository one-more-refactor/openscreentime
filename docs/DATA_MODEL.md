# Data Model

Postgres. All timestamps are `timestamptz`. All ids are `uuid` (v4) unless noted. Every
tenant-owned row carries `tenant_id` for isolation. Row-level tenant scoping is enforced in the
application layer (every query filters by `tenant_id` from the authenticated session).

## Tables

### `tenants`
| column      | type        | notes                          |
|-------------|-------------|--------------------------------|
| id          | uuid pk     |                                |
| name        | text        |                                |
| created_at  | timestamptz | default now()                  |

### `admins`
Parents/operators. Passkey-only — **no password column**.
| column       | type        | notes                         |
|--------------|-------------|-------------------------------|
| id           | uuid pk     |                               |
| tenant_id    | uuid fk     | → tenants.id                  |
| email        | text unique |                               |
| display_name | text        |                               |
| created_at   | timestamptz | default now()                 |

### `webauthn_credentials`
One row per registered passkey.
| column         | type        | notes                                   |
|----------------|-------------|-----------------------------------------|
| id             | uuid pk     |                                         |
| admin_id       | uuid fk     | → admins.id                             |
| credential_id  | bytea       | raw credential id                       |
| passkey        | jsonb       | serialized `webauthn_rs::prelude::Passkey` |
| nickname       | text        | e.g. "Pixel 8 fingerprint"              |
| created_at     | timestamptz | default now()                           |
| last_used_at   | timestamptz | nullable                                |

### `profiles`
Policy presets. Ships with three `is_preset=true` rows per tenant on creation: kids, teen, default.
| column      | type        | notes                                        |
|-------------|-------------|----------------------------------------------|
| id          | uuid pk     |                                              |
| tenant_id   | uuid fk     |                                              |
| name        | text        | "Kids", "Teen", "Default", or custom         |
| kind        | text        | enum: `kids` \| `teen` \| `default` \| `custom` |
| is_preset   | bool        | true for the three shipped presets           |
| policy      | jsonb       | the Policy document (see API.md → Policy)     |
| created_at  | timestamptz |                                              |
| updated_at  | timestamptz |                                              |

### `devices`
| column          | type        | notes                                                   |
|-----------------|-------------|---------------------------------------------------------|
| id              | uuid pk     |                                                         |
| tenant_id       | uuid fk     |                                                         |
| name            | text        | friendly name                                           |
| hostname        | text        | reported by agent                                       |
| os              | text        | e.g. "linux"                                            |
| agent_version   | text        |                                                         |
| status          | text        | enum: `pending` \| `online` \| `offline` \| `locked`    |
| tamper_level    | int         | 1 (default) or 3                                        |
| device_token    | text        | bearer token the agent uses (hashed at rest)            |
| enroll_token    | text        | one-time enrollment token, null after enrollment        |
| enroll_token_expires_at | timestamptz | 24 h after issue; NULL for rows predating this column (no expiry) |
| public_ip       | inet        | nullable                                                |
| last_seen       | timestamptz | nullable                                                |
| created_at      | timestamptz |                                                         |

### `device_users`
One row per OS user account on a device (zero-trust: policy is per person).
| column          | type        | notes                                       |
|-----------------|-------------|---------------------------------------------|
| id              | uuid pk     |                                             |
| device_id       | uuid fk     |                                             |
| os_username     | text        | Linux username                              |
| display_name    | text        | nullable                                    |
| profile_id      | uuid fk     | → profiles.id (which policy applies)        |
| created_at      | timestamptz |                                             |
| UNIQUE(device_id, os_username)                                                            |

### `commands`
Server → agent command queue. Agent pulls on heartbeat / WS.
| column      | type        | notes                                                      |
|-------------|-------------|------------------------------------------------------------|
| id          | uuid pk     |                                                            |
| device_id   | uuid fk     |                                                            |
| type        | text        | `lock` \| `unlock` \| `apply_policy` \| `set_tamper_level` \| `credit_time` \| `deny_earn` |
| payload     | jsonb       | command-specific args                                      |
| status      | text        | `queued` \| `sent` \| `acked` \| `failed`                  |
| result      | jsonb       | nullable, agent's response                                 |
| created_at  | timestamptz |                                                            |
| acked_at    | timestamptz | nullable                                                   |

### `events`
Agent → server telemetry & audit log.
| column      | type        | notes                                                          |
|-------------|-------------|----------------------------------------------------------------|
| id          | uuid pk     |                                                                |
| tenant_id   | uuid fk     |                                                                |
| device_id   | uuid fk     | nullable                                                       |
| device_user_id | uuid fk  | nullable                                                       |
| type        | text        | `heartbeat` \| `tamper` \| `lock` \| `unlock` \| `policy_applied` \| `screen_time_exceeded` \| `screen_time_earned` \| `enrolled` \| `ssh` (historical only — see below) \| `earn_request` |
| severity    | text        | `info` \| `warn` \| `critical`                                 |
| payload     | jsonb       |                                                                |
| created_at  | timestamptz |                                                                |

The `ssh` event type is **historical only**: the remote-shell feature (and its
`ssh_sessions` table) was removed in v0.4, but existing `ssh` event rows stay readable —
the record that past sessions happened survives; only the capability is gone. No new
`ssh` events are written.

### `admin_sessions`
DB-backed admin login sessions (cookie `ost_session`). Expired rows are deleted lazily.
| column      | type        | notes                                        |
|-------------|-------------|----------------------------------------------|
| id          | uuid pk     |                                              |
| token_hash  | text unique | sha256 hex of the cookie value               |
| admin_id    | uuid fk     | → admins.id (cascade)                        |
| tenant_id   | uuid fk     | → tenants.id (cascade)                       |
| created_at  | timestamptz | default now()                                |
| expires_at  | timestamptz | 30 days after creation (not sliding)         |

### `earn_requests`
Earn-time approval flow: agent files a request, a parent approves/denies it.
| column          | type        | notes                                        |
|-----------------|-------------|----------------------------------------------|
| id              | uuid pk     |                                              |
| tenant_id       | uuid fk     |                                              |
| device_id       | uuid fk     |                                              |
| device_user_id  | uuid fk     |                                              |
| task_id         | text        | earn task id from the policy                 |
| task_label      | text        |                                              |
| minutes         | int         | 1..240                                       |
| status          | text        | `pending` \| `approved` \| `denied`          |
| created_at      | timestamptz |                                              |
| decided_at      | timestamptz | nullable                                     |

One *pending* request per (device_user, task, day) — the server dedupes by returning the
existing pending row. Approval upserts `screen_time_ledger.earned_seconds` and enqueues a
`credit_time` command.

### `screen_time_ledger`
Per-user daily balance for the "earn time" mechanic.
| column          | type        | notes                                     |
|-----------------|-------------|-------------------------------------------|
| id              | uuid pk     |                                           |
| device_user_id  | uuid fk     |                                           |
| day             | date        |                                           |
| earned_seconds  | int         | credits earned via tasks                  |
| used_seconds    | int         | consumed                                  |
| streak_days     | int         | current streak                            |
| UNIQUE(device_user_id, day)                                                |

## Migrations

Live in `server/migrations/` as SQLx migrations (`NNNN_description.sql`). `0001_init.sql`
creates the original tables; `0002_prod.sql` adds `admin_sessions`, `earn_requests` and extends
the `commands.type` (`credit_time`) and `events.type` (`ssh`, `earn_request`) CHECK constraints.
`0003_deny_earn.sql` extends the `commands.type` CHECK constraint to add `deny_earn`, the mirror
of `credit_time` for the denial path (lets the agent clear its once-per-day earn-request dedupe
and surface an honest denial instead of a stale "WAITING FOR APPROVAL"). `0004_enroll_token_ttl.sql`
adds `devices.enroll_token_expires_at` (24 h TTL on enrollment tokens; NULL/no-expiry for rows
predating the migration, additive with no backfill). `0008_remove_ssh.sql` drops the
`ssh_sessions` table and the `ssh_open`/`ssh_close` command types (the remote shell is gone);
`events.type = 'ssh'` stays in the CHECK constraint so historical rows remain readable.
Seeding the three preset profiles happens in
application code when a tenant is created (see `PROFILES.md`).


## 0.4 (migration 0015)

- `admins` += `role`, `age_bracket`, `birthdate`, `theme`, `self_managed`,
  `profile_id` (→ profiles); `email` is nullable (members usually have none).
  The admins table *is* the account table; "member" = a managed person or a
  self-tracking adult, no passkey.
- `device_users` += `account_id` (→ admins, ON DELETE SET NULL). Every OS login
  is linked on enroll/heartbeat/startup (`members::link_os_user`).
- `devices` += `parent_totp_secret` (base32 — the per-device parent code),
  `owner_account_id`, `locked` (bool), `last_state` (jsonb). `status` CHECK is
  now `pending|online|offline`; old `'locked'` rows became `offline`+`locked`.
- `device_vouchers` += `account_id`.
- `profiles.kind` CHECK accepts the five bracket ids (+ the legacy three and
  `custom`). Five bracket presets per tenant; a member's rules are a non-preset
  copy with `kind = <bracket>`.
- `events.type` CHECK += `parent_code_ok`, `parent_code_failed`,
  `parent_code_backup_used`, `app_blocked`, `member`, and restores
  `enforcement_degraded` + `vpn_profile` (dropped by 0013 by mistake).
