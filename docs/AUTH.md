# OpenScreenTime — Auth & User Management design

The contract for the auth rework (step 2 of `OPENSCREENTIME.md`). Grounded in
what the server actually does today. When this conflicts with older docs, this
wins for auth.

## Today (as-built)

- **Sessions:** opaque token in an HttpOnly cookie `sentinel_session`; DB table
  `admin_sessions` stores it sha256-hashed. **Fixed 30-day TTL, no rotation, no
  refresh.** Validated by the `AuthAdmin` extractor. Challenge state (WebAuthn)
  is **in-memory** → single-instance only.
- **Passkeys:** webauthn-rs 0.5, table `webauthn_credentials`. Registration
  closes after the first admin unless `SENTINEL_OPEN_REGISTRATION=1`. OIDC SSO
  is hand-rolled.
- **Accounts:** `tenants` (the family) → `admins` (parents — **no role column,
  no password**) → `device_users` (OS accounts on devices — the kids, **cannot
  log in**). No RBAC: every admin is fully powerful within its tenant. Isolation
  is app-layer `WHERE tenant_id = …` on every query.
- **Second factor / email:** **none anywhere.** No TOTP, no email sender. The
  device recovery PIN (argon2) is offline-unlock only, not account 2FA.
- **Agent:** `enroll_token` (plaintext, 24h) → `device_token` (bearer, hashed).
  Binary `sentinel-agent`, env `SENTINEL_TOKEN`.

## Target

- **Everyone has an account** with a **role** and an **age bracket**.
- **Sign in** two ways (user's choice): **passkey** OR **device-voucher
  autologin** (the installed client authenticates the browser on that machine).
- **Reading is frictionless; every change needs step-up 2FA.**
- **The server validates everything.** Rotating **7-day** sessions.
- **Scrapped for v1:** SMS/phone, QR device-pairing.

## The core invariant: read-free, write-stepped

- The session cookie proves **who** you are (identity).
- A **mutation** additionally requires a valid **step-up grant**: a short-lived
  (≈5 min) server-issued marker bound to the session, obtained by passing a
  second factor.
- Enforced **server-side** by a `StepUp` extractor on every mutating `/api`
  handler — never by the client. Missing/expired grant → `428 step_up_required`.
- Client reaction: catch `step_up_required`, open the **StepUp modal**, verify a
  factor, retry the original request. One reusable component, one interceptor.

## Second factors (v1)

- **TOTP (authenticator app):** per-account secret, RFC 6238 (30 s, ±1 window),
  provisioned via `otpauth://` (secret string shown; a QR of that string is a
  convenience for scanning into the app — this is *not* the scrapped device
  pairing), confirmed once before it counts.
- **Email token:** 6–8 digit single-use code, short TTL. **Dev:** emitted to the
  server log (no SMTP needed to build). **Prod:** pluggable sender (lettre or an
  outbound webhook).
- Account picks which; email is the fallback when no authenticator is enrolled.

## Sessions (rotating)

- Replace fixed-30-day with **7-day sliding + rotation**: rotate the token on
  step-up (and past a use-threshold), invalidate the old with a short grace
  window, track a token family for reuse detection. Keep sha256-at-rest.

## Device-voucher autologin

- Contract: **voucher in → session out, server-verified.** The installed client
  (holding the hashed `device_token`) mints a one-time voucher the local browser
  can read; the browser exchanges it at `POST /api/auth/voucher`; the server
  verifies the device_token *and* that the requesting account is permitted on
  that device, then issues a session. The exact local hand-off (loopback broker
  vs. registered local origin) is settled in 2a implementation; the server
  contract above is fixed.

## API contract (new / changed)

- `GET /api/me` → extended with `role`, `age_bracket`, `household`.
- `POST /api/auth/voucher {voucher}` → session (device-voucher login).
- `GET  /api/me/2fa` → `{ totp_enrolled, email_available }`
- `POST /api/me/2fa/totp/start` → `{ secret, otpauth_uri }`
- `POST /api/me/2fa/totp/confirm { code }` → ok
- `POST /api/auth/stepup/email/start` → sends code
- `POST /api/auth/stepup/verify { method, code }` → sets step-up grant, returns expiry
- All mutating `/api/*` now require `StepUp`; without it → `428 step_up_required`.

## Phasing

- **2a — Mechanics on current (parent) accounts. BUILT** (server:
  `server/src/stepup.rs`, migration `0012_stepup_2fa.sql`). Session rotation on
  step-up with a 2-minute grace on the superseded token; TOTP (RFC 6238, ±1
  window, single-use counter) and emailed single-use codes; failure counting
  with a doubling lockout; device vouchers. The UI half was already built
  against the mock and needs no change.

  Two decisions taken during implementation, both deliberate departures from
  the sketch above:

  * **A layer, not a per-handler extractor.** The invariant is "no mutation
    without a grant". A layer over `/api` with a small explicit exempt list (the
    auth flow itself) delivers that and *fails closed for routes nobody has
    written yet* — a new mutating endpoint is guarded the day it exists, with no
    chance of forgetting a parameter. `require_step_up` documents the exempt
    list.
  * **A voucher session starts with no grant.** Possession of a machine is not
    possession of the second factor, so device-voucher autologin buys reading
    and identity only; changing anything still needs a code. This is what makes
    it safe for a local surface (the notch) to hold a session permanently.
  * **Confirming an authenticator is itself a step-up.** You just proved the
    factor; making you wait out the 30-second window before your first change
    would be friction with no security in it.
- **2b — Everyone has an account.** Role + age bracket + birthdate; per-person
  login; adults' private self-tracking; link `device_users` ↔ accounts.
- **2c — Identifier rename + hardening.** `sentinel-agent` → `openscreentime-agent`,
  `SENTINEL_TOKEN` → `OST_TOKEN` (with back-compat); hash `enroll_token` at rest
  (open red-team item); move WebAuthn challenge state out of memory.

## Build approach

UI-first against mock for 2a's **sign-in + StepUp modal** (fast, gorgeous, low
risk), then wire the Rust backend to the same contract. The final authority is
always the server.

## Sensitive reads (the one exception to "reading is free")

Security & access data — passkey inventory, 2FA enrollment state, parent
pairing tokens — is takeover material, so these GETs are the one class of
*read* the server also answers with `428 step_up_required` unless the session
holds a live step-up grant:

- `GET /api/auth/passkeys`
- `GET /api/me/2fa`
- `GET /api/parent-tokens`

The web console mirrors this honestly: the Security & access section of
Settings mounts (and fires these fetches) only after `requireStepUp()`
resolves. The client gate is comfort; the 428 is the lock.
