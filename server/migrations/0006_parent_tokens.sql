-- Parent access tokens: a scoped bearer credential a logged-in admin mints for
-- the parent-facing surfaces (the tray parent-mode companion and phone web-push
-- alerts). Deliberately NOT a session and NOT tied to a passkey — it's a
-- long-lived, revocable token pasted into a companion, in the same spirit as a
-- device_token. Its scope is fixed by the /api/parent/* routes: read pending
-- earn-requests + recent alerts, approve/deny requests. It cannot touch policy,
-- devices, SSH, or admin settings.
--
-- Stored hashed at rest (sha256), exactly like device_token and the admin
-- session cookie — the raw value is shown once at mint time and never again.
CREATE TABLE parent_access_tokens (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    token_hash   text NOT NULL UNIQUE,
    label        text NOT NULL DEFAULT '',
    created_by   uuid REFERENCES admins(id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz,
    revoked_at   timestamptz
);
CREATE INDEX idx_parent_tokens_tenant ON parent_access_tokens(tenant_id);
