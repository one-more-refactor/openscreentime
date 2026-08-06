-- Step-up 2FA (docs/AUTH.md phase 2a): reading is free, every change needs a
-- second factor.
--
-- The session cookie proves WHO you are. A mutation additionally requires a
-- short-lived step-up grant bound to that session, obtained by passing a second
-- factor. Nothing here is enforced by a client; the grant lives on the session
-- row and only the server can set it.

-- ── TOTP, per account ───────────────────────────────────────────────────────
-- The secret is stored base32 exactly as it was handed to the authenticator.
-- It is NOT hashed: a one-way digest cannot generate the next code, so a
-- verifier has to be able to read it. That makes the DB the thing to protect,
-- which it already was (it holds sessions and device tokens).
--
-- totp_confirmed_at is what "enrolled" means: a secret that was generated but
-- never proved with a live code is a secret nobody has, and accepting it would
-- lock the account behind an authenticator that was never actually set up.
ALTER TABLE admins
    ADD COLUMN totp_secret        text,
    ADD COLUMN totp_confirmed_at  timestamptz,
    -- Highest TOTP counter already spent. Codes are single-use: a shoulder-
    -- surfed or shell-history'd code is dead the moment it is used once.
    ADD COLUMN totp_last_counter  bigint NOT NULL DEFAULT 0,
    -- Failed second-factor attempts, and the lockout they earn. Counted here
    -- rather than in memory so a server restart is not a way to clear it.
    ADD COLUMN stepup_fails       integer NOT NULL DEFAULT 0,
    ADD COLUMN stepup_locked_until timestamptz;

-- ── the grant, on the session ───────────────────────────────────────────────
ALTER TABLE admin_sessions
    -- Non-null and in the future = this session may mutate.
    ADD COLUMN stepup_until  timestamptz,
    -- Sliding 7-day sessions need a last-touch to slide from.
    ADD COLUMN last_seen_at  timestamptz NOT NULL DEFAULT now(),
    -- Rotation: on step-up the token is replaced. The superseded hash stays
    -- valid for a short grace so a request already in flight with the old
    -- cookie does not 401 (and so two tabs do not fight).
    ADD COLUMN prev_token_hash    text,
    ADD COLUMN prev_valid_until   timestamptz,
    -- A session minted from a device voucher rather than a passkey. It can
    -- read, and it can step up like any other session — but it never starts
    -- with a grant, because possession of a machine is not possession of the
    -- second factor.
    ADD COLUMN via_voucher   boolean NOT NULL DEFAULT false;

CREATE INDEX admin_sessions_prev_token_hash_idx
    ON admin_sessions (prev_token_hash)
    WHERE prev_token_hash IS NOT NULL;

-- ── emailed step-up codes ───────────────────────────────────────────────────
-- The fallback factor, and the only one available before an authenticator is
-- enrolled. Hashed at rest like every other credential here; attempts are
-- counted per code so guessing a live one costs you the code.
CREATE TABLE stepup_email_codes (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_id    uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    code_hash   text NOT NULL,
    attempts    integer NOT NULL DEFAULT 0,
    consumed_at timestamptz,
    expires_at  timestamptz NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX stepup_email_codes_admin_idx ON stepup_email_codes (admin_id, expires_at DESC);

-- ── device vouchers ─────────────────────────────────────────────────────────
-- Device-voucher autologin (docs/AUTH.md): the installed client, which already
-- holds a device_token, mints a one-time voucher that a local surface on that
-- machine (the browser, the notch) exchanges for a session. Short TTL, single
-- use, hashed at rest.
CREATE TABLE device_vouchers (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id     uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    tenant_id     uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    voucher_hash  text NOT NULL UNIQUE,
    consumed_at   timestamptz,
    expires_at    timestamptz NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX device_vouchers_expiry_idx ON device_vouchers (expires_at);
