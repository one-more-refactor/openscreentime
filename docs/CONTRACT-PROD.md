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

## 3. Remote SSH — REMOVED in v0.4

This section specified the remote-shell feature end-to-end: the agent PTY, the
`ssh_open`/`ssh_data`/`ssh_resize`/`ssh_close`/`ssh_closed` WS frames,
`POST /api/devices/:id/ssh`, `GET /api/ssh/:session_id/ws`,
`POST /api/ssh/:session_id/close`, and the `ssh_sessions` table. The whole
capability was removed in v0.4 (`0008_remove_ssh.sql`) — there is no remote
shell anymore; everything is UI-only. Historical events of `type = 'ssh'`
remain readable in the event log as the record of past sessions. A possible
replacement (secure reverse tunnel, native SSH+RDP) was considered and
deferred. The section number is kept so existing cross-references ("contract
§3") stay resolvable.

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
(used for audit trail of requests/decisions). Since v0.4, `'ssh'` is a historical-only
event type — the capability behind it is gone (see §3), but the rows and the CHECK entry stay.

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

## 12. Per-user tray companion (`sentinel-agent tray`, feature `tray`)

The root agent publishes an atomically-replaced, world-readable snapshot at
`/run/sentinel/status.json` every tick (connection state, device/user lock+freeze state,
per-user used/remaining minutes; `remote_shell_open` was dropped with the shell in v0.4,
see §3). A feature-gated subcommand
(`cargo build --features tray`, `ksni` + `notify-rust`, session bus only, **no root**) polls it
every 5s and renders:

- a StatusNotifierItem using themed freedesktop icons (`security-high` online/unlocked,
  `security-medium` offline-within-grace, `security-low` fail-closed/locked/frozen/lockdown),
  tooltip `TIME LEFT: NN MIN · ONLINE` (or `NO LIMIT` / `PAUSED`);
- a read-only menu (time left, connection) plus
  `ABOUT SENTINEL` → notification "This device is managed. Screen time and network filtering
  are active.";
- desktop notifications on state **transitions only**: remaining time crossing ≤10/≤2 min,
  pending freeze countdown, frozen on/off, fail-closed/back-online, device lock/lockdown
  on/off.

`install-service` best-effort drops `client/systemd/sentinel-tray.service` into
`/etc/systemd/user/`; each desktop user opts in with `systemctl --user enable --now sentinel-tray`
(never auto-enabled).

## 12. Parent daily jobs (2026-07-14)

- **Grant extra time:** `POST /api/device-users/:id/credit-time` body `{ "minutes": 1..=240 }`
  → `{ ok, minutes }`. Same mechanics as an approved earn request: upserts today's
  `screen_time_ledger.earned_seconds` and enqueues `credit_time`
  `{ os_username, minutes, request_id: null }` (no earn request exists — agents already
  tolerate a null/absent `request_id`). Audited as an `earn_request` event,
  `payload.action = "granted"`.
- **Enroll-token TTL + regen:** `devices.enroll_token_expires_at` (migration
  `0004_enroll_token_ttl.sql`, additive). Tokens are issued with a 24 h TTL on create and via
  the new `POST /api/devices/:id/enroll-token` (409 unless the device is still `pending`).
  `/agent/enroll` rejects expired tokens like consumed ones (401).
- **Truthful lock state:** lock/unlock responses are
  `{ command_id, queued: true, delivered: bool }`. `devices.status` flips immediately only when
  the command reached a live agent WS; a queued command flips the status when the agent **acks**
  it (`lock` acked → `locked`, `unlock` acked → `online`). Web shows a "LOCK PENDING" chip
  instead of optimistically flipping the card.
- **Offline sweeper:** background task, every 60 s:
  `status = 'online' AND last_seen < now() - 3 min` → `offline`. Catches dead poll-mode agents.
  Web escalates devices offline ≥ 7 days to a red "GONE DARK Nd" badge (card + fleet strip).
- **`lockdown.offline_lockdown_days`** (policy crate, already shipped) is now editable in the
  web PolicyEditor; the field is omitted from the serialized policy when 0 so preset JSON stays
  byte-identical.

## 13. Agent distribution + self-update (2026-07-14)

- **Shipped binary:** the container image builds `client/` (default features only) as a
  true static `x86_64-unknown-linux-musl` binary and stages it under `/app/agent` with a
  `manifest.json` (`{version, artifacts:[{target, features, url, sha256}]}`). The gui/tray
  features (eframe/glow; ksni → libdbus-sys) link C system libraries and are NOT musl-built;
  desktop builds come from source until CI exists. Headless is enforcement-complete.
- **Serving:** `GET /api/agent/latest`, `GET /api/agent/download/:file` (bare-filename only,
  traversal rejected), `GET /install.sh` — public, rate-limited 30/60 s/IP
  (`server/src/agent_dist.rs`; installer source of truth is `server/install.sh`).
- **Self-update (client `update.rs`):** ~2 min after startup, then daily: fetch the manifest;
  if `version` is newer than `CARGO_PKG_VERSION` and a `x86_64-linux-musl`/`headless`
  artifact exists, download → verify sha256 of the exact bytes → stage as
  `/usr/local/bin/.sentinel-agent.new` → keep old as `sentinel-agent.bak` → atomic rename →
  emit `tamper` info event `agent_updated` (old→new) → `systemctl restart sentinel-agent`
  via `Exec` (dry-run safe). Only runs when the process IS `/usr/local/bin/sentinel-agent`
  and the build is headless x86_64. Gates: `auto_update = true` (agent.toml, default) and
  `SENTINEL_NO_SELF_UPDATE=1` kill switch.
- **Trust model v1 (decided):** artifact integrity = sha256-over-TLS from the enrolled
  server. A compromised server therefore compromises the fleet — this is ALREADY the trust
  reality (the server can push arbitrary root commands to agents), so self-update does not
  widen the blast radius. v2 should pin a minisign/ed25519 signing key in the agent so
  binaries verify independently of the transport.
- **Rollback:** systemd `Restart=` + the watchdog timer is the safety net for a broken
  binary; `install-service` re-copies the running binary (unchanged); manual rollback:
  `mv /usr/local/bin/sentinel-agent.bak /usr/local/bin/sentinel-agent && systemctl restart
  sentinel-agent`.
- **Registration lockdown:** register start/finish → 403 `registration_closed` once ≥1 admin
  exists, unless `SENTINEL_OPEN_REGISTRATION=1`; a valid session whose admin email matches
  the request email bypasses (Settings add-passkey flow). First boot (0 admins) stays open.
