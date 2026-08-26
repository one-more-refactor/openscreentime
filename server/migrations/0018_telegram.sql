-- One Telegram bot per deployment (OST_TELEGRAM_BOT_TOKEN); parents pair
-- their personal chat with their account. A paired chat gets alerts, can
-- answer a time request inline ("ok to a chore"), and can approve a
-- confirm-it's-you check with one tap.

CREATE TABLE telegram_chats (
    chat_id     bigint PRIMARY KEY,
    admin_id    uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    tenant_id   uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    username    text,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX telegram_chats_tenant ON telegram_chats(tenant_id);
CREATE INDEX telegram_chats_admin ON telegram_chats(admin_id);

-- Short-lived pairing codes, shown once in the console's Security room and
-- typed to the bot as /start <code>. Stored hashed, like every other token.
CREATE TABLE telegram_pair_codes (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_id    uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    tenant_id   uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    code_hash   text NOT NULL UNIQUE,
    expires_at  timestamptz NOT NULL,
    consumed_at timestamptz
);

-- A pending "confirm it's you" tap: bound to the session that asked, so the
-- tap opens THAT session's confirm window and nobody else's.
CREATE TABLE telegram_verifications (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_id    uuid NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
    session_id  uuid NOT NULL REFERENCES admin_sessions(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    decided_at  timestamptz,
    approved    boolean
);
