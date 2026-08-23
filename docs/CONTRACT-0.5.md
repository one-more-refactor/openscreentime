# 0.5.0 build contract — "the console owns the keys"

Shared contract for the 0.5.0 push. Two workstreams build against this in
parallel (A: server + client, B: web). When it disagrees with older docs, this
wins; `docs/CONTRACT-0.4.md` still describes everything not mentioned here.

Scope, in one breath: the per-device **unlock code** is owned by OpenScreenTime
— the parent reads the rotating 6-digit code (and one-time **recovery codes**)
from the console after proving it's them, never from a third-party
authenticator; the console gets an explicit **change mode** (enter a second
factor once, change things for 15 minutes, lock it down again — with a
full-screen enter/leave animation) instead of a popup per change; and the
web gets a **consistency & depth pass** (same system, less flat).

Naming, everywhere a person can read it: **unlock code** = the rotating
6-digit code; **recovery code** = a one-time 8-digit backup code. Internals
keep their names (`parent_totp_secret`, event types `parent_code_*`, client
module `parentcode`, the `parent-code` flag names in the client CLI may stay
as aliases).

---

## 1. Unlock codes — the secret never leaves the server/device

The device TOTP secret (`devices.parent_totp_secret`, RFC 6238 as in 0.4)
stays; what changes is who holds it: **only the server and the agent.** No
QR, no `otpauth://`, no `secret` field in any API response.

### Server (A)

- `POST /api/devices` no longer returns `parent_code`.
- `GET /api/devices/{id}/unlock-code` — sensitive read (step-up gated, like
  `/parent-code` was; replace that path in `stepup::sensitive_read`) →
  ```json
  { "code": "123456", "seconds_left": 17, "period": 30, "device_name": "Kid laptop" }
  ```
  `code` is `totp_at(secret, now_counter())`. The old `/parent-code` routes
  are removed.
- `POST /api/devices/{id}/unlock-code/rotate` (mutation → step-up) → same
  shape plus `"recovery_codes_cleared": true`. New secret, enqueue
  `apply_policy`, event `parent_code_ok {action:"rotated"}`. Rotating also
  **deletes the device's recovery codes** (they are keyed by the secret).
- Recovery codes — migration `0016_recovery_codes_and_change_mode.sql`:
  ```sql
  CREATE TABLE device_recovery_codes (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id  uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    idx        smallint NOT NULL,
    mac        text NOT NULL,            -- hex HMAC-SHA256(key = base32-decoded totp secret, msg = 8 digits)
    created_at timestamptz NOT NULL DEFAULT now(),
    used_at    timestamptz
  );
  CREATE INDEX ON device_recovery_codes (device_id);
  ```
  - `POST /api/devices/{id}/recovery-codes` (step-up) → replaces the whole
    set with 8 fresh codes, returned **once**:
    `{ "codes": ["1234 5678", …8], "generated_at": ts }`. Enqueue
    `apply_policy`; event `parent_code_ok {action:"recovery_codes_generated"}`.
  - `GET /api/devices/{id}/recovery-codes` (sensitive read) →
    `{ "unused": 5, "total": 8, "generated_at": ts|null }`.
  - Device JSON (`GET /api/devices`, `/api/family`) gains
    `"recovery_codes_unused": n` (0 when none generated).
- Agent pull `GET /agent/policy`:
  `parent_code: { totp_secret, recovery_codes: [{ "id": uuid, "mac": hex }] }`
  (unused only). **Stop minting the device recovery PIN at enroll**: no
  `recovery_pin` in the enroll response, and `recovery_pin_hash` is no longer
  injected as `parent_pin_hash` (a profile-level `parent_pin_hash`, set
  deliberately, still passes through).
- `POST /agent/events` with `type: "parent_code_backup_used"` and payload
  `{ "recovery_id": uuid, … }` → `UPDATE … SET used_at = now()` for that
  device's code (ignore unknown ids). Event is stored as before.

### Client (A)

- `policy::ParentCode { totp_secret, #[serde(default)] recovery_codes: Vec<RecoveryCode { id: String, mac: String }> }`.
- `parentcode::Verifier` learns recovery codes: digits-only the input, if it
  is 8 digits compute HMAC-SHA256 over the ASCII digits with the decoded
  secret, compare constant-time to each unused mac → `Verdict::Recovery(id)`.
  Used ids persist in the state file (`used_recovery: Vec<String>`) so a code
  is single-use offline too. Legacy `Verdict::Backup` (argon2 `parent_pin_hash`)
  stays for profile-level pins. `event()` for Recovery = type
  `parent_code_backup_used`, payload `{ via, user, recovery_id }`.
