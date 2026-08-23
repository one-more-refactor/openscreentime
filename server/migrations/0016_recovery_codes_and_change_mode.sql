-- 0.5.0: the console owns the keys (docs/CONTRACT-0.5.md).
--
-- Recovery codes: eight one-time 8-digit codes per device, shown once in the
-- console after a step-up, verified OFFLINE by the agent. Stored as
-- hex HMAC-SHA256(key = the device's decoded TOTP secret, msg = the 8 ASCII
-- digits), so a database leak yields nothing without the per-device secret
-- (and the secret alone is already "root on that device", nothing more).
-- Replaces the single recovery PIN minted at enroll (0011) — that column stays
-- for devices that still carry one; nothing new is written to it.
CREATE TABLE device_recovery_codes (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id  uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    idx        smallint NOT NULL,
    mac        text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    used_at    timestamptz
);
CREATE INDEX device_recovery_codes_device_idx ON device_recovery_codes (device_id);

-- Change mode: a step-up grant is now 15 minutes and may be extended ONCE per
-- grant from the console. The flag is reset whenever a fresh grant is issued.
ALTER TABLE admin_sessions ADD COLUMN stepup_extended boolean NOT NULL DEFAULT false;
