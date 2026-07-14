-- Enroll tokens get a 24-hour TTL: a `pending` device whose token leaked (chat
-- log, screenshot) can no longer be enrolled days later. NULL means "no
-- expiry" for rows created before this migration (additive, no backfill).
ALTER TABLE devices ADD COLUMN enroll_token_expires_at timestamptz;
