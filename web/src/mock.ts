// ============================================================================
// Sample data so the UI renders standalone (no backend). Mirrors the preset
// policies in docs/PROFILES.md verbatim. The API client falls back to this
// whenever a request fails (backend down) so the control center is reviewable
// as a pure front-end artifact.
// ============================================================================

import type {
  Admin,
  Device,
  DeviceDetail,
  DeviceUser,
  DiscoveryResult,
  Event,
  Me,
  Passkey,
  Policy,
  Profile,
  Tenant,
} from "./types";

const TENANT_ID = "11111111-1111-1111-1111-111111111111";

export const kidsPolicy: Policy = {
  version: 1,
  dns: {
    mode: "default_deny",
    allowlist: [
      "wikipedia.org",
      "khanacademy.org",
      "pbskids.org",
      "scratch.mit.edu",
      "duolingo.com",
    ],
    blocklist: [],
    safe_search: true,
    upstream: "1.1.1.2",
  },
  firewall: {
    mode: "default_deny",
    allow_outbound_ports: [53, 80, 443],
    allow_inbound_ports: [],
  },
  screen_time: {
    enabled: true,
    daily_limit_minutes: 60,
    schedule: [
      { days: [1, 2, 3, 4, 5], start: "15:00", end: "19:00" },
      { days: [0, 6], start: "09:00", end: "19:00" },
    ],
    bedtime: { start: "20:00", end: "07:00" },
  },
  app_limits: [{ match: "steam", daily_limit_minutes: 30 }],
  gamification: {
    earn_time: {
      enabled: true,
      tasks: [
        { id: "reading", label: "Read for 20 min", reward_minutes: 15 },
        { id: "chores", label: "Finish chores", reward_minutes: 15 },
      ],
    },
    lockout: { enabled: true, unlock_challenge: "math" },
    streaks: { enabled: true, nudges: ["bedtime", "breaks"] },
  },
};

export const teenPolicy: Policy = {
  version: 1,
  dns: {
    mode: "default_deny",
    allowlist: [
      "*.wikipedia.org",
      "github.com",
      "google.com",
      "youtube.com",
      "duolingo.com",
      "*.edu",
    ],
    blocklist: [],
    safe_search: true,
    upstream: "1.1.1.2",
  },
  firewall: {
    mode: "default_deny",
    allow_outbound_ports: [53, 80, 443, 123],
    allow_inbound_ports: [],
  },
  screen_time: {
    enabled: true,
    daily_limit_minutes: 180,
    schedule: [
      { days: [1, 2, 3, 4, 5], start: "07:00", end: "21:00" },
      { days: [0, 6], start: "08:00", end: "22:00" },
    ],
    bedtime: { start: "22:30", end: "06:30" },
  },
  app_limits: [{ match: "steam", daily_limit_minutes: 90 }],
  gamification: {
    earn_time: {
      enabled: true,
      tasks: [{ id: "homework", label: "Finish homework", reward_minutes: 20 }],
    },
    lockout: { enabled: true, unlock_challenge: "wait" },
    streaks: { enabled: true, nudges: ["breaks"] },
  },
};

export const defaultPolicy: Policy = {
  version: 1,
  dns: {
    mode: "default_deny",
    allowlist: ["*"],
    blocklist: [],
    safe_search: true,
    upstream: "1.1.1.2",
  },
  firewall: {
    mode: "default_deny",
    allow_outbound_ports: [53, 80, 443, 123],
    allow_inbound_ports: [],
  },
  screen_time: {
    enabled: false,
    daily_limit_minutes: 0,
    schedule: [],
    bedtime: null,
  },
  app_limits: [],
  gamification: {
    earn_time: { enabled: false, tasks: [] },
    lockout: { enabled: false, unlock_challenge: "wait" },
    streaks: { enabled: false, nudges: [] },
  },
};

