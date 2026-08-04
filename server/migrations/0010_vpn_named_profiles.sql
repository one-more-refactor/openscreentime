-- Named VPN profiles (v0.4): upload several per device, exactly one active.
--
-- Replaces the single write-only blob on `devices`. Each profile carries a
-- lifecycle the agent reports back into:
--   untested — stored, never tried
--   testing  — activation pushed; the agent is applying + verifying it
--   active   — the agent brought it up and the tunnel verified
--   failed   — the agent tried it, verification failed, previous state restored
-- `devices.vpn_updated_at` stays as the change stamp feeding policy_version.

CREATE TABLE device_vpn_profiles (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id      uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    name           text NOT NULL,
    kind           text NOT NULL CHECK (kind IN ('wireguard','openvpn')),
    config         text NOT NULL,
    status         text NOT NULL DEFAULT 'untested'
                   CHECK (status IN ('untested','testing','active','failed')),
    last_error     text,
    last_tested_at timestamptz,
    is_active      boolean NOT NULL DEFAULT false,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (device_id, name)
);

CREATE UNIQUE INDEX idx_vpn_one_active
    ON device_vpn_profiles (device_id) WHERE is_active;

-- Carry over the existing per-device configs as an active "imported" profile.
INSERT INTO device_vpn_profiles
    (device_id, name, kind, config, status, is_active, created_at, updated_at)
SELECT id, 'imported', vpn_kind, vpn_config, 'active', true,
       COALESCE(vpn_updated_at, now()), COALESCE(vpn_updated_at, now())
FROM devices
WHERE vpn_kind IS NOT NULL AND vpn_config IS NOT NULL;

ALTER TABLE devices DROP COLUMN vpn_kind;
ALTER TABLE devices DROP COLUMN vpn_config;
