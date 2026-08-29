-- Account login identity moves from email to a username.
--
-- Passkey registration/login and the client-code ("number match") login now key
-- off `admins.username` instead of `email`. Email is no longer required or shown
-- in the console; the column stays (nullable) only so OIDC SSO and existing rows
-- aren't broken. Kids / auto-provisioned members have no username (they don't log
-- in) — the uniqueness is enforced only where a username is actually set.

ALTER TABLE admins ADD COLUMN username text;

-- Backfill login-capable accounts so an existing instance isn't stranded by the
-- switch. Prefer the email local-part, else a slug of the display name, else
-- "user"; disambiguate collisions (email was globally unique, but two different
-- domains can share a local-part) with a numeric suffix.
WITH cand AS (
    SELECT id,
           coalesce(
               nullif(lower(regexp_replace(
                   coalesce(nullif(split_part(coalesce(email,''),'@',1),''), display_name, ''),
                   '[^a-z0-9._-]+', '', 'g')), ''),
               'user') AS base
      FROM admins
     WHERE role <> 'member' OR email IS NOT NULL
),
numbered AS (
    SELECT id, base, row_number() OVER (PARTITION BY base ORDER BY id) AS rn
      FROM cand
)
UPDATE admins a
   SET username = n.base || CASE WHEN n.rn = 1 THEN '' ELSE '-' || n.rn::text END
  FROM numbered n
 WHERE a.id = n.id;

-- Case-insensitive, global uniqueness — but only for rows that carry a username.
CREATE UNIQUE INDEX idx_admins_username_lower ON admins (lower(username)) WHERE username IS NOT NULL;
