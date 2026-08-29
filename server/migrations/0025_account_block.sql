-- "Block account" — a parent Danger-Zone action on a child.
--
-- Blocking sets `blocked_at`; the account can no longer authenticate (passkey or
-- client-code login) and any live console sessions are cut. Blocking also locks
-- the child's devices so their screens stop immediately. Unblocking clears the
-- flag (devices are unlocked deliberately by the parent, not automatically).
ALTER TABLE admins ADD COLUMN blocked_at timestamptz;
