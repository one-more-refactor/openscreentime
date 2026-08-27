-- A stable face for every person (CONTRACT-0.6 §3): a parent-picked emoji.
-- NULL = the deterministic monogram the console draws anyway.
ALTER TABLE admins ADD COLUMN avatar text;
