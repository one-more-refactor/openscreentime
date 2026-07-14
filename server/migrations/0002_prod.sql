-- Prod push: DB-backed admin sessions, earn-time approval flow, and new
-- command/event types (docs/CONTRACT-PROD.md).

-- admin_sessions (cookie value is sha256-hashed at rest, like device tokens) --
CREATE TABLE admin_sessions (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash  text NOT NULL UNIQUE,          -- sha256 hex of the cookie value
    admin_id    uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    tenant_id   uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL
);
CREATE INDEX idx_admin_sessions_expires ON admin_sessions(expires_at);

-- earn_requests (kid asks for extra screen time; parent approves/denies) -----
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

-- commands: new `credit_time` type (approved earn request -> agent credit) ---
ALTER TABLE commands DROP CONSTRAINT commands_type_check;
ALTER TABLE commands ADD CONSTRAINT commands_type_check CHECK (type IN
    ('lock','unlock','apply_policy','ssh_open','ssh_close','discover',
     'set_tamper_level','credit_time'));

-- events: new `ssh` (remote-shell audit) and `earn_request` (audit) types ----
ALTER TABLE events DROP CONSTRAINT events_type_check;
ALTER TABLE events ADD CONSTRAINT events_type_check CHECK (type IN
    ('heartbeat','tamper','lock','unlock','policy_applied',
     'screen_time_exceeded','screen_time_earned','streak',
     'enrolled','discovery_result','ssh','earn_request'));
