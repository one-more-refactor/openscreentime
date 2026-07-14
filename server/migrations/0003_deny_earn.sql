-- deny_earn: mirror of credit_time for the denial path, so the agent can clear
-- its once-per-day earn-request dedupe and surface the denial to the user
-- instead of a stale "WAITING FOR APPROVAL".
ALTER TABLE commands DROP CONSTRAINT commands_type_check;
ALTER TABLE commands ADD CONSTRAINT commands_type_check CHECK (type IN
    ('lock','unlock','apply_policy','ssh_open','ssh_close','discover',
     'set_tamper_level','credit_time','deny_earn'));
