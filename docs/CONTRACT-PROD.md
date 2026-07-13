# Prod push — cross-layer contract (2026-07-12)

Authoritative spec for this session's changes. Server, agent, and web MUST match these shapes
exactly. Existing conventions still apply: every admin JSON response is wrapped in a named
envelope; agent API is Bearer-token; tenant isolation threaded through every query.

## 1. Sessions (server-only change)

Admin sessions move from in-memory HashMap to Postgres.

```sql
CREATE TABLE admin_sessions (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash  text NOT NULL UNIQUE,          -- sha256 hex of the cookie value
    admin_id    uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    tenant_id   uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL
);
```

- Cookie: `sentinel_session`, `HttpOnly`, `SameSite=Lax`, **`Secure` unless env
  `SENTINEL_INSECURE_COOKIES=1`** (dev). TTL 30 days, sliding not required.
- Expired rows deleted lazily on lookup (`DELETE ... WHERE expires_at < now()` opportunistically).
- WebAuthn *challenges* stay in-memory (short-lived, single-process acceptable).
- Logout deletes the row.

## 2. Rate limiting (server-only)

Simple in-memory fixed-window limiter (no new deps), keyed by client IP
(`X-Forwarded-For` first value if `SENTINEL_TRUST_PROXY=1`, else peer addr):

- `/api/auth/*` (all login/register/OIDC starts + finishes): 10 req / 60 s / IP → 429.
- `/agent/enroll`: 5 req / 60 s / IP → 429.

429 body uses the standard error envelope.

## 3. Remote SSH — working end-to-end

### Wire format, agent ↔ server (existing agent WS, `/agent/ws`)

The agent already sends `{"type":"ssh_data","session_id":"<uuid>","data_b64":"<base64>"}`.
**Server adopts `data_b64`** (base64-encoded raw terminal bytes) in BOTH directions. Frames:

- server → agent: `ssh_open {session_id}`, `ssh_data {session_id, data_b64}`,
  `ssh_resize {session_id, cols, rows}`, `ssh_close {session_id}`
- agent → server: `ssh_data {session_id, data_b64}`, `ssh_closed {session_id, exit_code?}`

### Agent PTY

Agent allocates a real PTY (`nix` openpty or `rustix-openpty`; pick the lightest dep already
in-tree if possible), spawns `/bin/bash -l` on the slave, bridges master ↔ WS. Handles
`ssh_resize` via `TIOCSWINSZ`. On child exit sends `ssh_closed`.

### Admin side (new)

- `POST /api/devices/:id/ssh` (exists) → `{ session: { id, status, ... } }`. Status stays
  `opening` until the agent's first `ssh_data`/ack; server marks `open` when the agent WS
  confirms (`ssh_open` is acked by first agent frame or explicit ack — implementer's choice,
  but do NOT mark open unconditionally at creation).
- **New:** `GET /api/ssh/:session_id/ws` — cookie-authenticated WebSocket upgrade for the
  browser terminal.
  - browser → server: **binary frames** = raw keystroke bytes; **text frames** = JSON
    `{"type":"resize","cols":N,"rows":N}`.
  - server → browser: **binary frames** = raw terminal output bytes; text frame
    `{"type":"closed","exit_code":N|null}` before server closes.
  - Server bridges: browser binary → base64 → `ssh_data` to agent; agent `ssh_data` → decode →
    binary to browser. Tenant + admin checked on upgrade; session must belong to tenant.
- `POST /api/ssh/:session_id/close` (replaces unused `closeSsh` shape if different) → closes
  agent side (`ssh_close`), marks row `closed`. Browser WS close also triggers this.
- SSH audit events use **`type = 'ssh'`** (new event type; migration extends the CHECK
  constraint), severity `info`. Stop logging them as `tamper`.

## 4. Earn-time approval — working end-to-end

New table:

```sql
CREATE TABLE earn_requests (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    device_id      uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    device_user_id uuid NOT NULL REFERENCES device_users(id) ON DELETE CASCADE,
    task_id        text NOT NULL,
    task_label     text NOT NULL,
    minutes        int  NOT NULL CHECK (minutes > 0 AND minutes <= 240),
    status         text NOT NULL DEFAULT 'pending'
                   CHECK (status IN ('pending','approved','denied')),
    created_at     timestamptz NOT NULL DEFAULT now(),
    decided_at     timestamptz
);
CREATE INDEX idx_earn_tenant_status ON earn_requests(tenant_id, status);
```

