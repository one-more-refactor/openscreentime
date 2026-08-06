-- Two things the console already believed were here.
--
-- 1. `offline_allowed_until` — the "this laptop is at grandma's for the
--    weekend, stop calling it trouble" window. The web has shipped the UI for
--    it since the family-page rebuild: Devices renders a countdown chip and the
--    home page's trouble banner filters on it. Nothing ever persisted it and no
--    endpoint ever served it, so outside mock mode the button 404'd and every
--    away device was reported as a problem. The column makes the feature real.
--
-- 2. An index for the family view. GET /api/family reads every device_user in
--    the tenant in one shot instead of one query per device; today's ledger row
--    is looked up by (device_user_id, day), which had no index.

ALTER TABLE devices ADD COLUMN IF NOT EXISTS offline_allowed_until timestamptz;

CREATE INDEX IF NOT EXISTS idx_ledger_user_day
    ON screen_time_ledger (device_user_id, day);

-- The family view also fetches every pending command for the tenant at once.
CREATE INDEX IF NOT EXISTS idx_commands_device_pending
    ON commands (device_id) WHERE status IN ('queued', 'sent');
