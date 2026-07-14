-- evasion: server-side anti-cheat telemetry. The server cross-checks each
-- heartbeat against what it already knows and records an `evasion` event when
-- the story doesn't add up — today, a reported per-user usage total that has
-- regressed below the recorded total (a wiped/rolled-back client ledger). The
-- monotonic GREATEST clamp already neutralizes the cheat; this makes it visible
-- instead of silently swallowed.
ALTER TABLE events DROP CONSTRAINT events_type_check;
ALTER TABLE events ADD CONSTRAINT events_type_check CHECK (type IN
    ('heartbeat','tamper','lock','unlock','policy_applied',
     'screen_time_exceeded','screen_time_earned','streak',
     'enrolled','discovery_result','ssh','earn_request','evasion'));