Flow:

1. **Agent → server:** `POST /agent/earn-request` (Bearer) body
   `{"os_username":"…","task_id":"…","task_label":"…","minutes":N}` →
   `{ "request": { "id": …, "status": "pending" } }`. Agent sends this when the user selects
   an earn offer on the lockout screen (headless: offer auto-requested when lockout engages —
   one open request per (user, task) per day, server dedupes by returning the existing pending row).
2. **Web:** `GET /api/earn-requests?status=pending` → `{ requests: [...] }` (joined with device
   name + user display name). Badge in nav; approve/deny buttons.
3. **Admin decision:** `POST /api/earn-requests/:id/approve` | `/deny` → `{ request: {...} }`.
   Approve additionally: inserts `screen_time_ledger` earned_seconds (+minutes*60, upsert on
   (device_user_id, day)) and enqueues command **`credit_time`** payload
   `{"os_username":"…","minutes":N,"request_id":"…"}` to the device. New command type —
   migration extends the commands CHECK constraint.
4. **Agent:** handles `credit_time` → `UsageTracker::add_earned(os_username, minutes)` +
   emits existing `screen_time_earned` event.

## 5. Heartbeat usage persistence

Agent heartbeat body gains `usage: [{"os_username":"…","used_minutes_today":N}]` (replaces the
never-persisted `metrics` field — delete `metrics`). Server upserts into `screen_time_ledger`
(`used_seconds = N*60` for today's row per device_user). Web `DeviceDetail` shows per-user
used/earned today via `GET /api/devices/:id/users` → `{ users: [ { ..., used_minutes_today,
earned_minutes_today } ] }` (this previously-unused endpoint becomes real; joined from ledger).

## 6. OIDC SSO (Authentik)

Env config (all optional; feature off unless all three set):
`SENTINEL_OIDC_ISSUER` (e.g. `https://auth.example.com/application/o/sentinel/`),
`SENTINEL_OIDC_CLIENT_ID`, `SENTINEL_OIDC_CLIENT_SECRET`, optional
`SENTINEL_OIDC_NAME` (display label, default "SSO").

- Discovery via `<issuer>/.well-known/openid-configuration` fetched at startup (reqwest,
  rustls). Authorization-code flow, scopes `openid email profile`.
- `GET /api/auth/config` (public, no auth) → `{ "auth": { "oidc": bool, "oidc_name": "…" } }`.
- `GET /api/auth/oidc/start` → 302 to authorize URL. `state` = random token stored in-memory
  (10-min TTL) mapped to a redirect-back path.
- `GET /api/auth/oidc/callback?code&state` → exchanges code at token endpoint (client_secret
  in POST body), fetches userinfo endpoint with the access token; requires verified `email`.
  - If an admin with that email exists in ANY tenant → create session, redirect `/`.
  - If NO admin exists at all (fresh install) → bootstrap tenant + admin (same path as first
    passkey registration), session, redirect `/`.
  - If admins exist but email unknown → redirect `/login?error=sso_unknown_account` (no
    auto-provisioning of extra admins — this is a family server).
- Redirect URI: `<SENTINEL_PUBLIC_URL>/api/auth/oidc/callback`; new env
  `SENTINEL_PUBLIC_URL` (falls back to RP origin already configured for WebAuthn).

## 7. Passkey management

- `DELETE /api/me/passkeys/:id` → `{ "ok": true }`. Refuse (409, error envelope) to delete the
  **last** credential of an admin **unless** OIDC is enabled (they'd lock themselves out).
- Web Settings gets delete buttons with confirm modal.

## 8. Dead-code and stub removals (decided)

- Server: `RegChallenge.user_id`, `AppError::Forbidden`, `SshBridge` unused fields (fields
  become used by the real bridge or are deleted), heartbeat `metrics`, drain-task skeleton.
- Agent: `device_locked` becomes **read** (bug fix: `enforcement_tick` must not unfreeze
  users while an admin lock is active; admin lock freezes all users / lockout regardless of
  screen-time verdict). `add_earned`/`earned_event`/`EarnOffer.id` become live (earn flow).
  `Challenge::verify`/`answer`, `Nudge.copy`: keep ONLY if the gui feature still compiles and
  uses them; they are behind `--features gui` which stays. Delete anything else unreachable
  in the default build. Config-signature no-op stub: delete the pretend function, leave a
  doc comment.
- Web: delete unused `api.ts` functions that remain unused after this work
  (`deleteDevice` and `listDeviceUsers` become used; `closeSsh` becomes used; `getProfile`
  delete if still unused), `Device.online` type field.
- Presets: add a Rust test asserting each preset JSON round-trips through `sentinel_policy::Policy`
  with no unknown-field loss (serialize(parse(x)) == normalize(x)) so drift is caught.
- Command-ack logic: single shared fn used by both HTTP ack and WS ack paths.

## 9. UI feature visibility

- `app_limits` is not enforced by the agent → REMOVE the app-limits section from PolicyEditor
  and mark the field deprecated in docs (type stays in the policy crate for forward compat).
- Discovery vendor/hostname stubs: show "—" instead of fake values.
- Mock fallback (`read()` → mock.ts) STAYS, but only when `import.meta.env.VITE_USE_MOCK === "1"`
  — never silently in prod builds.

## 10. Migration file

All schema changes above land in `server/migrations/0002_prod.sql`: admin_sessions,
earn_requests, commands CHECK += 'credit_time', events CHECK += 'ssh' and += 'earn_request'
(used for audit trail of requests/decisions).

## 11. Network lockdown + parent PIN (v1 prod)

Two new fields on the shared Policy document (`sentinel-policy` crate; mirrored in
`web/src/types.ts`). Both are optional and omitted from serialized output when unset, so
existing preset JSON stays byte-identical (the preset drift guard depends on this).

### `lockdown` — network anti-bypass toggles
```
"lockdown": {
  "force_dns":  bool,   // drop plaintext DNS (udp/tcp 53) egress to anything but the agent's upstream
  "block_doh":  bool,   // drop the well-known public DoH resolver IPs (except the configured upstream)
  "block_dot":  bool,   // drop DNS-over-TLS (tcp/udp 853)
  "block_tor":  bool,   // drop Tor ports {9001,9030,9050,9051,9150} + NXDOMAIN .onion/torproject.org
  "block_vpn":  bool    // drop WireGuard 51820, OpenVPN 1194, IPsec/IKE 500/4500
}
```
Enforced by the agent in `enforce/firewall.rs` (nft DROP rules placed before the generic
accepts, first-match-wins) and `enforce/dns.rs` (Tor domain sink). Omitted entirely when every
flag is false. Kids preset enables all five; teen enables block_doh/block_dot/block_tor.

### `parent_pin_hash` — local escape hatch
```
"parent_pin_hash": "<argon2 PHC string>"   // absent when no PIN set
```
Set server-side: the profile create/update request accepts a **plaintext** `parent_pin` field
(sibling of `name`/`policy`, never inside the policy object). The server argon2-hashes it and
stores only the hash in the policy jsonb. Semantics: absent `parent_pin` preserves the existing
hash; `""` clears it; a non-empty value (≥4 chars) sets a new hash. The plaintext is never
stored or returned.

The agent verifies an entered PIN against this hash locally (works offline): (a) as a master
override on the lockout overlay, and (b) via `sentinel-agent unlock --pin <PIN> [--minutes N]`,
which suspends enforcement (tears down the nft table, un-pins resolv.conf) for N minutes. Root
required for the CLI unlock.

### Fail-closed offline behavior
The agent tracks last successful server contact. Beyond a grace window
(`SENTINEL_OFFLINE_GRACE_SECS`, default 900) it keeps the last-known policy fully enforced and
re-asserts it every loop (so nothing drifts open while the command server is unreachable), emits
a `network_offline` tamper event once, and a `network_online` event on recovery. It does NOT
black out all traffic — the device stays usable under its existing strict allowlist.
