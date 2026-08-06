-- Two features removed, and the schema stops pretending they exist.
--
-- LAN discovery: an admin could ask an agent to sweep its subnet and the hosts
-- came back as `discovery_result` events. Nothing in the product needs a
-- network scanner — it was inherited from the device-management framing, not
-- the screen-time one.
--
-- Streaks: `streak` events backed bedtime/break nudges of the "KEEP YOUR
-- STREAK 🔥" variety. The product brief is explicit that this app stays silent
-- unless a human must act, so the nudges are gone and the event type with
-- them. Wind-down warnings ("2 min left") survive as a local notification and
-- deliberately emit no event.
--
-- Historical rows go too: unlike the ssh events in 0008 (a transparency record
-- worth keeping), these are a scan dump and an engagement log, neither of which
-- a parent has any reason to read back.

DELETE FROM commands WHERE type = 'discover';
DELETE FROM events WHERE type IN ('discovery_result', 'streak');

ALTER TABLE commands DROP CONSTRAINT commands_type_check;
ALTER TABLE commands ADD CONSTRAINT commands_type_check CHECK (type IN
    ('lock','unlock','apply_policy',
     'set_tamper_level','credit_time','deny_earn'));

ALTER TABLE events DROP CONSTRAINT events_type_check;
ALTER TABLE events ADD CONSTRAINT events_type_check CHECK (type IN
    ('heartbeat','tamper','lock','unlock','policy_applied',
     'screen_time_exceeded','screen_time_earned',
     'enrolled','ssh','earn_request','evasion'));

-- The one-pending-command-per-type guard from 0009. Its predicate has to stay
-- character-identical to the `ON CONFLICT ... WHERE` clause in agent.rs, or
-- Postgres cannot match the arbiter index and every enqueue errors out — so
-- dropping 'discover' from the code means recreating the index here.
DROP INDEX IF EXISTS idx_commands_one_pending;
CREATE UNIQUE INDEX idx_commands_one_pending
    ON commands (device_id, type)
 WHERE status IN ('queued','sent')
   AND type IN ('lock','unlock','apply_policy','set_tamper_level');
