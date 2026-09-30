-- 0.6.2 "appliance": what the server needs to run unattended.

-- Idempotent event ingest. The agent retries a batch it could not deliver;
-- with a per-event id minted on the device, a retry of a batch that DID land
-- (the response was lost) inserts nothing twice — so a critical event is never
-- re-alerted to the parent's phone. Scoped per device: one device can never
-- shadow another's events by guessing ids.
ALTER TABLE events ADD COLUMN client_id uuid;
CREATE UNIQUE INDEX idx_events_client_id ON events (device_id, client_id)
    WHERE client_id IS NOT NULL;

-- An event type this server does not know (a newer agent) is stored as
-- `other` with the original type in the payload, instead of failing the whole
-- batch on the CHECK and stalling the device's audit trail behind it.
ALTER TABLE events DROP CONSTRAINT events_type_check;
ALTER TABLE events ADD CONSTRAINT events_type_check CHECK (type IN
    ('heartbeat','tamper','lock','unlock','policy_applied',
     'screen_time_exceeded','screen_time_earned',
     'enrolled','ssh','earn_request','evasion',
     'enforcement_degraded','vpn_profile',
     'parent_code_ok','parent_code_failed','parent_code_backup_used',
     'app_blocked','member',
     -- written by the device-login flow, missing from the list until now
     -- (those inserts failed the CHECK silently):
     'account_login','login_approval',
     'other'));

-- Enrollment that survives a lost response. The enroll token used to be burnt
-- before the agent had its credentials; if the reply never arrived, the
-- one-liner failed and the token was gone. Now a retry with the same token,
-- from the same host, shortly after, and before the device ever used its
-- credentials, re-issues them instead of refusing.
ALTER TABLE devices ADD COLUMN enroll_token_used text;
ALTER TABLE devices ADD COLUMN enrolled_at timestamptz;

-- What the host-side scripts (deploy/backup.sh, deploy/update.sh) did, so the
-- server can tell the operator when something went wrong — and notice when a
-- nightly backup has quietly stopped happening. Written by the scripts through
-- the db container; read by alerts.rs.
CREATE TABLE ops_log (
    id         bigserial PRIMARY KEY,
    kind       text NOT NULL CHECK (kind IN ('install','backup','update')),
    ok         boolean NOT NULL,
    detail     text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_ops_log_kind_time ON ops_log (kind, created_at DESC);
-- The "no backup for two days" check counts from here on an install that has
-- never recorded one.
INSERT INTO ops_log (kind, ok, detail) VALUES ('install', true, 'ops log created');

-- Operator-level problems currently open. One row per incident, so each one
-- is announced once — not once per check, and not again after a restart.
CREATE TABLE ops_incidents (
    key       text PRIMARY KEY,
    tenant_id uuid REFERENCES tenants(id) ON DELETE CASCADE,
    message   text NOT NULL,
    opened_at timestamptz NOT NULL DEFAULT now()
);