- Every prompt / label / help text: "Unlock code" — "the 6 digits from the
  OpenScreenTime console on your phone (open this computer → Unlock code), or
  one of its recovery codes". Overlay parent field, `ost unlock`, tray, PAM,
  `install-service` comments, enroll output (no PIN printed any more — print
  where the codes live instead). Remove the "this server did not issue a
  backup code" warning.
- Version 0.5.0 (workspace crates).

## 2. Change mode (the console's explicit edit state)

### Server (A)

- `GRANT_MINUTES` = 15.
- `GET /api/auth/stepup` → `{ "armed_until": ts|null, "extended": bool }`
  (plain read; lets a reloaded console restore its state).
- `POST /api/auth/stepup/lock` (exempt from the guard) → clears
  `stepup_until`, `{ "armed_until": null }`.
- `POST /api/auth/stepup/extend` (guarded, i.e. needs the live grant) →
  once per grant: `admin_sessions.stepup_extended boolean NOT NULL DEFAULT
  false` (migration 0016), reset to false on every fresh grant. Second call →
  `409 already_extended`. Response `{ "armed_until": ts, "extended": true }`.
- `POST /api/auth/stepup/verify` response adds `"extended": false`.

### Web (B)

- `lib/stepup.tsx` → `lib/changemode.tsx` exporting `ChangeModeProvider`,
  `useChangeMode()` = `{ armed, armedUntil, extended, enter(), lock(),
  extend(), guard(fn), requireStepUp() }` (`guard`/`requireStepUp` keep their
  semantics: a locked control's first click opens the "Turn on change mode"
  dialog, then runs; nothing pops up again while armed).
- Rail footer (and the mobile drawer): one control. Locked: a small closed
  lock + "Locked" + "Make changes" button. Armed: open lock + "Change mode ·
  14:59" countdown + "Lock" (and "Extend" while `!extended`). The floating
  mobile trigger shows an armed dot.
- On mount: `GET /api/auth/stepup` restores `armedUntil`.
- **Full-screen animations**: entering change mode plays a ≈1.1 s veil (ink
  field, large mono "CHANGE MODE", lock glyph stroke-opening, one ring sweep —
  the same motion family as Pause Everything), then the app is revealed
  already unlocked. Locking (manual or auto) plays the reverse in ≈0.7 s.
  `prefers-reduced-motion` → a 150 ms fade, no choreography.
- Locked styling audit: every control that mutates sits at the same reduced
  presence while locked (the `[data-stepup="locked"]` rule today misses some);
  pure-read controls carry `.no-code`.

## 3. Consistency & depth pass (B)

The design is right; it is flat. Keep the system, add depth and make it
uniform:

- Tokens: `--elev-1`, `--elev-2` (light: soft ambient shadows; dark: surface
  step + `--line-2` edge + a faint inset top highlight), used by cards,
  panels, modals, drawer, rail. Hover lift on interactive cards
  (`translateY(-1px)` + edge to `--line-2`, `--dur-tick`), pressed state on
  buttons. Update the header comment in `theme.css` ("no shadows" → ambient
  elevation only, via tokens).
- Same `PageHeader` on every page, same section rhythm, same label voice,
  `Button` variants instead of ad-hoc inline-styled buttons, same empty-state
  and inline-status components. `/me` and `/login` get the same elevation
  language (the kid themes keep their warmth).
- Unlock-code UI replaces every QR/secret surface: in **AddChild step 2** and
  **Settings → Unlock codes**, a device row opens a panel with the live
  6-digit code (ring countdown, refetch each period), "Recovery codes"
  (generate → show once with print/copy, count of unused), and "Replace"
  (rotate, with the recovery-codes-cleared warning). Mock API mirrors all of it.
- Types/API: `UnlockCode`, `RecoveryCodes`, `ChangeModeStatus`; remove
  `ParentCode`, `getParentCode`, `rotateParentCode`, the `QrCode` usage for it.
- Version 0.5.0 in `web/package.json`.

## 4. Release

- `CHANGELOG.md` `## [0.5.0]`; docs: AUTH.md (change mode, unlock codes),
  AGENT.md, API.md, PROFILES.md, README screenshots/text where they mention
  the QR or authenticator for devices.
- Smoke script `server/scripts/smoke-0.4.sh` grows checks for the four new
  endpoints (rename is optional).
