-- One computer, one record — and a person's day that outlives the record
-- (server/src/machine.rs).
--
-- Re-running the install one-liner on a computer the household already has
-- (a new token: "Add my computer" on the machine that was "Mia's computer")
-- left two records for one machine. The old one kept today's minutes, the
-- agent counted the same minutes again under the new one from its own
-- ledger, and the person's day doubled (Mia 32 -> 64 min). The agent now says
-- which machine it is at enrollment — an HMAC of /etc/machine-id keyed with
-- this household's salt, never the id itself — and enrolling a machine the
-- household already has folds the older record into the new one.
ALTER TABLE tenants ADD COLUMN machine_salt text NOT NULL
    DEFAULT replace(gen_random_uuid()::text, '-', '');
ALTER TABLE devices ADD COLUMN machine_hash text;
CREATE INDEX idx_devices_machine ON devices (tenant_id, machine_hash)
    WHERE machine_hash IS NOT NULL;

-- A folded record's token is kept as a tombstone (0034) that points at the
-- record it went into: the agent still running there with its old token
-- (until the installer restarts it) is heard as the new record — never told
-- to take itself off the computer it was just enrolled on again. When that
-- record is removed, the pointer clears and it is a plain tombstone.
ALTER TABLE retired_devices ADD COLUMN merged_into uuid
    REFERENCES devices(id) ON DELETE SET NULL;

-- Removing a computer wiped the day of everyone who used it: the ledger hung
-- off the computer's logins and went with them (Mia: "17 min left of 17"
-- after using 37). A removed computer's usage is kept here, under the
-- person, for as long as the history looks back; their day, their week and
-- the time their other computers count stay true.
CREATE TABLE retired_usage (
    tenant_id       uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    account_id      uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    -- The removed record (gone, so no foreign key) and what it was called.
    device_id       uuid NOT NULL,
    device_name     text NOT NULL,
    -- Its machine, so enrolling the same machine again takes these back
    -- instead of counting them twice (the agent's own ledger has them).
    machine_hash    text,
    os_username     text NOT NULL,
    -- The computer's local day, as screen_time_ledger filed it.
    day             date NOT NULL,
    used_seconds    int  NOT NULL DEFAULT 0,
    earned_seconds  int  NOT NULL DEFAULT 0,
    utc_offset_secs int,
    retired_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (device_id, os_username, day)
);
CREATE INDEX idx_retired_usage_account ON retired_usage (account_id, day);
CREATE INDEX idx_retired_usage_machine ON retired_usage (tenant_id, machine_hash)
    WHERE machine_hash IS NOT NULL;
