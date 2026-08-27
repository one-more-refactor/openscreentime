-- Client-first login (CONTRACT-0.6 §2): the browser asks by username, the
-- person's own computer approves. PKCE-style: the browser keeps a secret
-- verifier and sends only its SHA-256; approval is worthless to any other
-- browser.
CREATE TABLE login_requests (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    account_id         uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    code_challenge     text NOT NULL,
    status             text NOT NULL DEFAULT 'pending', -- pending|approved|denied
    approved_device_id uuid REFERENCES devices(id) ON DELETE SET NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    expires_at         timestamptz NOT NULL
);
CREATE INDEX login_requests_expiry ON login_requests(expires_at);
