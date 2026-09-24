# Data model

Postgres. The schema is whatever `server/migrations/` produces; this doc is
the map. Timestamps are `timestamptz`, ids `uuid` unless noted. Every
household-owned row carries `tenant_id` (a household is a tenant), and every
query filters on it — scoping lives in the application, not in row-level
security.

## Tables

### `tenants`
A household. `id`, `name`, `created_at`.

### `admins` — accounts
Everyone has one: parents and the people they look after. No password column
anywhere.

| column | type | notes |
|---|---|---|
| id, tenant_id | uuid | |
| display_name | text | |
| username | text | sign-in name; unique (case-insensitive) where set |
| email | text, nullable, unique | legacy; SSO still matches on it |
| role | text | `owner` \| `parent` \| `member` |
| age_bracket | text | `little` \| `kid` \| `younger_teen` \| `older_teen` \| `adult` |
| birthdate | date, nullable | picks the bracket when given |
| theme | text, nullable | `playful` \| `calm` \| `plain`; null = by bracket |
| self_managed | bool | keeps their own time (adults) |
| profile_id | uuid → profiles | their rules |
| avatar | text, nullable | a parent-picked emoji; null = monogram |
| goal_minutes | int, nullable | their own daily goal (API only now) |
| blocked_at | timestamptz, nullable | account suspended |
| created_at | timestamptz | |

### `webauthn_credentials`
One row per passkey: `id`, `admin_id`, `credential_id` (bytea, indexed),
`passkey` (jsonb, `webauthn_rs` `Passkey`), `nickname`, `created_at`,
`last_used_at`.

### `admin_sessions`
Console sessions (cookie `ost_session`): `id`, `token_hash` (sha256 hex,
unique), `admin_id`, `tenant_id`, `created_at`, `expires_at` (30 days, 7 for a
voucher; not sliding), `stepup_until` (the "confirm it's you" window),
`last_seen_at`, `prev_token_hash` + `prev_valid_until` (a rotated token stays
valid 2 minutes), `via_voucher`.

### `login_codes`
Door one and "confirm with a code": `id`, `tenant_id`, `account_id` (both null
for a decoy), `purpose` (`login` \| `confirm`), `code_challenge` (PKCE, login),
`session_id` (confirm), `code_hash`, `attempts`, `used_at`, `created_at`,
`expires_at`.

### `signin_links`
One-time recovery links from `openscreentime-server recover`: `id`,
`tenant_id`, `account_id`, `token_hash` (unique), `created_at`, `expires_at`,
`consumed_at`.

### `device_vouchers`
One-time `ost login` vouchers: `id`, `device_id`, `tenant_id`, `account_id`,
`voucher_hash` (unique), `consumed_at`, `expires_at`, `created_at`.

### `profiles` — rules
| column | type | notes |
|---|---|---|
| id, tenant_id | uuid | |
| name | text | |
| kind | text | `little` \| `kid` \| `younger_teen` \| `older_teen` \| `adult` \| `custom`, plus legacy `kids` \| `teen` \| `default` |
| is_preset | bool | the five bracket presets per household |
| policy | jsonb | the `Policy` document (docs/PROFILES.md) |
| created_at, updated_at | timestamptz | `updated_at` bumps make agents re-pull |

A person's rules are a non-preset copy with `kind` = their bracket.

