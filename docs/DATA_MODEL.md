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
| type        | text        | `lock` \| `unlock` \| `apply_policy` \| `ssh_open` \| `ssh_close` \| `discover` \| `set_tamper_level` |
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
| type        | text        | `heartbeat` \| `tamper` \| `lock` \| `unlock` \| `policy_applied` \| `screen_time_exceeded` \| `screen_time_earned` \| `streak` \| `enrolled` \| `discovery_result` |
| severity    | text        | `info` \| `warn` \| `critical`                                 |
| payload     | jsonb       |                                                                |
| created_at  | timestamptz |                                                                |

### `ssh_sessions`
Reverse-tunnel bookkeeping for remote shell.
| column       | type        | notes                                          |
|--------------|-------------|------------------------------------------------|
| id           | uuid pk     |                                                |
| device_id    | uuid fk     |                                                |
| admin_id     | uuid fk     |                                                |
| broker_port  | int         | port on server the tunnel is bound to          |
| status       | text        | `opening` \| `open` \| `closed` \| `failed`    |
| created_at   | timestamptz |                                                |
| closed_at    | timestamptz | nullable                                        |

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

Live in `server/migrations/` as SQLx migrations (`NNNN_description.sql`). The first migration
creates all tables above. Seeding the three preset profiles happens in application code when a
tenant is created (see `PROFILES.md`).