export const mockProfiles: Profile[] = [
  {
    id: "p-kids",
    tenant_id: TENANT_ID,
    name: "Kids",
    kind: "kids",
    is_preset: true,
    policy: kidsPolicy,
    created_at: "2026-06-01T10:00:00Z",
    updated_at: "2026-06-01T10:00:00Z",
  },
  {
    id: "p-teen",
    tenant_id: TENANT_ID,
    name: "Teen",
    kind: "teen",
    is_preset: true,
    policy: teenPolicy,
    created_at: "2026-06-01T10:00:00Z",
    updated_at: "2026-06-01T10:00:00Z",
  },
  {
    id: "p-default",
    tenant_id: TENANT_ID,
    name: "Default",
    kind: "default",
    is_preset: true,
    policy: defaultPolicy,
    created_at: "2026-06-01T10:00:00Z",
    updated_at: "2026-06-01T10:00:00Z",
  },
  {
    id: "p-gaming",
    tenant_id: TENANT_ID,
    name: "Weekend Gaming",
    kind: "custom",
    is_preset: false,
    policy: {
      ...teenPolicy,
      screen_time: { ...teenPolicy.screen_time, daily_limit_minutes: 240 },
    },
    created_at: "2026-06-20T12:00:00Z",
    updated_at: "2026-06-28T09:12:00Z",
  },
];

function du(
  id: string,
  device_id: string,
  os_username: string,
  display_name: string | null,
  profile_id: string,
): DeviceUser {
  return {
    id,
    device_id,
    os_username,
    display_name,
    profile_id,
    created_at: "2026-06-10T08:00:00Z",
  };
}

export const mockDevices: Device[] = [
  {
    id: "d-livingroom",
    tenant_id: TENANT_ID,
    name: "Living Room PC",
    hostname: "livingroom-01",
    os: "linux",
    agent_version: "0.3.1",
    status: "online",
    tamper_level: 1,
    public_ip: "84.112.22.9",
    last_seen: "2026-07-07T14:58:12Z",
    created_at: "2026-06-10T08:00:00Z",
    users: [
      du("u-mia", "d-livingroom", "mia", "Mia", "p-kids"),
      du("u-leo", "d-livingroom", "leo", "Leo", "p-teen"),
    ],
  },
  {
    id: "d-studio",
    tenant_id: TENANT_ID,
    name: "Studio Laptop",
    hostname: "studio-lt",
    os: "linux",
    agent_version: "0.3.1",
    status: "locked",
    tamper_level: 3,
    public_ip: "84.112.22.9",
    last_seen: "2026-07-07T14:40:03Z",
    created_at: "2026-06-12T18:20:00Z",
    users: [du("u-noah", "d-studio", "noah", "Noah", "p-teen")],
  },
  {
    id: "d-loft",
    tenant_id: TENANT_ID,
    name: "Loft Desktop",
    hostname: "loft-desk",
    os: "linux",
    agent_version: "0.2.9",
    status: "offline",
    tamper_level: 1,
    public_ip: null,
    last_seen: "2026-07-06T22:10:44Z",
    created_at: "2026-06-14T09:00:00Z",
    users: [du("u-ada", "d-loft", "ada", "Ada", "p-default")],
  },
  {
    id: "d-new",
    tenant_id: TENANT_ID,
    name: "Kitchen Tablet Host",
    hostname: "—",
    os: "linux",
    agent_version: "—",
    status: "pending",
    tamper_level: 1,
    public_ip: null,
    last_seen: null,
    created_at: "2026-07-07T13:00:00Z",
    users: [],
  },
];

