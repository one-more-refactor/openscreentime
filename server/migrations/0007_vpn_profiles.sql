-- Per-device VPN profile: an admin-uploaded WireGuard / OpenVPN client config
-- the agent applies on the device (wg-quick@sentinel / openvpn-client@sentinel).
-- The config body holds private keys — it is returned ONLY on the authenticated
-- agent policy pull, never in admin device responses (they get kind + timestamp).
ALTER TABLE devices
    ADD COLUMN vpn_kind text CHECK (vpn_kind IN ('wireguard','openvpn')),
    ADD COLUMN vpn_config text,
    -- Set on every set/remove so the derived policy_version changes and
    -- poll-mode agents re-pull (removal must propagate too, so this stays
    -- non-null after a remove while vpn_kind/vpn_config go back to NULL).
    ADD COLUMN vpn_updated_at timestamptz;

-- New event types: vpn_profile (admin set/removed a profile) and
-- enforcement_degraded (agent accepted a policy it cannot fully enforce —
-- the client has emitted this since the DNS-gap fix, but it was missing from
-- this CHECK, so those criticals were rejected by the DB and retried forever).
ALTER TABLE events DROP CONSTRAINT events_type_check;
ALTER TABLE events ADD CONSTRAINT events_type_check CHECK (type IN
    ('heartbeat','tamper','lock','unlock','policy_applied',
     'screen_time_exceeded','screen_time_earned','streak',
     'enrolled','discovery_result','ssh','earn_request','evasion',
     'enforcement_degraded','vpn_profile'));