### `devices` — computers
| column | type | notes |
|---|---|---|
| id, tenant_id | uuid | |
| name, hostname, os, agent_version | text | |
| status | text | presence only: `pending` \| `online` \| `offline` |
| locked | bool | paused, as the agent last reported |
| last_state | jsonb | the agent's last `state` frame |
| tamper_level | int | 1 or 3 |
| device_token | text | sha256 of the agent's bearer token |
| enroll_token, enroll_token_expires_at | text, timestamptz | one-time, 24 h |
| enroll_token_used, enrolled_at | text, timestamptz | a retried enroll within 15 min gets the same answer |
| parent_totp_secret | text | the secret behind the unlock code |
| owner_account_id | uuid → admins | whose computer it is |
| owner_os_username | text | the owner's own login; a parent's codes and vouchers go there only |
| agent_features | text[] | what the agent said it understands (`login_code`); null = older agent |
| offline_allowed_until | timestamptz | "Allow offline…" |
| utc_offset_secs | int | its UTC offset as last reported (which date is "today" there) |
| vpn_updated_at | timestamptz | |
| recovery_pin_hash, recovery_pin_set_at | text, timestamptz | from 0011; nothing reads them |
| public_ip | inet | |
| last_seen, created_at | timestamptz | |

### `device_users` — logins
One row per OS login on a computer: `id`, `device_id`, `os_username`,
`display_name`, `profile_id` (always the linked person's rules), `account_id`
(→ admins, the person), `unsorted` (became a person of its own because nobody
could say whose it is; cleared when a parent sorts it), `created_at`.
`UNIQUE(device_id, os_username)`.

### `commands`
Server → agent queue.

| column | type | notes |
|---|---|---|
| id, device_id | uuid | |
| type | text | `lock` \| `unlock` \| `apply_policy` \| `set_tamper_level` \| `credit_time` \| `deny_earn` \| `ping` \| `login_code` |
| payload, result | jsonb | |
| status | text | `queued` \| `sent` \| `acked` \| `failed` \| `cancelled` |
| created_at, sent_at, acked_at | timestamptz | |

A partial unique index allows one pending (`queued`/`sent`) `lock`, `unlock`,
`apply_policy` or `set_tamper_level` per computer.
A `login_code` row's code is emptied once it's delivered, acked, used or
expired, and never listed in the console.

### `events`
The agent's reports and the server's own audit trail.

| column | type | notes |
|---|---|---|
| id, tenant_id | uuid | |
| device_id, device_user_id | uuid, nullable | |
| client_id | uuid, nullable | the agent's event id; unique per device, so a redelivery is stored once |
| type | text | `heartbeat`, `tamper`, `lock`, `unlock`, `policy_applied`, `screen_time_exceeded`, `screen_time_earned`, `enrolled`, `ssh` (historical), `earn_request`, `evasion`, `enforcement_degraded`, `vpn_profile`, `parent_code_ok`, `parent_code_failed`, `parent_code_backup_used`, `app_blocked`, `member`, `account_login`, `login_approval`, `other` |
| severity | text | `info` \| `warn` \| `critical` |
| payload | jsonb | |
| created_at | timestamptz | pruned after 90 days |

### `earn_requests` — requests for more time
`id`, `tenant_id`, `device_id`, `device_user_id`, `task_id` (`ask` from the
web, else an earn task id), `task_label`, `minutes` (1–240), `status`
(`pending` \| `approved` \| `denied`), `created_at`, `decided_at`. One pending
request per login, task and day.

### `screen_time_ledger`
Per-login daily use and grants. A person's day is the sum of their logins'
rows for that day — one budget per person (docs/TRACKING.md).

| column | type | notes |
|---|---|---|
| id, device_user_id | uuid | `UNIQUE(device_user_id, day)` |
| day | date | the **device-local** day the agent enforces |
| used_seconds | int | real use, never decreases within a day (`GREATEST`) |
| earned_seconds | int | time given (a grant or an approved request) |
| streak_days | int | never written by anything |

Not pruned.

### `usage_slices` — where the time went
`device_id`, `tenant_id`, `os_username` (`''` for a site), `hour`, `kind`
(`app`: seconds open; `site`: lookups — no CHECK), `key`, `amount`. Primary
key `(device_id, os_username, hour, kind, key)`. Pruned after 21 days.

### `device_recovery_codes`
`id`, `device_id`, `idx`, `mac` (HMAC of the 8 digits, keyed by the unlock
secret; the code itself isn't stored), `created_at`, `used_at`.

### `device_vpn_profiles`
`id`, `device_id`, `name` (unique per computer), `kind` (`wireguard` \|
`openvpn`), `config`, `status` (`untested` \| `testing` \| `active` \|
`failed`), `last_error`, `last_tested_at`, `is_active` (at most one per
computer), `created_at`, `updated_at`.

### `parent_access_tokens`
Paired-companion tokens: `id`, `tenant_id`, `token_hash` (unique), `label`,
`created_by`, `created_at`, `last_used_at`, `revoked_at`.

### `telegram_chats`, `telegram_pair_codes`
A paired chat: `chat_id` (bigint pk), `admin_id`, `tenant_id`, `username`,
`created_at`. A pairing code: `id`, `admin_id`, `tenant_id`, `code_hash`,
`expires_at`, `consumed_at`.

### `ops_log`, `ops_incidents`
The appliance's own record. `ops_log`: `id` (bigserial), `kind` (`install` \|
`backup` \| `update`), `ok`, `detail`, `created_at` (pruned after 90 days).
`ops_incidents`: one open incident per `key` (`tenant_id`, `message`,
`opened_at`), so an operator alert fires once per incident.

## Migrations

`server/migrations/NNNN_description.sql`, run by the server on start
(`db::migrate`). Numbers go 0001–0027, then 0030: **0028 and 0029 don't
exist** — they were left unused when parallel branches merged, and sqlx
accepts the gap. A new migration is 0031 or later (a lower number would run
after 0030 on databases that already have it).

| # | What it did |
|---|---|
| 0001 | Initial schema (tenants, admins, passkeys, profiles, devices, device users, commands, events, ledger, ssh sessions). Its header still claims to mirror this doc. |
| 0002 | `admin_sessions`, `earn_requests`; `credit_time`, `ssh`, `earn_request` types. |
| 0003 | `deny_earn` command. |
| 0004 | 24 h expiry on enroll tokens. |
| 0005 | `evasion` events. |
| 0006 | `parent_access_tokens`. |
| 0007 | A VPN profile per device (columns dropped by 0010). |
| 0008 | The remote shell is gone: drops `ssh_sessions` and its commands; `ssh` events stay readable. |
| 0009 | A real command queue: `sent_at`, `cancelled`, dedupe. |
| 0010 | Named VPN profiles (`device_vpn_profiles`). |
| 0011 | A recovery PIN per device (no longer used). |
| 0012 | Step-up 2FA (its admin columns and email codes dropped by 0030); `device_vouchers`; session rotation columns. |
| 0013 | Drops LAN discovery and streak nudges. |
| 0014 | `offline_allowed_until`; ledger and pending-command indexes. |
| 0015 | Accounts: roles, brackets, `self_managed`, `profile_id`; per-device unlock-code secret; `locked` as its own column; five bracket presets; new event types. |
| 0016 | `device_recovery_codes`; change mode (dropped by 0030). |
| 0017 | Trust decided at login (its column dropped by 0030). |
| 0018 | Telegram pairing (its verification table dropped by 0030). |
| 0019 | Client-first login requests (dropped by 0030). |
| 0020 | `avatar`. |
| 0021 | `usage_slices`. |
| 0022 | A match code for login requests (dropped with them). |
| 0023 | `goal_minutes`. |
| 0024 | `username`. |
| 0025 | `blocked_at`. |
| 0026 | The appliance: idempotent events (`client_id`), retry-safe enroll, `ops_log`, `ops_incidents`, `login_approval` / `other` events. |
| 0027 | The ledger's day is the device-local day; `utc_offset_secs`. |
| 0030 | Two doors: `login_codes`, `signin_links`, `owner_os_username`, `agent_features`, `unsorted`; drops TOTP 2FA, email codes, change mode, trusted sessions, login requests and Telegram verifications; unlinks logins it can't attribute on a parent's computer (docs/AUTH.md). |

The five bracket presets are seeded in application code
(`server/src/presets.rs`), for every household at startup and on creation.
