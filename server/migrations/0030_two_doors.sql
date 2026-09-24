-- Sign-in, simplified (docs/AUTH.md): two doors — a code shown on your own
-- computer, or a passkey — and one confirm (a passkey or a code). No
-- authenticator apps, no Telegram taps, no change mode.

-- ── 1. Name → a code on your own computer ──────────────────────────────────
-- The browser no longer shows numbers to tap on the device; the person's own
-- computer shows a 6-digit code and the browser types it in. A request for an
-- unknown name (or for someone with no computer online) is a real row with no
-- account — a decoy whose code went nowhere — so every answer the browser can
-- get (wrong code, too many tries, expired) is the same whether the name
-- exists or not.
DROP TABLE login_requests;
CREATE TABLE login_codes (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Both NULL = a decoy.
    tenant_id      uuid REFERENCES tenants(id) ON DELETE CASCADE,
    account_id     uuid REFERENCES admins(id) ON DELETE CASCADE,
    -- 'login'  : the browser holding the PKCE verifier gets a session.
    -- 'confirm': the signed-in session that asked gets its confirm window.
    purpose        text NOT NULL CHECK (purpose IN ('login', 'confirm')),
    code_challenge text,
    session_id     uuid REFERENCES admin_sessions(id) ON DELETE CASCADE,
    -- sha256("<id>:<code>"): the code itself is never stored.
    code_hash      text NOT NULL,
    attempts       integer NOT NULL DEFAULT 0,
    used_at        timestamptz,
    created_at     timestamptz NOT NULL DEFAULT now(),
    expires_at     timestamptz NOT NULL
);
CREATE INDEX login_codes_account_idx ON login_codes (account_id, created_at);
CREATE INDEX login_codes_expiry_idx ON login_codes (expires_at);

-- "Sign in with a passkey" asks for no name first: the credential says whose
-- it is, so passkeys are now looked up by credential id.
CREATE INDEX webauthn_credentials_credential_idx ON webauthn_credentials (credential_id);

-- ── 2. Account recovery ─────────────────────────────────────────────────────
-- `openscreentime-server recover <name>` (root inside the container) mints a
-- one-time sign-in link for an EXISTING account. Hashed at rest, short-lived.
CREATE TABLE signin_links (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    account_id  uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    token_hash  text NOT NULL UNIQUE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    consumed_at timestamptz
);

-- ── 3. Whose login is the owner's ──────────────────────────────────────────
-- "This is Mia's computer" used to link EVERY unmatched OS login on it to Mia
-- (so the parent's admin login became Mia), and a "my computer" would have
-- linked a child's login to the parent. Now exactly one login is the owner's,
-- settled at enrollment; every other login is its own person.
ALTER TABLE devices ADD COLUMN owner_os_username text;
-- A login that became a person of its own because nobody could say whose it
-- is. The Family page asks a parent to sort it (Devices → Who's who); on a
-- parent's own computer such a person's rules enforce nothing meanwhile.
ALTER TABLE device_users ADD COLUMN unsorted boolean NOT NULL DEFAULT false;

-- A parent is sent codes and vouchers only for the owner's login, so the old
-- links must not carry over as "the owner's": on a parent's own computer with
-- ONE login linked to the parent, that login is the owner's. With several
-- nobody can say which one is theirs (the child's login got linked the same
-- way), so none is: all of them are unlinked, and the startup backfill makes
-- each a person of its own — a member by name, else an unsorted person whose
-- rules enforce nothing (one of them is the parent's own login: never lock a
-- parent out) — until the parent points their own login back at themselves
-- under Devices → Who's who (behind confirm-it's-you). Nothing is guessed onto
-- a parent, and no child's rules are guessed onto anyone.
UPDATE devices d
   SET owner_os_username = one.os_username
  FROM (SELECT du.device_id, min(du.os_username) AS os_username
          FROM device_users du
          JOIN devices dv ON dv.id = du.device_id
          JOIN admins a ON a.id = dv.owner_account_id
         WHERE du.account_id = dv.owner_account_id AND a.role <> 'member'
         GROUP BY du.device_id
        HAVING count(*) = 1) one
 WHERE d.id = one.device_id;

UPDATE device_users du
   SET account_id = NULL
  FROM devices d
  JOIN admins a ON a.id = d.owner_account_id
 WHERE du.device_id = d.id
   AND du.account_id = d.owner_account_id
   AND a.role <> 'member'
   AND d.owner_os_username IS NULL;

-- What the agent says it understands beyond the basics (`login_code`, …),
-- from its `state` frame or heartbeat. NULL: it never said — an agent from
-- before sign-in codes, which a code is never sent to.
ALTER TABLE devices ADD COLUMN agent_features text[];

-- ── 4. Gone ─────────────────────────────────────────────────────────────────
-- Authenticator apps (TOTP) and their failure counters, the Telegram confirm
-- tap, the long-retired emailed codes, change-mode extension, and untrusted
-- sessions (every login ceremony has produced trusted sessions since 0017).
-- Device unlock codes (devices.parent_totp_secret) are untouched.
ALTER TABLE admins
    DROP COLUMN totp_secret,
    DROP COLUMN totp_confirmed_at,
    DROP COLUMN totp_last_counter,
    DROP COLUMN stepup_fails,
    DROP COLUMN stepup_locked_until;
DROP TABLE telegram_verifications;
DROP TABLE stepup_email_codes;
DELETE FROM admin_sessions WHERE NOT trusted;
ALTER TABLE admin_sessions
    DROP COLUMN trusted,
    DROP COLUMN stepup_extended;

-- ── 5. Types the constraints never allowed ─────────────────────────────────
-- `login_code` is new. `ping` (the Devices page's liveness probe) and the
-- `account_login` alert were written by 0.6 code but rejected here, so both
-- failed at runtime.
ALTER TABLE commands DROP CONSTRAINT commands_type_check;
ALTER TABLE commands ADD CONSTRAINT commands_type_check CHECK (type IN
    ('lock','unlock','apply_policy','set_tamper_level','credit_time','deny_earn',
     'ping','login_code'));

ALTER TABLE events DROP CONSTRAINT events_type_check;
ALTER TABLE events ADD CONSTRAINT events_type_check CHECK (type IN
    ('heartbeat','tamper','lock','unlock','policy_applied',
     'screen_time_exceeded','screen_time_earned',
     'enrolled','ssh','earn_request','evasion',
     'enforcement_degraded','vpn_profile',
     'parent_code_ok','parent_code_failed','parent_code_backup_used',
     'app_blocked','member','account_login',
     -- kept from 0026: the retired number-match flow's type, and `other`
     -- (an event type from a newer agent, original type in the payload).
     'login_approval','other'));
