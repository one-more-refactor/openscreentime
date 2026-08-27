-- Where the time goes (CONTRACT-0.6 §3): per-hour attribution slices the
-- agent reports. kind 'app' carries seconds of the app being open for a
-- specific OS user; kind 'site' carries DNS-query activity for the whole
-- device (os_username '' — resolver traffic has no user). Upsert-summed.
CREATE TABLE usage_slices (
    device_id   uuid NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    tenant_id   uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    os_username text NOT NULL DEFAULT '',
    hour        timestamptz NOT NULL,
    kind        text NOT NULL,
    key         text NOT NULL,
    amount      bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (device_id, os_username, hour, kind, key)
);
CREATE INDEX usage_slices_tenant_hour ON usage_slices(tenant_id, hour);
