-- Every device gets its own 8-digit recovery PIN, generated at enrollment.
--
-- Why this exists: `sentinel-agent unlock --pin` is the only offline way back
-- into a device that has locked itself out — no server, no network, no SSH
-- needed. It reads `policy.parent_pin_hash`, and that was only ever set if an
-- admin remembered to type a PIN into the profile editor. Nothing enforced it.
--
-- A device enrolled without one is unrecoverable by design: the lockout overlay
-- tells the user "ASK A PARENT (PIN UNLOCKS)" while every PIN path refuses,
-- `unlock` bails with "no parent PIN is configured", and the server-side unlock
-- command needs exactly the connectivity whose absence caused the lockdown. The
-- only remaining route is a keyboard, physical access, and knowing to mask a
-- systemd unit from the GRUB command line.
--
-- Stored as an argon2 PHC hash. The plaintext is shown once, in the enrollment
-- response, and never persisted.
ALTER TABLE devices ADD COLUMN recovery_pin_hash text;

-- When it was generated, so the console can say "PIN set at …" and a future
-- rotate endpoint has something to show.
ALTER TABLE devices ADD COLUMN recovery_pin_set_at timestamptz;
