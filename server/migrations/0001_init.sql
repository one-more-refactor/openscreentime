-- Sentinel initial schema. Mirrors docs/DATA_MODEL.md exactly.
-- All ids are uuid v4, all timestamps timestamptz. Tenant isolation is enforced
-- in the application layer (every query filters by tenant_id from the session).

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- tenants -------------------------------------------------------------------
CREATE TABLE tenants (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- admins (passkey-only, no password column) ---------------------------------
CREATE TABLE admins (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    email        text NOT NULL UNIQUE,
    display_name text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
);

-- webauthn_credentials (one row per registered passkey) ---------------------
CREATE TABLE webauthn_credentials (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_id      uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    credential_id bytea NOT NULL,
    passkey       jsonb NOT NULL,
    nickname      text NOT NULL DEFAULT '',
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_used_at  timestamptz
);
CREATE INDEX idx_webauthn_admin ON webauthn_credentials(admin_id);

-- profiles (policy presets + custom) ----------------------------------------
CREATE TABLE profiles (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id  uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name       text NOT NULL,
    kind       text NOT NULL CHECK (kind IN ('kids','teen','default','custom')),
    is_preset  bool NOT NULL DEFAULT false,
    policy     jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_profiles_tenant ON profiles(tenant_id);

-- devices -------------------------------------------------------------------
CREATE TABLE devices (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name          text NOT NULL,
    hostname      text NOT NULL DEFAULT '',
    os            text NOT NULL DEFAULT '',
    agent_version text NOT NULL DEFAULT '',
    status        text NOT NULL DEFAULT 'pending'
                  CHECK (status IN ('pending','online','offline','locked')),
    tamper_level  int  NOT NULL DEFAULT 1 CHECK (tamper_level IN (1,3)),
    device_token  text,                       -- sha256 hex of the bearer token
    enroll_token  text,                       -- one-time, null after enrollment
    public_ip     inet,
    last_seen     timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_devices_tenant ON devices(tenant_id);
CREATE INDEX idx_devices_token ON devices(device_token);
CREATE UNIQUE INDEX idx_devices_enroll ON devices(enroll_token) WHERE enroll_token IS NOT NULL;

-- device_users (per-OS-user policy — zero trust is per person) ---------------
CREATE TABLE device_users (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id    uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    os_username  text NOT NULL,
    display_name text,
    profile_id   uuid NOT NULL REFERENCES profiles(id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (device_id, os_username)
);
CREATE INDEX idx_device_users_device ON device_users(device_id);

-- commands (server -> agent queue) ------------------------------------------
CREATE TABLE commands (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id  uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    type       text NOT NULL CHECK (type IN
                 ('lock','unlock','apply_policy','ssh_open','ssh_close','discover','set_tamper_level')),
    payload    jsonb NOT NULL DEFAULT '{}'::jsonb,
    status     text NOT NULL DEFAULT 'queued'
               CHECK (status IN ('queued','sent','acked','failed')),
    result     jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    acked_at   timestamptz
);
CREATE INDEX idx_commands_device_status ON commands(device_id, status);

-- events (agent -> server telemetry & audit) --------------------------------
CREATE TABLE events (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    device_id      uuid REFERENCES devices(id) ON DELETE CASCADE,
    device_user_id uuid REFERENCES device_users(id) ON DELETE SET NULL,
    type           text NOT NULL CHECK (type IN
                     ('heartbeat','tamper','lock','unlock','policy_applied',
                      'screen_time_exceeded','screen_time_earned','streak',
                      'enrolled','discovery_result')),
    severity       text NOT NULL DEFAULT 'info' CHECK (severity IN ('info','warn','critical')),
    payload        jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_events_tenant_created ON events(tenant_id, created_at DESC);
CREATE INDEX idx_events_device ON events(device_id);
CREATE INDEX idx_events_type ON events(type);

-- ssh_sessions (reverse-tunnel bookkeeping) ---------------------------------
CREATE TABLE ssh_sessions (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id   uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    admin_id    uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    broker_port int  NOT NULL,
    status      text NOT NULL DEFAULT 'opening'
                CHECK (status IN ('opening','open','closed','failed')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    closed_at   timestamptz
);
CREATE INDEX idx_ssh_device ON ssh_sessions(device_id);

-- screen_time_ledger (per-user daily balance for "earn time") ----------------
CREATE TABLE screen_time_ledger (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    device_user_id uuid NOT NULL REFERENCES device_users(id) ON DELETE CASCADE,
    day            date NOT NULL,
    earned_seconds int  NOT NULL DEFAULT 0,
    used_seconds   int  NOT NULL DEFAULT 0,
    streak_days    int  NOT NULL DEFAULT 0,
    UNIQUE (device_user_id, day)
);