export const mockEvents: Event[] = [
  {
    id: "e1",
    tenant_id: TENANT_ID,
    device_id: "d-studio",
    device_user_id: "u-noah",
    type: "tamper",
    severity: "critical",
    payload: { kind: "nft_flush_attempt", detail: "nftables ruleset flushed; re-applied" },
    created_at: "2026-07-07T14:41:00Z",
  },
  {
    id: "e2",
    tenant_id: TENANT_ID,
    device_id: "d-studio",
    device_user_id: null,
    type: "lock",
    severity: "warn",
    payload: { reason: "admin_initiated" },
    created_at: "2026-07-07T14:40:03Z",
  },
  {
    id: "e3",
    tenant_id: TENANT_ID,
    device_id: "d-livingroom",
    device_user_id: "u-mia",
    type: "screen_time_earned",
    severity: "info",
    payload: { task: "reading", reward_minutes: 15 },
    created_at: "2026-07-07T14:12:22Z",
  },
  {
    id: "e4",
    tenant_id: TENANT_ID,
    device_id: "d-livingroom",
    device_user_id: "u-mia",
    type: "screen_time_exceeded",
    severity: "warn",
    payload: { balance_minutes: 0 },
    created_at: "2026-07-07T13:58:00Z",
  },
  {
    id: "e5",
    tenant_id: TENANT_ID,
    device_id: "d-livingroom",
    device_user_id: "u-leo",
    type: "policy_applied",
    severity: "info",
    payload: { policy_version: "42", profile: "teen" },
    created_at: "2026-07-07T13:30:10Z",
  },
  {
    id: "e6",
    tenant_id: TENANT_ID,
    device_id: "d-livingroom",
    device_user_id: "u-leo",
    type: "streak",
    severity: "info",
    payload: { streak_days: 6, nudge: "breaks" },
    created_at: "2026-07-07T12:00:00Z",
  },
  {
    id: "e7",
    tenant_id: TENANT_ID,
    device_id: "d-loft",
    device_user_id: null,
    type: "enrolled",
    severity: "info",
    payload: { hostname: "loft-desk" },
    created_at: "2026-06-14T09:02:00Z",
  },
  {
    id: "e8",
    tenant_id: TENANT_ID,
    device_id: "d-livingroom",
    device_user_id: null,
    type: "discovery_result",
    severity: "info",
    payload: { hosts_found: 4 },
    created_at: "2026-07-07T11:20:00Z",
  },
];

export const mockDiscovery: DiscoveryResult = {
  id: "disc-1",
  device_id: "d-livingroom",
  created_at: "2026-07-07T11:20:00Z",
  hosts: [
    { ip: "192.168.1.14", mac: "b8:27:eb:0a:11:22", hostname: "kitchen-tablet", open_ports: [22, 5555], vendor: "Raspberry Pi" },
    { ip: "192.168.1.22", mac: "44:65:0d:aa:bc:1e", hostname: "noah-switch", open_ports: [], vendor: "Nintendo" },
    { ip: "192.168.1.31", mac: "dc:a6:32:99:00:5f", hostname: undefined, open_ports: [80, 443], vendor: "Espressif" },
    { ip: "192.168.1.40", mac: "f0:9f:c2:1a:2b:3c", hostname: "old-laptop", open_ports: [22], vendor: "Ubiquiti" },
  ],
};

export const mockAdmin: Admin = {
  id: "a-1",
  tenant_id: TENANT_ID,
  email: "parent@home.lan",
  display_name: "Parent",
  created_at: "2026-06-01T10:00:00Z",
};

export const mockTenant: Tenant = {
  id: TENANT_ID,
  name: "Home",
  created_at: "2026-06-01T10:00:00Z",
};

export const mockMe: Me = { admin: mockAdmin, tenant: mockTenant };

export const mockPasskeys: Passkey[] = [
  { id: "k-1", nickname: "Pixel 8 fingerprint", created_at: "2026-06-01T10:05:00Z", last_used_at: "2026-07-07T09:00:00Z" },
  { id: "k-2", nickname: "YubiKey 5C", created_at: "2026-06-02T18:00:00Z", last_used_at: null },
];

export function mockDeviceDetail(id: string): DeviceDetail {
  const dev = mockDevices.find((d) => d.id === id) ?? mockDevices[0];
  return {
    ...dev,
    users: dev.users ?? [],
    recent_events: mockEvents.filter((e) => e.device_id === dev.id).slice(0, 8),
  };
}
