-- A goal the PERSON sets for themselves (minutes/day), distinct from the
-- parent's daily_limit cap. This is what the ring, week strip, and streak
-- point at — the shift from externally-imposed limits to internalized
-- self-regulation (the behavioral-science load-bearing change). NULL = no
-- goal set yet; the parent cap remains the only line until they choose one.
ALTER TABLE admins ADD COLUMN goal_minutes int;
