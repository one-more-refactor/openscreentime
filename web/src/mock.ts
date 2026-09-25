// ============================================================================
// Sample data so the UI renders standalone (no backend). Mirrors the preset
// policies in docs/PROFILES.md verbatim. Served by the API client ONLY when
// the build runs with VITE_USE_MOCK=1 (design review).
// ============================================================================

import type {
  AgeBracket,
  Catalog,
  MemberPatch,
  MeToday,
  MyRules,
  NewMember,
  ConfirmStatus,
  RecoveryCodes,
  RecoveryCodesStatus,
  UnlockCode,
  UnlockCodeRotated,
  FamilyChild,
  FamilyResponse,
  Account,
  Admin,
  Device,
  DeviceUser,
  EarnRequest,
  EnrollTokenResponse,
  Event,
  Household,
  Me,
  Passkey,
  Policy,
  Profile,
  Tenant,
  WhereData,
} from "./types";
import { defaultThemeFor } from "./types";

const TENANT_ID = "11111111-1111-1111-1111-111111111111";

const kidsPolicy: Policy = {
  version: 1,
  dns: { mode: "allow_all", allowlist: ["*"], blocklist: [], safe_search: true, upstream: "1.1.1.3" },
  firewall: { mode: "allow_all", allow_outbound_ports: [], allow_inbound_ports: [] },
  screen_time: {
    enabled: true,
    daily_limit_minutes: 60,
    schedule: [
      { days: [1, 2, 3, 4, 5], start: "15:00", end: "19:00" },
      { days: [0, 6], start: "09:00", end: "19:00" },
    ],
    bedtime: { start: "20:00", end: "07:00" },
  },
  gamification: {
    earn_time: {
      enabled: true,
      tasks: [
        { id: "reading", label: "Read for 20 min", reward_minutes: 15 },
        { id: "chores", label: "Finish chores", reward_minutes: 15 },
      ],
    },
    lockout: { enabled: true, unlock_challenge: "math" },
  },
  lockdown: {
    force_dns: true,
    block_doh: true,
    block_dot: true,
    block_tor: true,
    block_vpn: true,
  },
  blocks: {
    apps: ["tiktok", "snapchat", "instagram", "discord", "twitch", "omegle"],
    categories: ["social", "adult", "gambling", "dating", "proxies"],
    custom_domains: [],
  },
};

const teenPolicy: Policy = {
  version: 1,
  dns: { mode: "allow_all", allowlist: ["*"], blocklist: ["9gag.com"], safe_search: true, upstream: "1.1.1.3" },
  firewall: { mode: "allow_all", allow_outbound_ports: [], allow_inbound_ports: [] },
  screen_time: {
    enabled: true,
    daily_limit_minutes: 180,
    schedule: [
      { days: [1, 2, 3, 4, 5], start: "07:00", end: "21:00" },
      { days: [0, 6], start: "08:00", end: "22:00" },
    ],
    bedtime: { start: "22:30", end: "06:30" },
  },
  gamification: {
    earn_time: {
      enabled: true,
      tasks: [{ id: "homework", label: "Finish homework", reward_minutes: 20 }],
    },
    lockout: { enabled: true, unlock_challenge: "wait" },
  },
  blocks: { apps: ["tiktok"], categories: ["adult", "gambling", "dating", "proxies"], custom_domains: [] },
};

