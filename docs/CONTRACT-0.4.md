# 0.4.0 build contract — "it finally works"

The shared contract for the 0.4.0 push. Three workstreams (server, client,
web) build against this in parallel; when they disagree with older docs, this
wins. `docs/OPENSCREENTIME.md` is still the product north star.

Scope, in one breath: every person is an **account** with a **role** and an
**age bracket**; the parent code is a **per-device TOTP** (authenticator app,
verified offline) instead of a PIN; blocking is **apps & categories, one
click** from a built-in catalog; presence is **WebSocket-first** with an
honest, separate **locked** state; a child who opens the console on their own
device lands on **their own page**, themed by bracket; `sudo` on a managed
machine asks for the **parent's authenticator code** via PAM.

What is explicitly NOT in 0.4.0: a remote shell (stays removed — the console
owns root only through the agent's command channel), SMS, QR device pairing,
AdGuard/Pi-hole integration beyond "use it as the DNS upstream".

---

## 1. Shared policy crate (`policy/`) — DONE, build against it

```rust
pub struct Policy {
    // …existing fields unchanged…
    /// One-click app / category blocks. Expanded on the device by
    /// `openscreentime_policy::catalog::expand`.
    #[serde(default, skip_serializing_if = "AppBlocks::is_default")]
    pub blocks: AppBlocks,
}

pub struct AppBlocks {
    pub apps: Vec<String>,          // catalog app ids, e.g. "youtube"
    pub categories: Vec<String>,    // catalog category ids, e.g. "social"
    pub custom_domains: Vec<String>,// extra domains the parent typed
}

pub enum AgeBracket { Little, Kid, YoungerTeen, OlderTeen, Adult }
// serde: "little" | "kid" | "younger_teen" | "older_teen" | "adult"
impl AgeBracket { pub fn from_birthdate(ymd: NaiveDate, today: NaiveDate) -> Self;
                  pub fn default_theme(&self) -> Theme; pub fn label(&self) -> &str }

pub enum Theme { Playful, Calm, Plain }   // serde lowercase

pub mod catalog {
    pub struct AppDef { pub id: &'static str, pub name: &'static str,
        pub category: &'static str, pub domains: &'static [&'static str],
        pub processes: &'static [&'static str] }
    pub struct CategoryDef { pub id, pub name, pub blurb }
    pub fn apps() -> &'static [AppDef];
    pub fn categories() -> &'static [CategoryDef];
    /// Expand blocks → (domains to NXDOMAIN, process names to deny).
    pub fn expand(b: &AppBlocks) -> Expanded { domains: Vec<String>, processes: Vec<String> }
    /// JSON for `GET /api/catalog` (web never duplicates the list).
    pub fn as_json() -> serde_json::Value
}
```

`parent_pin_hash` stays in `Policy` (deprecated; it is now the **backup code**
— see §4). Presets move to five bracket presets (§3).

## 2. Database (`server/migrations/0015_accounts_and_presence.sql`)

```sql
-- accounts = the admins table, grown up
ALTER TABLE admins ALTER COLUMN email DROP NOT NULL;          -- kids have none
ALTER TABLE admins ADD COLUMN role text NOT NULL DEFAULT 'owner'
    CHECK (role IN ('owner','parent','member'));
ALTER TABLE admins ADD COLUMN age_bracket text NOT NULL DEFAULT 'adult'
    CHECK (age_bracket IN ('little','kid','younger_teen','older_teen','adult'));
ALTER TABLE admins ADD COLUMN birthdate date;
ALTER TABLE admins ADD COLUMN theme text            -- NULL = auto by bracket
    CHECK (theme IS NULL OR theme IN ('playful','calm','plain'));
ALTER TABLE admins ADD COLUMN self_managed bool NOT NULL DEFAULT false;
ALTER TABLE admins ADD COLUMN profile_id uuid REFERENCES profiles(id);  -- the person's rules
-- an OS login on a device belongs to a person
ALTER TABLE device_users ADD COLUMN account_id uuid REFERENCES admins(id) ON DELETE SET NULL;
-- per-device parent authenticator (base32) — the offline parent code
ALTER TABLE devices ADD COLUMN parent_totp_secret text;
-- honest presence
ALTER TABLE devices ADD COLUMN locked bool NOT NULL DEFAULT false;
ALTER TABLE devices ADD COLUMN last_state jsonb;      -- last agent State frame
-- bracket presets
ALTER TABLE profiles DROP CONSTRAINT profiles_kind_check;
ALTER TABLE profiles ADD CONSTRAINT profiles_kind_check CHECK (kind IN
  ('little','kid','younger_teen','older_teen','adult','custom','kids','teen','default'));
```

Rules:
- The first admin of a tenant is `owner`. Existing tenants: the existing
  admin(s) become `owner`, bracket `adult`.
- **Members** (children, and adults who only self-track) are rows in `admins`
  with `role='member'`, no passkey, usually no email. A member's rules are
  `admins.profile_id`; `device_users.profile_id` is kept in sync from it
  (agent pull is unchanged: it still reads `device_users.profile_id`).
- On enroll, every OS user is linked: if a member with `display_name` ==
  the OS user's display name / username exists in the tenant, link; otherwise
  **create a member** (bracket from the device's enroll intent, default `kid`)
  and link. Nothing stays unlinked.
- `devices.status` keeps `pending|online|offline` — **`locked` is its own
  column**, never a status value any more (the old `'locked'` status value is
  migrated to `offline`+`locked=true`).

## 3. Age brackets & presets

Five preset profiles per tenant (`is_preset`, kind = bracket id):

| kind | limit | windows | bedtime | blocks (catalog) | lockout |
|---|---|---|---|---|---|
| little | 45 min | 08–19 | 19:00–07:00 | categories: social, video_streaming, games, messaging, adult, gambling, dating, ai_chat, proxies + apps: youtube | hard stop, no request UI, no earn |
| kid | 60 min | 07–20 / 09–20 | 20:00–07:00 | social, adult, gambling, dating, proxies + apps: tiktok, snapchat, instagram, discord, twitch, omegle | hard stop, requests + earn on |
| younger_teen | 150 min | 07–21 / 09–22 | 22:00–06:30 | adult, gambling, dating, proxies + apps: tiktok | hard stop after 2-min wind-down |
| older_teen | 0 (=none) | none | none | adult, gambling, proxies | stop only if parent caps |
| adult | 0 | none | none | none | none (self-imposed) |

- `daily_limit_minutes = 0` with `enabled=false` means **no limit**. (Unchanged.)
- DNS for all five: `mode: allow_all`, `allowlist: ["*"]`, upstream `1.1.1.3`
  (little/kid/younger_teen) or `1.1.1.2` (older_teen/adult); `safe_search: true`
  for the first three. Operators with AdGuard/Pi-hole set `upstream` to it —
  that is the whole "integration".
- The old `kids/teen/default` presets are not re-seeded; existing rows stay valid.

## 4. Parent code = per-device TOTP (replaces the PIN)

- `devices.parent_totp_secret` (base32, 20 bytes) is minted when the device row
  is created (`POST /api/devices`) and **returned once** with the enroll token
  as `parent_code: { secret, otpauth_uri }` —
  `otpauth://totp/OpenScreenTime:<device name>?secret=…&issuer=OpenScreenTime&digits=6&period=30`.
  The web shows the QR (`qrcode` npm, render client-side) next to the install
  command: "scan this into your authenticator — it's the parent key for this
  computer".
- `GET /api/devices/{id}/parent-code` (step-up gated, sensitive read) returns
  it again. `POST /api/devices/{id}/parent-code/rotate` (step-up) re-mints.
- Agent pull `GET /agent/policy` adds top-level `parent_code: { totp_secret }`.
  The agent stores it root-only with the bundle cache.
- Agent verifies **offline**: RFC 6238, SHA1, 6 digits, 30 s, ±1 step, and a
  persisted last-accepted counter (`/var/lib/openscreentime/parent_code.json`)
  so a code is single-use. Five wrong codes → 60 s lockout (doubles, max 15 min),
  persisted.
- Everywhere the PIN was asked (lockout overlay "parent" field, `ost unlock`,
  tray unlock, `pam-auth`) now asks for "parent code (authenticator app)". The
  device recovery PIN survives **only as the backup code** (`parent_pin_hash`
  fallback) — accepted, logged as `parent_code_backup_used` (warn).
- Event: `parent_code_ok` / `parent_code_failed` (info/warn), payload `{ via:
  "overlay"|"unlock"|"tray"|"pam", user }`.

## 5. Presence, lock state, usage — WebSocket-first

Agent → server frames (`AgentFrame`, tagged `type`):
- `heartbeat` (existing) every **30 s** over WS, carrying `usage`.
- **new** `state` — sent on connect, whenever it changes, and at least every
  60 s: `{ locked: bool, frozen_users: [os_username], enforcing: bool,
  gaps: [string], agent_version, active_users: [os_username] }`.
- `event`, `ack`, `pong` unchanged.

Server:
- On WS open: `status='online'`, `last_seen=now()`. On any frame: `last_seen`.
  On WS close: `status='offline'` **immediately**. Sweep: `online` with
  `last_seen < now()-90s` → `offline` (WS-less agents on HTTP fallback poll
  every 30 s).
- `state` frame → `devices.locked`, `devices.last_state`, `agent_version`.
- `lock`/`unlock` commands are still enqueued; the device is **not** shown as
  locked until the agent's `state`/ack says so. API exposes
  `{ status, locked, lock_pending: bool, last_seen, last_state }`. "Pause
  everything" = lock on every device; `lock_pending` drives the sweep
  animation; `locked` is the truth.
- HTTP `/agent/heartbeat` remains for the poll fallback.

Agent:
- WS reconnect with jittered backoff 1 s → 60 s cap; HTTP poll fallback every
  30 s while WS is down. Usage is written to the local ledger every tick
  regardless and re-sent with the next heartbeat — nothing is lost offline.
- `locked` reported = **what the kernel says** (freeze files read back), never
  what we intended.

## 6. Accounts, sessions, the child's own page

- `GET /api/me` → `{ account: {id, household_id, display_name, email, role,
  age_bracket, birthdate, theme, effective_theme, self_managed, profile_id,
  created_at}, household: {id, name, created_at}, admin, tenant }` (last two
  are the deprecated aliases the web already types).
- **Member sessions** (role = member) may call only: `GET /api/me`,
  `GET /api/me/today`, `POST /api/me/ask`, `POST /api/auth/logout`,
  `GET /api/auth/config`, `GET /api/catalog`, the step-up/2fa routes. Everything
  else → `403 forbidden_for_member`. Enforced as a layer (fails closed).
- `GET /api/me/today` → `{ used_minutes, earned_minutes, limit_minutes|null,
  left_minutes|null, locked: bool, devices: [{name, status, locked}],
  blocks: AppBlocks, bracket, theme, pending_request: bool,
  bedtime: {start,end}|null, windows: [..] }`.
- `POST /api/me/ask { minutes, reason? }` → creates an earn/time request for
  the member (not for little).
- Device voucher: agent `POST /agent/voucher { os_username }` → server resolves
  `device_users.account_id` for that OS user → voucher bound to **that**
  account. Unlinked OS user → `404 no_account`. `POST /api/auth/voucher`
  issues a session for the bound account (member sessions never start stepped
  up; parent sessions neither).
- `ost login` passes the invoking desktop user (`SUDO_USER`/`$USER`) as
  `os_username`.
- Parent endpoints: `POST /api/members` {display_name, birthdate?, age_bracket?,
  theme?} → member + profile from the bracket preset; `PATCH /api/members/{id}`
  {display_name?, birthdate?, age_bracket?, theme?, profile_id?};
  `DELETE /api/members/{id}`. `GET /api/family` children = **members**
  (key = account id), each with `age_bracket`, `theme`, `effective_theme`,
  `devices`, `locked`, usage, `pending_requests`.
- `GET /api/catalog` (any session) → `catalog::as_json()`.

## 7. Agent-side app/category blocking

- `catalog::expand(&policy.blocks)` → domains go into the dnsmasq ruleset as
  `address=/domain/` (+ `address=/domain/::`) — NXDOMAIN-style sinkhole, applied
  as the **union over active users** (most restrictive wins, as today);
  `custom_domains` likewise. Process names: a 10 s tick that SIGKILLs matching
  comm names owned by a blocked user's uid and emits one `app_blocked` event
  per user/app/day (info). That is the whole native-app story for 0.4.
- Adults (self-managed): blocks apply only to their own OS user; no overlay.

## 8. PAM: parent sudo on a managed machine

- New hidden subcommand `openscreentime pam-auth`: reads the authtok from
  stdin (pam_exec `expose_authtok`), verifies it as a parent code (§4), emits
  the event, exits 0/1. `PAM_USER`/`PAM_SERVICE` go in the event payload.
- `install-service` writes `/etc/pam.d/openscreentime-parent`:
  ```
  auth     required   pam_exec.so expose_authtok quiet /usr/local/bin/openscreentime pam-auth
  account  required   pam_permit.so
  ```
  and `/etc/sudoers.d/10-openscreentime` (0440, validated with `visudo -c`):
  ```
  Defaults:%ost-managed pam_service=openscreentime-parent, timestamp_timeout=0
  %ost-managed ALL=(ALL:ALL) ALL
  ```
  and creates group `ost-managed`. The agent keeps group membership in sync
  every policy apply: OS users whose account bracket is not `adult` are in
  `ost-managed`; adults are not. Effect: a child typing `sudo` is asked for
  the parent's authenticator code, and the parent can administer the machine
  without a local password. Nothing else about sudo changes.
- Removal (`uninstall`) deletes both files and the group.

## 9. Web

- **ChildRules → "Apps & categories"** first: category chips (one click) and
  an app grid (icon + name, one click), fed from `/api/catalog`; custom
  domains under "More". Bracket preset pre-checks. Saves `policy.blocks`.
- **AddChild**: name + birthdate (bracket derived, overridable) → creates the
  member, then the device step shows install command **and the parent-code
  QR** side by side.
- **ChildDetail**: age bracket + theme picker (auto/playful/calm/plain) in the
  header menu; lock state honest (`locked` vs `lock_pending`), devices with
  `online/offline` dots.
- **`/me`** — the child's own page for member sessions (and what a parent sees
  under "My screen time" for their own adult account): today's ring, time
  left, what's blocked, "ask for more time" (kid+), bedtime. Three themes:
  `playful` (little/kid: big round ring, chunky type, bright warm palette,
  gentle motion — Duolingo-energy without a mascot), `calm` (teens: quieter,
  stats), `plain` (adults: compact private dashboard). Member sessions can
  reach nothing else; the rail hides.
- **Settings → Security**: per-device parent code (show QR again / rotate),
  step-up gated.
- Login page: unchanged; `#v=` voucher redemption unchanged.

## 10. Release

- Version **0.4.0** everywhere (server, client, policy, web, CHANGELOG).
- Server image is built on the dev box and loaded onto the host (the LXC has
  one core); `deploy/update.sh` keeps working for the git-pull path.
