-- Real command queue (v0.4): dedup, cancellation, and redelivery tracking.
--
-- * `sent_at` — when the command was last handed to an agent. A `sent` row
--   whose ack never arrives is redelivered only after a grace window, instead
--   of on every heartbeat (which re-executed unacked commands forever).
-- * `cancelled` — an admin withdrew a queued/sent command before the ack.
-- * At most ONE pending (queued|sent) command per (device, type) for the
--   idempotent types; a second enqueue coalesces into the pending row.
--   credit_time/deny_earn stay unrestricted — each grant is distinct.

ALTER TABLE commands ADD COLUMN sent_at timestamptz;

-- Backfill: anything already `sent` gets its creation time as the best guess,
-- so the redelivery grace window starts sane instead of NULL-forever.
UPDATE commands SET sent_at = created_at WHERE status = 'sent';

ALTER TABLE commands DROP CONSTRAINT IF EXISTS commands_status_check;
ALTER TABLE commands ADD CONSTRAINT commands_status_check
    CHECK (status IN ('queued','sent','acked','failed','cancelled'));

-- Collapse pre-existing duplicate pending rows (keep the oldest) so the
-- unique guard below can be created.
DELETE FROM commands a USING commands b
 WHERE a.device_id = b.device_id
   AND a.type = b.type
   AND a.status IN ('queued','sent')
   AND b.status IN ('queued','sent')
   AND a.type IN ('lock','unlock','apply_policy','discover','set_tamper_level')
   AND (a.created_at > b.created_at
        OR (a.created_at = b.created_at AND a.id > b.id));

CREATE UNIQUE INDEX idx_commands_one_pending
    ON commands (device_id, type)
 WHERE status IN ('queued','sent')
   AND type IN ('lock','unlock','apply_policy','discover','set_tamper_level');