const defaultPolicy: Policy = {
  version: 1,
  dns: { mode: "allow_all", allowlist: ["*"], blocklist: [], safe_search: true, upstream: "1.1.1.2" },
  firewall: { mode: "allow_all", allow_outbound_ports: [], allow_inbound_ports: [] },
  screen_time: {
    enabled: false,
    daily_limit_minutes: 0,
    schedule: [],
    bedtime: null,
  },
  gamification: {
    earn_time: { enabled: false, tasks: [] },
    lockout: { enabled: false, unlock_challenge: "wait" },
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
  // An adult who keeps their own time: their rules are theirs, so the family
  // view never lists this profile (the server leaves it out, and so does the
  // mock).
  {
    id: "p-jonas",
    tenant_id: TENANT_ID,
    name: "Jonas's rules",
    kind: "adult",
    is_preset: false,
    policy: {
      ...defaultPolicy,
      screen_time: { enabled: true, daily_limit_minutes: 180, schedule: [], bedtime: null },
      focus: {
        sites: ["reddit.com", "youtube.com", "news.ycombinator.com", "twitter.com"],
        hours: { days: [1, 2, 3, 4, 5], start: "09:00", end: "12:00" },
      },
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
  used_minutes_today = 0,
  earned_minutes_today = 0,
  account_id?: string,
): DeviceUser {
  return {
    id,
    device_id,
    os_username,
    display_name,
    profile_id,
    used_minutes_today,
    earned_minutes_today,
    created_at: "2026-06-10T08:00:00Z",
    account_id,
  };
}

/** Minutes-ago helper so mock devices always look freshly alive in a demo. */
function ago(minutes: number): string {
  return new Date(Date.now() - minutes * 60_000).toISOString();
}

export const mockDevices: Device[] = [
  {
    id: "d-livingroom",
    tenant_id: TENANT_ID,
    name: "Living Room PC",
    hostname: "livingroom-01",
    os: "linux",
    agent_version: "0.4.0",
    status: "online",
    locked: false,
    lock_pending: false,
    tamper_level: 1,
    public_ip: "84.112.22.9",
    last_seen: ago(2),
    created_at: "2026-06-10T08:00:00Z",
    recovery_codes_unused: 6,
    users: [
      du("u-mia", "d-livingroom", "mia", "Mia", "p-kids", 48, 15),
      du("u-leo", "d-livingroom", "leo", "Leo", "p-teen", 96, 0),
    ],
  },
  {
    id: "d-studio",
    tenant_id: TENANT_ID,
    name: "Studio Laptop",
    hostname: "studio-lt",
    os: "linux",
    agent_version: "0.4.0",
    status: "online",
    // Paused — and the agent has confirmed it (locked is the truth, not a wish).
    locked: true,
    lock_pending: false,
    last_state: { locked: true, frozen_users: ["noah"], enforcing: true, gaps: [], agent_version: "0.4.0", active_users: ["noah"] },
    tamper_level: 3,
    public_ip: "84.112.22.9",
    last_seen: ago(18),
    created_at: "2026-06-12T18:20:00Z",
    recovery_codes_unused: 8,
    users: [du("u-noah", "d-studio", "noah", "Noah", "p-teen", 120, 20)],
  },
  {
    id: "d-loft",
    tenant_id: TENANT_ID,
    name: "Loft Desktop",
    hostname: "loft-desk",
    os: "linux",
    agent_version: "0.3.1",
    status: "offline",
    locked: false,
    lock_pending: false,
    tamper_level: 1,
    public_ip: null,
    last_seen: ago(11 * 60),
    created_at: "2026-06-14T09:00:00Z",
    // Ada took the desktop to a school project day — offline is allowed.
    offline_allowed_until: ago(-3 * 60),
    // Nobody made recovery codes for this one yet — the card says so.
    recovery_codes_unused: 0,
    users: [du("u-ada", "d-loft", "ada", "Ada", "p-default")],
  },
  {
    id: "d-desk",
    tenant_id: TENANT_ID,
    name: "Desk laptop",
    hostname: "desk-lt",
    os: "linux",
    agent_version: "0.6.1",
    status: "online",
    locked: false,
    lock_pending: false,
    tamper_level: 1,
    public_ip: "84.112.22.9",
    last_seen: ago(0),
    created_at: "2026-06-20T12:00:00Z",
    recovery_codes_unused: 8,
    owner_account_id: "acc-jonas",
    users: [du("u-jonas", "d-desk", "jonas", "Jonas", "p-jonas", 72, 0, "acc-jonas")],
  },
  {
    // The parent's own computer — their login is theirs, never a child card.
    id: "d-parent",
    tenant_id: TENANT_ID,
    name: "Studio laptop",
    hostname: "studio",
    os: "linux",
    agent_version: "0.6.1",
    status: "online",
    locked: false,
    lock_pending: false,
    tamper_level: 1,
    public_ip: "84.112.22.9",
    last_seen: ago(0),
    created_at: "2026-06-01T10:30:00Z",
    recovery_codes_unused: 8,
    owner_account_id: "acc-parent",
    users: [du("u-parent", "d-parent", "philip", "Philip", "p-default", 72, 0, "acc-parent")],
  },
  {
    id: "d-new",
    tenant_id: TENANT_ID,
    name: "Kitchen Tablet Host",
    hostname: "—",
    os: "linux",
    agent_version: "—",
    status: "pending",
    locked: false,
    lock_pending: false,
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
    created_at: ago(95),
  },
  {
    id: "e4",
    tenant_id: TENANT_ID,
    device_id: "d-livingroom",
    device_user_id: "u-mia",
    type: "screen_time_exceeded",
    severity: "warn",
    payload: { balance_minutes: 0 },
    created_at: ago(26 * 60),
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
    type: "screen_time_earned",
    severity: "info",
    payload: { reward_minutes: 15, task: "Read for 20 min" },
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
];

const mockAdmin: Admin = {
  id: "a-1",
  tenant_id: TENANT_ID,
  username: "philip",
  display_name: "Philip",
  created_at: "2026-06-01T10:00:00Z",
};

const mockTenant: Tenant = {
  id: TENANT_ID,
  name: "Home",
  created_at: "2026-06-01T10:00:00Z",
};

const mockHousehold: Household = {
  id: TENANT_ID,
  name: "The Harper house",
  created_at: "2026-06-01T10:00:00Z",
};

/** The signed-in identity — the founding parent (household owner). */
const mockAccount: Account = {
  id: "acc-parent",
  household_id: TENANT_ID,
  display_name: "Philip",
  email: "parent@home.lan",
  role: "owner",
  age_bracket: "adult",
  birthdate: "1988-04-12",
  self_managed: true,
  theme: null,
  effective_theme: "plain",
  profile_id: "p-default",
  created_at: "2026-06-01T10:00:00Z",
};

/** The whole household — everyone has an account now, across every bracket. */
export const mockHouseholdAccounts: Account[] = [
  mockAccount,
  {
    id: "acc-coparent",
    household_id: TENANT_ID,
    display_name: "Sam",
    email: "sam@home.lan",
    role: "parent",
    age_bracket: "adult",
    birthdate: "1990-09-01",
    self_managed: true,
    theme: null,
    effective_theme: "plain",
    profile_id: null,
    created_at: "2026-06-01T10:05:00Z",
  },
  {
    id: "acc-leo",
    household_id: TENANT_ID,
    display_name: "Leo",
    avatar: "🎮",
    email: "leo@home.lan",
    role: "member",
    age_bracket: "older_teen",
    birthdate: "2009-02-20",
    self_managed: false,
    theme: null,
    effective_theme: "calm",
    profile_id: "p-teen",
    created_at: "2026-06-02T08:00:00Z",
  },
  {
    id: "acc-noah",
    household_id: TENANT_ID,
    display_name: "Noah",
    avatar: "🚀",
    email: "noah@home.lan",
    role: "member",
    age_bracket: "younger_teen",
    birthdate: "2012-06-11",
    self_managed: false,
    theme: null,
    effective_theme: "calm",
    profile_id: "p-teen",
    created_at: "2026-06-02T08:05:00Z",
  },
  {
    id: "acc-mia",
    household_id: TENANT_ID,
    display_name: "Mia",
    avatar: "🦄",
    email: null,
    role: "member",
    age_bracket: "kid",
    birthdate: "2016-11-03",
    self_managed: false,
    theme: null,
    effective_theme: "playful",
    profile_id: "p-kids",
    created_at: "2026-06-02T08:10:00Z",
  },
  {
    id: "acc-ada",
    household_id: TENANT_ID,
    display_name: "Ada",
    avatar: "🐼",
    email: null,
    role: "member",
    age_bracket: "little",
    birthdate: "2020-01-30",
    self_managed: false,
    theme: null,
    effective_theme: "playful",
    profile_id: "p-default",
    created_at: "2026-06-02T08:15:00Z",
  },
  {
    // An adult in the household keeping time for themselves (?as=jonas).
    id: "acc-jonas",
    household_id: TENANT_ID,
    display_name: "Jonas",
    email: null,
    role: "member",
    age_bracket: "adult",
    birthdate: "1996-03-14",
    self_managed: true,
    theme: null,
    effective_theme: "plain",
    profile_id: "p-jonas",
    created_at: "2026-06-20T12:00:00Z",
  },
];

export const mockMe: Me = {
  account: mockAccount,
  household: mockHousehold,
  admin: mockAdmin,
  tenant: mockTenant,
};

/** The code design-review mode accepts wherever a code is typed. */
export const MOCK_CODE = "123456";

export const mockEarnRequests: EarnRequest[] = [
  {
    id: "er-1",
    device_id: "d-livingroom",
    device_user_id: "u-mia",
    os_username: "mia",
    task_id: "reading",
    task_label: "Read for 20 min",
    minutes: 15,
    status: "pending",
    created_at: ago(4), // fresh in a demo
    decided_at: null,
    device_name: "Living Room PC",
    user_display_name: "Mia",
  },
  {
    id: "er-2",
    device_id: "d-studio",
    device_user_id: "u-noah",
    os_username: "noah",
    task_id: "homework",
    task_label: "Finish homework",
    minutes: 20,
    status: "pending",
    created_at: ago(21),
    decided_at: null,
    device_name: "Studio Laptop",
    user_display_name: "Noah",
  },
  {
    id: "er-3",
    device_id: "d-livingroom",
    device_user_id: "u-leo",
    os_username: "leo",
    task_id: "chores",
    task_label: "Finish chores",
    minutes: 15,
    status: "approved",
    created_at: "2026-07-06T16:20:00Z",
    decided_at: "2026-07-06T16:24:30Z",
    device_name: "Living Room PC",
    user_display_name: "Leo",
  },
  {
    id: "er-4",
    device_id: "d-livingroom",
    device_user_id: "u-mia",
    os_username: "mia",
    task_id: "reading",
    task_label: "Read for 20 min",
    minutes: 15,
    status: "denied",
    created_at: "2026-07-05T19:02:00Z",
    decided_at: "2026-07-05T19:10:12Z",
    device_name: "Living Room PC",
    user_display_name: "Mia",
  },
];

export const mockPasskeys: Passkey[] = [
  { id: "k-1", nickname: "Pixel 8 fingerprint", created_at: "2026-06-01T10:05:00Z", last_used_at: "2026-07-07T09:00:00Z" },
  { id: "k-2", nickname: "YubiKey 5C", created_at: "2026-06-02T18:00:00Z", last_used_at: null },
];

/** Mock for POST /api/device-users/:id/credit-time: bump today's earned
 * minutes in-place so the UI reflects the grant on the next read. */
export function mockCreditTime(deviceUserId: string, minutes: number): void {
  for (const dev of mockDevices) {
    const user = dev.users?.find((u) => u.id === deviceUserId);
    if (user) {
      user.earned_minutes_today = (user.earned_minutes_today ?? 0) + minutes;
      return;
    }
  }
}

/** Mock for POST /api/devices — creates a pending device + one-time token +
 * nothing else — the unlock code is read from the device later, on demand. */
export function mockCreateDevice(name: string, member_id?: string): EnrollTokenResponse {
  const id = `mock-dev-${mockDevices.length + 1}`;
  const dev: Device = {
    id,
    tenant_id: TENANT_ID,
    name,
    hostname: "",
    os: "",
    agent_version: "",
    status: "pending",
    locked: false,
    lock_pending: false,
    tamper_level: 1,
    public_ip: null,
    last_seen: null,
    created_at: new Date().toISOString(),
    users: [],
  };
  mockDevices.push(dev);
  if (member_id) mockDeviceMember.set(id, member_id);
  return {
    device: dev,
    enroll_token: `mock-${id}-${Math.random().toString(36).slice(2, 10)}`,
  };
}

/** Device → the member it was set up for (the enroll intent). */
const mockDeviceMember = new Map<string, string>();

/** Mock for POST /api/devices/:id/enroll-token (pending devices only). */
export function mockRegenEnrollToken(id: string): EnrollTokenResponse {
  const dev = mockDevices.find((d) => d.id === id) ?? mockDevices[0];
  return {
    device: dev,
    enroll_token: `mock-${id}-${Math.random().toString(36).slice(2, 10)}`,
  };
}

/**
 * Mock for GET /api/family — assembled from the mock devices exactly the way
 * the server assembles it from real rows, so design-review mode exercises the
 * same shape the console gets in production.
 */
/** Design review: `?mock=solo` is a household of one — the parent and their
 * own computer; `?mock=empty` is a fresh server with nothing added yet. */
function household(): "full" | "solo" | "empty" {
  if (typeof window === "undefined") return "full";
  const m = new URLSearchParams(window.location.search).get("mock");
  return m === "solo" ? "solo" : m === "empty" ? "empty" : "full";
}

/** Design review scenarios on top of the household: `?mock=degraded` — the
 * living-room PC and the parent's laptop are stock desktops with no resolver,
 * so they can't filter websites; `?mock=stops` — Mia is out of time (21 of
 * her 17 minutes) and Leo is stopped by bedtime. */
function scenario(): string | null {
  if (typeof window === "undefined") return null;
  return new URLSearchParams(window.location.search).get("mock");
}

const NO_RESOLVER = ["dns_no_local_resolver", "dns_policy_not_loaded"];

/** The computers design review shows (all of them, or the solo household's). */
export function mockVisibleDevices(): Device[] {
  const h = household();
  if (h === "empty") return [];
  const all =
    scenario() === "degraded"
      ? mockDevices.map((d) =>
          d.id === "d-livingroom" || d.id === "d-parent"
            ? {
                ...d,
                last_state: {
                  locked: false,
                  frozen_users: [],
                  enforcing: false,
                  gaps: NO_RESOLVER,
                  agent_version: d.agent_version,
                  active_users: [],
                },
              }
            : d,
        )
      : mockDevices;
  if (h === "solo") return all.filter((d) => d.owner_account_id === mockAccount.id);
  return all;
}

/** Everyone design review shows in the household. */
export function mockVisibleAccounts(): Account[] {
  return household() === "full" ? mockHouseholdAccounts : mockHouseholdAccounts.filter((a) => a.role !== "member");
}

/** Keeps their own time: the parent themselves, or an adult. */
function selfManaged(a: Account): boolean {
  return a.role !== "member" || a.age_bracket === "adult" || a.self_managed;
}

/** The account an OS user belongs to — the link the server made, else by
 * display name the way the server links on enroll; an unknown OS user gets a
 * member made for it. */
function accountForOsUser(u: DeviceUser): Account {
  if (u.account_id) {
    const linked = mockHouseholdAccounts.find((a) => a.id === u.account_id);
    if (linked) return linked;
  }
  const name = u.display_name?.trim() || u.os_username;
  let acc = mockHouseholdAccounts.find(
    (a) => a.role === "member" && a.display_name.toLowerCase() === name.toLowerCase(),
  );
  if (!acc) {
    acc = {
      id: `acc-${u.os_username}`,
      household_id: TENANT_ID,
      display_name: name,
      email: null,
      role: "member",
      age_bracket: "kid",
      birthdate: null,
      self_managed: false,
      theme: null,
      effective_theme: "playful",
      profile_id: u.profile_id,
      created_at: new Date().toISOString(),
    };
    mockHouseholdAccounts.push(acc);
  }
  return acc;
}

export function mockFamily(): FamilyResponse {
  const byKey = new Map<string, FamilyChild>();
  const solo = household() !== "full";
  for (const d of mockVisibleDevices()) {
    for (const u of d.users ?? []) {
      const acc = accountForOsUser(u);
      // A parent's own login is theirs — never a card on the family wall.
      if (acc.role !== "member") continue;
      const own = selfManaged(acc);
      const profile =
        mockProfiles.find((p) => p.id === (acc.profile_id ?? u.profile_id)) ?? null;
      const st = profile?.policy.screen_time;
      // Someone who keeps their own time: the hub sees their minutes, never
      // their rules (the server blanks them the same way).
      const limit =
        !own && st?.enabled && (st.daily_limit_minutes ?? 0) > 0 ? st.daily_limit_minutes : null;
      const entry = {
        id: d.id,
        name: d.name,
        status: d.status,
        locked: d.locked,
        lock_pending: d.lock_pending,
        device_user_id: u.id,
      };
      const existing = byKey.get(acc.id);
      if (existing) {
        existing.used_minutes += u.used_minutes_today ?? 0;
        existing.earned_minutes += u.earned_minutes_today ?? 0;
        existing.devices.push(entry);
        existing.locked = existing.devices.every((x) => x.locked);
        if (existing.limit_minutes === null) existing.limit_minutes = limit;
      } else {
        byKey.set(acc.id, {
          key: acc.id,
          account_id: acc.id,
          name: acc.display_name,
          avatar: acc.avatar ?? null,
          age_bracket: acc.age_bracket,
          theme: acc.theme,
          effective_theme: acc.theme ?? defaultThemeFor(acc.age_bracket),
          locked: d.locked,
          used_minutes: u.used_minutes_today ?? 0,
          earned_minutes: u.earned_minutes_today ?? 0,
          limit_minutes: limit ?? null,
          profile_id: profile?.id ?? null,
          profile_name: u.profile_name ?? profile?.name ?? null,
          devices: [entry],
          pending_requests: mockEarnRequests.filter(
            (r) => r.os_username === u.os_username && r.status === "pending",
          ).length,
          blocked: mockBlocked.has(acc.id),
          managed: acc.age_bracket !== "adult",
          self_managed: own,
        });
      }
    }
  }
  // Members with no device yet still belong on the home screen.
  for (const acc of solo ? [] : mockHouseholdAccounts) {
    if (acc.role !== "member" || byKey.has(acc.id)) continue;
    const profile = mockProfiles.find((p) => p.id === acc.profile_id) ?? null;
    byKey.set(acc.id, {
      key: acc.id,
      account_id: acc.id,
      name: acc.display_name,
      avatar: acc.avatar ?? null,
      age_bracket: acc.age_bracket,
      theme: acc.theme,
      effective_theme: acc.theme ?? defaultThemeFor(acc.age_bracket),
      locked: false,
      used_minutes: 0,
      earned_minutes: 0,
      limit_minutes: null,
      profile_id: profile?.id ?? null,
      profile_name: profile?.name ?? null,
      devices: [],
      pending_requests: 0,
      blocked: mockBlocked.has(acc.id),
      managed: acc.age_bracket !== "adult",
      self_managed: selfManaged(acc),
    });
  }
  // Their own rules stay theirs: never in the hub's profile list.
  const ownRules = new Set(
    mockHouseholdAccounts.filter((a) => a.role === "member" && selfManaged(a)).map((a) => a.profile_id),
  );
  if (scenario() === "stops") {
    const tomorrowAt7 = new Date();
    tomorrowAt7.setUTCDate(tomorrowAt7.getUTCDate() + 1);
    tomorrowAt7.setUTCHours(7, 0, 0, 0);
    const stopped = (reason: "limit" | "bedtime", resume: Date | null) => ({
      allowed: false,
      reason,
      minutes_left: 0,
      stop_at: new Date().toISOString(),
      resume_at: resume?.toISOString() ?? null,
    });
    for (const c of byKey.values()) {
      if (c.name === "Mia") {
        Object.assign(c, { used_minutes: 21, earned_minutes: 0, limit_minutes: 17, left_minutes: 0 });
        c.rules = stopped("limit", null);
      }
      if (c.name === "Leo") {
        c.left_minutes = 0;
        c.rules = stopped("bedtime", tomorrowAt7);
      }
    }
  }
  return {
    children: [...byKey.values()].sort((a, b) => a.name.localeCompare(b.name)),
    // The family's computers come without their logins (the server's
    // /api/family has none; Computers reads them from /api/devices).
    devices: mockVisibleDevices().map(({ users: _users, ...d }) => d),
    profiles: mockProfiles.filter((p) => !ownRules.has(p.id)),
    requests: solo ? [] : mockEarnRequests.filter((r) => r.status === "pending"),
    server_time: new Date().toISOString(),
  };
}

/** Old account blocks the console only offers to lift (design review:
 * `?mock=blocked` blocks Noah). */
const mockBlocked = new Set<string>(
  typeof window !== "undefined" && new URLSearchParams(window.location.search).get("mock") === "blocked"
    ? ["acc-noah"]
    : [],
);
export function mockUnblockMember(id: string): void {
  mockBlocked.delete(id);
}

// ---- Members ------------------------------------------------------------------

/** Mock for POST /api/members — a new member with the bracket's preset rules. */
export function mockCreateMember(m: NewMember): Account {
  const bracket: AgeBracket = m.age_bracket ?? "kid";
  const preset =
    mockProfiles.find((p) => p.is_preset && p.kind === bracket) ??
    mockProfiles.find((p) => p.kind === (bracket === "little" || bracket === "kid" ? "kids" : bracket === "adult" ? "default" : "teen")) ??
    null;
  const acc: Account = {
    id: `acc-${Math.random().toString(36).slice(2, 8)}`,
    household_id: TENANT_ID,
    display_name: m.display_name.trim(),
    email: null,
    role: "member",
    age_bracket: bracket,
    birthdate: m.birthdate ?? null,
    self_managed: bracket === "adult" || bracket === "older_teen",
    theme: m.theme ?? null,
    effective_theme: m.theme ?? defaultThemeFor(bracket),
    profile_id: preset?.id ?? null,
    created_at: new Date().toISOString(),
  };
  mockHouseholdAccounts.push(acc);
  return acc;
}

export function mockUpdateMember(id: string, patch: MemberPatch): Account {
  const acc = mockHouseholdAccounts.find((a) => a.id === id);
  if (!acc) throw new Error("No such member");
  if (patch.display_name !== undefined) acc.display_name = patch.display_name;
  if (patch.birthdate !== undefined) acc.birthdate = patch.birthdate;
  if (patch.age_bracket !== undefined) acc.age_bracket = patch.age_bracket;
  if (patch.theme !== undefined) acc.theme = patch.theme;
  if (patch.profile_id !== undefined) acc.profile_id = patch.profile_id;
  acc.effective_theme = acc.theme ?? defaultThemeFor(acc.age_bracket);
  return { ...acc };
}

export function mockDeleteMember(id: string): void {
  const i = mockHouseholdAccounts.findIndex((a) => a.id === id);
  if (i >= 0) mockHouseholdAccounts.splice(i, 1);
  // Like the server: a computer set up for them that never joined goes too.
  for (let d = mockDevices.length - 1; d >= 0; d--) {
    const dev = mockDevices[d];
    const theirs = mockDeviceMember.get(dev.id) === id || dev.owner_account_id === id;
    if (theirs && dev.status === "pending") {
      mockDevices.splice(d, 1);
      mockDeviceMember.delete(dev.id);
    }
  }
}

// ---- Unlock codes -------------------------------------------------------------
// A rotating 6-digit code per device that the console shows on demand, and a
// set of one-time recovery codes. The mock keeps a secret per device and
// derives the code from (secret, 30 s step) so it ticks over like the real one.

const PERIOD = 30;
const mockSecrets = new Map<string, number>();
function mockSecret(deviceId: string): number {
  let s = mockSecrets.get(deviceId);
  if (s === undefined) {
    s = Math.floor(Math.random() * 2 ** 31);
    mockSecrets.set(deviceId, s);
  }
  return s;
}
function sixDigits(secret: number, counter: number): string {
  // A tiny integer hash — not TOTP, just stable and well spread.
  let h = (secret ^ counter) >>> 0;
  h = Math.imul(h ^ (h >>> 16), 0x45d9f3b) >>> 0;
  h = Math.imul(h ^ (h >>> 16), 0x45d9f3b) >>> 0;
  h = (h ^ (h >>> 16)) >>> 0;
  return String(h % 1_000_000).padStart(6, "0");
}
export function mockUnlockCode(deviceId: string): UnlockCode {
  const now = Math.floor(Date.now() / 1000);
  const name = mockDevices.find((d) => d.id === deviceId)?.name ?? "device";
  return {
    code: sixDigits(mockSecret(deviceId), Math.floor(now / PERIOD)),
    seconds_left: PERIOD - (now % PERIOD),
    period: PERIOD,
    device_name: name,
  };
}
export function mockRotateUnlockCode(deviceId: string): UnlockCodeRotated {
  mockSecrets.delete(deviceId);
  mockRecovery.delete(deviceId);
  const dev = mockDevices.find((d) => d.id === deviceId);
  if (dev) dev.recovery_codes_unused = 0;
  return { ...mockUnlockCode(deviceId), recovery_codes_cleared: true };
}

const mockRecovery = new Map<string, { codes: string[]; generated_at: string }>();
export function mockGenerateRecoveryCodes(deviceId: string): RecoveryCodes {
  const codes = Array.from({ length: 8 }, () => {
    const n = String(Math.floor(Math.random() * 1e8)).padStart(8, "0");
    return `${n.slice(0, 4)} ${n.slice(4)}`;
  });
  const set = { codes, generated_at: new Date().toISOString() };
  mockRecovery.set(deviceId, set);
  const dev = mockDevices.find((d) => d.id === deviceId);
  if (dev) dev.recovery_codes_unused = 8;
  return set;
}
export function mockRecoveryCodesStatus(deviceId: string): RecoveryCodesStatus {
  const set = mockRecovery.get(deviceId);
  const dev = mockDevices.find((d) => d.id === deviceId);
  const unused = set ? 8 : (dev?.recovery_codes_unused ?? 0);
  return {
    unused,
    total: 8,
    generated_at: set?.generated_at ?? (unused > 0 ? ago(9 * 24 * 60) : null),
  };
}

// ---- Confirm it's you ------------------------------------------------------------
// Mirrors the server's confirm window: 15 minutes, opened by a passkey or a code.

const CONFIRM_MS = 15 * 60_000;
// undefined = not asked yet. Design review: ?mock=confirmed opens the window
// from the start (screenshots of the keys).
let mockArmedUntil: number | null | undefined;
export const mockConfirm = {
  status(): ConfirmStatus {
    if (mockArmedUntil === undefined)
      mockArmedUntil =
        new URLSearchParams(window.location.search).get("mock") === "confirmed" ? Date.now() + CONFIRM_MS : null;
    if (mockArmedUntil !== null && mockArmedUntil <= Date.now()) mockArmedUntil = null;
    return {
      armed_until: mockArmedUntil === null ? null : new Date(mockArmedUntil).toISOString(),
      passkey: true,
      computer: true,
    };
  },
  open(): { armed_until: string } {
    mockArmedUntil = Date.now() + CONFIRM_MS;
    return { armed_until: new Date(mockArmedUntil).toISOString() };
  },
};

// ---- The person's own page --------------------------------------------------------

/** Whose session design review is: `?as=mia` signs in as Mia (a member
 * session — her own page and nothing else), `?as=jonas` as an adult keeping
 * his own time. Without it, the parent. */
function mockMeAccount(): Account {
  const q = typeof window !== "undefined" ? new URLSearchParams(window.location.search).get("as") : null;
  if (q) {
    const acc = mockHouseholdAccounts.find((a) => a.display_name.toLowerCase() === q.toLowerCase());
    if (acc) return acc;
  }
  return mockMe.account;
}

/** Mock for GET /api/me — the parent, or whoever `?as=` names. */
export function mockMeSession(): Me {
  const account = mockMeAccount();
  return account.id === mockMe.account.id ? mockMe : { ...mockMe, account };
}

/** Each self-managed person's own rules (the board's "My computer"). */
const myRules = new Map<string, MyRules>();
function ownRulesFor(acc: Account): MyRules {
  let r = myRules.get(acc.id);
  if (!r) {
    const p = mockProfiles.find((x) => x.id === acc.profile_id && x.kind === "adult");
    r = {
      daily_limit_minutes: p?.policy.screen_time.daily_limit_minutes ?? 180,
      focus_hours: p?.policy.focus?.hours ?? { days: [1, 2, 3, 4, 5], start: "09:00", end: "12:00" },
      sites: p?.policy.focus?.sites ?? ["reddit.com", "youtube.com", "news.ycombinator.com", "twitter.com"],
    };
    myRules.set(acc.id, r);
  }
  return r;
}

/** Mock for GET /api/me/rules. A child's rules are a parent's: refused. */
export function mockMyRules(): MyRules {
  const acc = mockMeAccount();
  if (!selfManaged(acc)) throw new Error("your rules are set by a parent");
  return { ...ownRulesFor(acc) };
}

/** Mock for PUT /api/me/rules — stored as given (the page validated it). */
export function mockSetMyRules(r: MyRules): MyRules {
  const acc = mockMeAccount();
  if (!selfManaged(acc)) throw new Error("your rules are set by a parent");
  const next = { ...r, sites: [...new Set(r.sites.map((s) => s.trim().toLowerCase()))] };
  myRules.set(acc.id, next);
  return { ...next };
}

/** Mock for GET /api/me/today, assembled from the same rows as the family. */
export function mockMeToday(): MeToday {
  const acc = mockMeAccount();
  const own = selfManaged(acc);
  const rows = mockVisibleDevices().flatMap((d) =>
    (d.users ?? []).filter((u) => accountForOsUser(u).id === acc.id).map((u) => ({ d, u })),
  );
  const profile = mockProfiles.find((p) => p.id === acc.profile_id) ?? null;
  const rules = own ? ownRulesFor(acc) : null;
  const used = rows.reduce((s, r) => s + (r.u.used_minutes_today ?? 0), 0);
  const earned = rows.reduce((s, r) => s + (r.u.earned_minutes_today ?? 0), 0);
  const st = profile?.policy.screen_time;
  const limit = rules
    ? rules.daily_limit_minutes || null
    : st?.enabled && st.daily_limit_minutes > 0
      ? st.daily_limit_minutes
      : null;
  const left = limit === null ? null : Math.max(0, limit + earned - used);
  // Someone who sets their own rules hears what a computer of theirs can't do.
  const devices = rows.map(({ d }) => ({
    name: d.name,
    status: d.status,
    locked: d.locked,
    ...(own ? { gaps: d.status === "online" ? d.last_state?.gaps ?? [] : [] } : {}),
  }));
  return {
    used_minutes: used,
    earned_minutes: earned,
    limit_minutes: limit,
    left_minutes: left,
    locked: devices.length > 0 && devices.every((d) => d.locked),
    devices,
    blocks: (!own && profile?.policy.blocks) || { apps: [], categories: [], custom_domains: [] },
    bracket: acc.age_bracket,
    theme: acc.theme ?? defaultThemeFor(acc.age_bracket),
    pending_request: mockPendingAsk,
    bedtime: (!own && st?.bedtime) || null,
    windows: (!own && st?.schedule) || [],
    self_managed: own,
    focus: rules ? { sites: rules.sites, hours: rules.focus_hours } : { sites: [], hours: null },
    // The server's rule (usage.rs `hub_exposure`), for sample data only.
    parent_sees:
      acc.age_bracket === "adult" || acc.self_managed
        ? { apps: false, sites: false }
        : { apps: true, sites: acc.age_bracket !== "older_teen" },
  };
}

let mockPendingAsk = false;
export function mockAskForTime(_minutes: number): void {
  mockPendingAsk = true;
}

// ---- Catalog ------------------------------------------------------------------------
// Mirrors policy/src/catalog.rs (ids + names only). The server serves the real
// thing at GET /api/catalog; this is what design review sees.

export const mockCatalog: Catalog = {
  categories: [
    { id: "social", name: "Social media", blurb: "Feeds, stories, likes.", app_ids: ["tiktok", "instagram", "snapchat", "facebook", "x", "reddit", "pinterest"] },
    { id: "video_streaming", name: "Video & streaming", blurb: "Shows, films, live streams.", app_ids: ["youtube", "twitch", "netflix", "disney_plus", "prime_video", "spotify"] },
    { id: "games", name: "Games", blurb: "Game launchers, stores and servers.", app_ids: ["roblox", "fortnite", "minecraft", "steam", "riot", "ea", "supercell", "playstation", "xbox", "among_us"] },
    { id: "messaging", name: "Chat & messaging", blurb: "Messengers and group chats.", app_ids: ["discord", "whatsapp", "telegram", "signal", "messenger", "omegle"] },
    { id: "adult", name: "Adult content", blurb: "Pornography and explicit sites.", app_ids: [] },
    { id: "gambling", name: "Gambling & betting", blurb: "Casinos, betting, loot-box sites.", app_ids: [] },
    { id: "dating", name: "Dating", blurb: "Dating and hook-up apps.", app_ids: ["tinder", "bumble"] },
    { id: "shopping", name: "Shopping", blurb: "Online stores and marketplaces.", app_ids: ["amazon", "temu", "shein"] },
    { id: "ai_chat", name: "AI chatbots", blurb: "Chatbots and AI companions.", app_ids: ["chatgpt", "character_ai"] },
    { id: "proxies", name: "VPNs, proxies & piracy", blurb: "Ways around the rules, and torrent sites.", app_ids: [] },
  ],
  apps: [
    { id: "youtube", name: "YouTube", category: "video_streaming", has_native_client: true },
    { id: "tiktok", name: "TikTok", category: "social", has_native_client: false },
    { id: "instagram", name: "Instagram", category: "social", has_native_client: false },
    { id: "snapchat", name: "Snapchat", category: "social", has_native_client: false },
    { id: "facebook", name: "Facebook", category: "social", has_native_client: false },
    { id: "x", name: "X (Twitter)", category: "social", has_native_client: false },
    { id: "reddit", name: "Reddit", category: "social", has_native_client: false },
    { id: "pinterest", name: "Pinterest", category: "social", has_native_client: false },
    { id: "discord", name: "Discord", category: "messaging", has_native_client: true },
    { id: "whatsapp", name: "WhatsApp", category: "messaging", has_native_client: true },
    { id: "telegram", name: "Telegram", category: "messaging", has_native_client: true },
    { id: "signal", name: "Signal", category: "messaging", has_native_client: true },
    { id: "messenger", name: "Messenger", category: "messaging", has_native_client: true },
    { id: "omegle", name: "Omegle & stranger chat", category: "messaging", has_native_client: false },
    { id: "twitch", name: "Twitch", category: "video_streaming", has_native_client: true },
    { id: "netflix", name: "Netflix", category: "video_streaming", has_native_client: false },
    { id: "disney_plus", name: "Disney+", category: "video_streaming", has_native_client: false },
    { id: "prime_video", name: "Prime Video", category: "video_streaming", has_native_client: false },
    { id: "spotify", name: "Spotify", category: "video_streaming", has_native_client: true },
    { id: "roblox", name: "Roblox", category: "games", has_native_client: true },
    { id: "fortnite", name: "Fortnite / Epic", category: "games", has_native_client: true },
    { id: "minecraft", name: "Minecraft", category: "games", has_native_client: true },
    { id: "steam", name: "Steam", category: "games", has_native_client: true },
    { id: "riot", name: "League / Valorant", category: "games", has_native_client: true },
    { id: "ea", name: "EA (FIFA, Sims)", category: "games", has_native_client: true },
    { id: "supercell", name: "Brawl Stars / Clash", category: "games", has_native_client: false },
    { id: "playstation", name: "PlayStation Network", category: "games", has_native_client: false },
    { id: "xbox", name: "Xbox Live", category: "games", has_native_client: false },
    { id: "among_us", name: "Among Us", category: "games", has_native_client: true },
    { id: "chatgpt", name: "ChatGPT", category: "ai_chat", has_native_client: false },
    { id: "character_ai", name: "Character.AI", category: "ai_chat", has_native_client: false },
    { id: "tinder", name: "Tinder", category: "dating", has_native_client: false },
    { id: "bumble", name: "Bumble", category: "dating", has_native_client: false },
    { id: "amazon", name: "Amazon", category: "shopping", has_native_client: false },
    { id: "temu", name: "Temu", category: "shopping", has_native_client: false },
    { id: "shein", name: "Shein", category: "shopping", has_native_client: false },
  ],
};

/** Mock for GET /api/usage/where and /api/me/where — a believable day. */
export function mockWhere(): WhereData {
  return {
    apps: [
      { key: "discord", seconds: 52 * 60 },
      { key: "minecraft", seconds: 40 * 60 },
      { key: "spotify", seconds: 35 * 60 },
      { key: "steam", seconds: 12 * 60 },
    ],
    sites: [
      { key: "youtube.com", hits: 420 },
      { key: "wikipedia.org", hits: 160 },
      { key: "discord.com", hits: 120 },
      { key: "github.com", hits: 60 },
    ],
    hours: [15, 16, 17, 19, 20].map((h) => {
      const d = new Date();
      d.setHours(h, 0, 0, 0);
      return { hour: d.toISOString(), amount: h === 17 ? 300 : 120 };
    }),
    sites_hidden_shared: false,
    sites_hidden_age: false,
  };
}
