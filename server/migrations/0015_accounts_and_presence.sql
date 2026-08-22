-- 0.4.0: accounts with roles + age brackets, per-device parent code (TOTP),
-- honest presence (locked is its own column), bracket presets, and the event
-- types the 0.4 agent emits. See docs/CONTRACT-0.4.md §2.

-- accounts = the admins table, grown up -----------------------------------
ALTER TABLE admins ALTER COLUMN email DROP NOT NULL;          -- kids have none
ALTER TABLE admins ADD COLUMN role text NOT NULL DEFAULT 'owner'
    CHECK (role IN ('owner','parent','member'));
ALTER TABLE admins ADD COLUMN age_bracket text NOT NULL DEFAULT 'adult'
    CHECK (age_bracket IN ('little','kid','younger_teen','older_teen','adult'));
ALTER TABLE admins ADD COLUMN birthdate date;
ALTER TABLE admins ADD COLUMN theme text
    CHECK (theme IS NULL OR theme IN ('playful','calm','plain'));
ALTER TABLE admins ADD COLUMN self_managed bool NOT NULL DEFAULT false;
ALTER TABLE admins ADD COLUMN profile_id uuid REFERENCES profiles(id) ON DELETE SET NULL;
CREATE INDEX idx_admins_tenant_role ON admins (tenant_id, role);

-- an OS login on a device belongs to a person ------------------------------
ALTER TABLE device_users ADD COLUMN account_id uuid REFERENCES admins(id) ON DELETE SET NULL;
CREATE INDEX idx_device_users_account ON device_users (account_id);

-- devices: parent authenticator, enroll intent, honest presence -------------
ALTER TABLE devices ADD COLUMN parent_totp_secret text;
-- "this computer is <member>'s": every OS user that enrolls without a
-- name-match links to this account instead of spawning a new member.
ALTER TABLE devices ADD COLUMN owner_account_id uuid REFERENCES admins(id) ON DELETE SET NULL;
ALTER TABLE devices ADD COLUMN locked bool NOT NULL DEFAULT false;
ALTER TABLE devices ADD COLUMN last_state jsonb;
UPDATE devices SET locked = true, status = 'offline' WHERE status = 'locked';
ALTER TABLE devices DROP CONSTRAINT devices_status_check;
ALTER TABLE devices ADD CONSTRAINT devices_status_check
    CHECK (status IN ('pending','online','offline'));

-- a voucher is bound to the person whose OS login asked for it --------------
ALTER TABLE device_vouchers ADD COLUMN account_id uuid REFERENCES admins(id) ON DELETE CASCADE;

-- bracket presets -------------------------------------------------------------
ALTER TABLE profiles DROP CONSTRAINT profiles_kind_check;
ALTER TABLE profiles ADD CONSTRAINT profiles_kind_check CHECK (kind IN
    ('little','kid','younger_teen','older_teen','adult','custom','kids','teen','default'));

-- event types the 0.4 agent emits. 0013 accidentally dropped
-- enforcement_degraded and vpn_profile (added in 0007); they come back here.
ALTER TABLE events DROP CONSTRAINT events_type_check;
ALTER TABLE events ADD CONSTRAINT events_type_check CHECK (type IN
    ('heartbeat','tamper','lock','unlock','policy_applied',
     'screen_time_exceeded','screen_time_earned',
     'enrolled','ssh','earn_request','evasion',
     'enforcement_degraded','vpn_profile',
     'parent_code_ok','parent_code_failed','parent_code_backup_used',
     'app_blocked','member'));
