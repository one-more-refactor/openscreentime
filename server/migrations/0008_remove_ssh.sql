-- The remote shell is gone (v0.4): everything is UI-only. The ssh_sessions
-- table and the ssh_open/ssh_close command types go away; historical 'ssh'
-- EVENTS stay readable — the transparency record that shells happened
-- survives, only the capability is removed.

DROP TABLE IF EXISTS ssh_sessions;

DELETE FROM commands WHERE type IN ('ssh_open', 'ssh_close');

ALTER TABLE commands DROP CONSTRAINT commands_type_check;
ALTER TABLE commands ADD CONSTRAINT commands_type_check CHECK (type IN
    ('lock','unlock','apply_policy','discover',
     'set_tamper_level','credit_time','deny_earn'));
