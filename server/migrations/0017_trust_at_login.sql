-- Trust is decided at login. A session born from a passkey ceremony, an
-- enrolled device's voucher, or SSO starts trusted and mutates freely — the
-- separate 15-minute change-mode ceremony is gone. stepup_until survives with
-- a narrower job: the short confirm window over the sensitive corner (unlock
-- codes, recovery codes, passkeys, pairing tokens).
ALTER TABLE admin_sessions ADD COLUMN trusted boolean NOT NULL DEFAULT false;

-- Every session alive at migration time was born from a passkey, SSO, or a
-- device voucher — all of which now confer trust at birth.
UPDATE admin_sessions SET trusted = true;
