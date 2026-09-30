-- Removing a computer frees it (docs/API.md, "Removing a computer").
--
-- The console promises "no more limits there". Deleting the row alone broke
-- that promise: the agent's token stopped matching anything, every call got a
-- plain 401 — the same answer a network hiccup or a bad proxy can give — and
-- the agent kept enforcing its cached rules forever (a locked child stayed
-- locked). A removed computer's token is kept here, hashed, so the server can
-- answer it with a distinct `410 device_retired`: the one signal on which the
-- agent takes itself off the computer.
CREATE TABLE retired_devices (
    -- sha256 hex of the device token, as devices.device_token held it.
    token_hash text PRIMARY KEY,
    device_id  uuid NOT NULL,
    tenant_id  uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    retired_at timestamptz NOT NULL DEFAULT now()
);
