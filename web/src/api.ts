// ============================================================================
// Typed API client for the Admin API (docs/API.md + docs/CONTRACT-PROD.md).
// Session-cookie auth via `credentials: "include"`.
// Mock mode: reads serve bundled sample data ONLY when the build runs with
// VITE_USE_MOCK=1 (design review). Production builds never fall back silently.
// ============================================================================

import {
  startAuthentication,
  startRegistration,
  type PublicKeyCredentialCreationOptionsJSON,
  type PublicKeyCredentialRequestOptionsJSON,
} from "@simplewebauthn/browser";

import type {
  Account,
  Catalog,
  MemberPatch,
  MeHistory,
  WhereData,
  MeToday,
  NewMember,
  CodeRequest,
  ConfirmGrant,
  ConfirmStatus,
  RecoveryCodes,
  RecoveryCodesStatus,
  UnlockCode,
  UnlockCodeRotated,
  CommandRow,
  VpnProfile,
  UsageHistoryResponse,
  ApiErrorBody,
  AuthConfig,
  Device,
  DeviceDetail,
  DeviceUser,
  EarnRequest,
  EarnRequestStatus,
  EnrollTokenResponse,
  FamilyResponse,
  Event,
  EventType,
  LockResponse,
  Me,
  Passkey,
  ParentToken,
  MintedParentToken,
  TelegramPairing,
  TelegramStatus,
  Policy,
  Profile,
  Severity,
  TamperLevel,
  VpnKind,
} from "./types";

import {
  mockAskForTime,
  mockCatalog,
  mockCreateMember,
  mockCreditTime,
  mockDeleteMember,
  mockDeviceDetail,
  mockDevices,
  mockRegenEnrollToken,
  mockCreateDevice,
  mockEarnRequests,
  mockEvents,
  mockFamily,
  mockHouseholdAccounts,
  mockMe,
  mockMeToday,
  mockPasskeys,
  mockProfiles,
  mockConfirm,
  mockUnlockCode,
  mockRotateUnlockCode,
  mockGenerateRecoveryCodes,
  mockRecoveryCodesStatus,
  mockUpdateMember,
  MOCK_CODE,
} from "./mock";

/** Design-review mode: bundled sample data instead of network reads. */
export const usingMock = import.meta.env.VITE_USE_MOCK === "1";

export class ApiError extends Error {
  code: string;
  status: number;
  constructor(code: string, message: string, status: number) {
    super(message);
    this.code = code;
    this.status = status;
    this.name = "ApiError";
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: "include",
    headers: {
      "Content-Type": "application/json",
      ...(init?.headers ?? {}),
    },
    ...init,
  });

  if (!res.ok) {
    let code = "http_error";
    let message = `${res.status} ${res.statusText}`;
    try {
      const body = (await res.json()) as Partial<ApiErrorBody>;
      if (body?.error) {
        code = body.error.code ?? code;
        message = body.error.message ?? message;
      }
    } catch {
      /* non-JSON error body */
    }
    throw new ApiError(code, message, res.status);
  }

  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

/** Read that serves bundled sample data when (and only when) VITE_USE_MOCK=1. */
async function read<T>(path: string, fallback: () => T, init?: RequestInit): Promise<T> {
  if (usingMock) return fallback();
  return request<T>(path, init);
}

// ---- Sign-in (docs/AUTH.md) ------------------------------------------------
// Two doors: your name, then a code shown on your own computer — or a passkey.
// webauthn-rs wraps its options as `{ publicKey: {...} }`;
// @simplewebauthn/browser wants the inner object.

function b64url(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

/** A fresh PKCE pair: keep the verifier in this tab, send only the challenge.
 * A code typed into another browser is useless without it. */
export async function pkcePair(): Promise<{ verifier: string; challenge: string }> {
  const raw = new Uint8Array(32);
  crypto.getRandomValues(raw);
  const verifier = b64url(raw);
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return { verifier, challenge: b64url(new Uint8Array(digest)) };
}

/** Mock mode accepts this code wherever one is typed. */
function mockCode(code: string) {
  if (code.replace(/\D/g, "") !== MOCK_CODE) {
    throw new ApiError("wrong_code", "That code didn't match — check your computer and try again.", 401);
  }
}

export const auth = {
  /** First run: your name, then a passkey — that creates the household. */
  async register(name: string, setupToken?: string) {
    if (usingMock) return;
    const res = await request<{ publicKey: PublicKeyCredentialCreationOptionsJSON }>(
      "/api/auth/register/start",
      { method: "POST", body: JSON.stringify({ name, setup_token: setupToken }) },
    );
    const credential = await startRegistration({ optionsJSON: res.publicKey });
    await request("/api/auth/register/finish", {
      method: "POST",
      body: JSON.stringify({ credential, setup_token: setupToken }),
    });
  },

  /** Sign in with a passkey — no name first; the passkey says whose it is. */
  async passkey() {
    if (usingMock) return;
    const res = await request<{ publicKey: PublicKeyCredentialRequestOptionsJSON }>(
      "/api/auth/login/start",
      { method: "POST" },
    );
    const credential = await startAuthentication({ optionsJSON: res.publicKey });
    await request("/api/auth/login/finish", {
      method: "POST",
      body: JSON.stringify({ credential }),
    });
  },

  /** Door one: a name in, a 6-digit code on that person's own computer. */
  async codeStart(name: string, code_challenge: string): Promise<CodeRequest> {
    if (usingMock) return { request_id: "mock", expires_in_secs: 300 };
    return request<CodeRequest>("/api/auth/code/start", {
      method: "POST",
      body: JSON.stringify({ name, code_challenge }),
    });
  },

  /** …and the code typed back, from the browser that asked. */
  async codeVerify(request_id: string, code_verifier: string, code: string): Promise<void> {
    if (usingMock) return mockCode(code);
    await request("/api/auth/code/verify", {
      method: "POST",
      body: JSON.stringify({ request_id, code_verifier, code }),
    });
  },

  async logout() {
    return request<void>("/api/auth/logout", { method: "POST" });
  },

  /** `ost login`'s one-time voucher (from the URL fragment) → a session. */
  async voucher(voucher: string) {
    return request<void>("/api/auth/voucher", {
      method: "POST",
      body: JSON.stringify({ voucher }),
    });
  },

  /** A recovery link from `openscreentime-server recover` → a session. */
  async link(token: string) {
    return request<void>("/api/auth/link", {
      method: "POST",
      body: JSON.stringify({ token }),
    });
  },
};

/** GET /api/auth/config — public; reports SSO availability + first-run state. */
export interface OidcSetup {
  /** the IdP-verified email the account will be stamped with */
  email: string;
  suggested_username: string;
  suggested_name: string;
}

/** First-run SSO: read the parked identity behind a /welcome?setup=… link. */
export async function getOidcSetup(token: string): Promise<OidcSetup> {
  if (usingMock) {
    return { email: "you@home.lan", suggested_username: "dad", suggested_name: "Dad" };
  }
  return request<OidcSetup>(`/api/auth/oidc/setup/${encodeURIComponent(token)}`);
}

/** First-run SSO: create the account with the chosen name and sign in. Like
 * the passkey first run, it needs the server's setup code when it has one. */
export async function finishOidcSetup(
  token: string,
  username: string,
  display_name?: string,
  setup_token?: string,
): Promise<void> {
  if (usingMock) return;
  await request(`/api/auth/oidc/setup/${encodeURIComponent(token)}`, {
    method: "POST",
    body: JSON.stringify({ username, display_name, setup_token }),
  });
}

export async function getAuthConfig(): Promise<AuthConfig> {
  const res = await read<{
    needs_setup?: boolean;
    setup_code_required?: boolean;
    auth: Pick<AuthConfig, "oidc" | "oidc_name">;
  }>("/api/auth/config", () => ({ needs_setup: false, auth: { oidc: false, oidc_name: "SSO" } }));
  return {
    ...res.auth,
    needs_setup: res.needs_setup ?? false,
    setup_code_required: res.setup_code_required ?? false,
  };
}

// ---- Session ---------------------------------------------------------------

export async function getMe(): Promise<Me> {
  return read<Me>("/api/me", () => mockMe);
}

// ---- Confirm it's you (the sensitive corner) --------------------------------
// Signing in is the proof; inside, only the keys (unlock codes, recovery
// codes, passkeys, pairing tokens) ask again: a passkey, or a code from your
// own computer, opens a 15-minute window. A fresh sign-in opens it too.

export async function getConfirmStatus(): Promise<ConfirmStatus> {
  return read<ConfirmStatus>("/api/auth/confirm", () => mockConfirm.status());
}

/** Confirm with your passkey. */
export async function confirmWithPasskey(): Promise<ConfirmGrant> {
  if (usingMock) return mockConfirm.open();
  const res = await request<{ publicKey: PublicKeyCredentialRequestOptionsJSON }>(
    "/api/auth/confirm/passkey/start",
    { method: "POST" },
  );
  const credential = await startAuthentication({ optionsJSON: res.publicKey });
  return request<ConfirmGrant>("/api/auth/confirm/passkey/finish", {
    method: "POST",
    body: JSON.stringify({ credential }),
  });
}

/** Send a code to your own computer. */
export async function startConfirmCode(): Promise<CodeRequest> {
  if (usingMock) return { request_id: "mock", expires_in_secs: 300 };
  return request<CodeRequest>("/api/auth/confirm/code/start", { method: "POST" });
}

/** …and type it back. */
export async function verifyConfirmCode(request_id: string, code: string): Promise<ConfirmGrant> {
  if (usingMock) {
    mockCode(code);
    return mockConfirm.open();
  }
  return request<ConfirmGrant>("/api/auth/confirm/code/verify", {
    method: "POST",
    body: JSON.stringify({ request_id, code }),
  });
}

// ---- Telegram alerts (one-way) ----------------------------------------------

/** Pairing state of the account's Telegram companion (Security room). */
export async function getTelegram(): Promise<TelegramStatus> {
  return read<TelegramStatus>("/api/me/telegram", () => ({
    configured: true,
    bot: "OpenScreenTimeBot",
    paired: false,
    username: null,
    paired_at: null,
  }));
}

/** Mint a pairing code, shown once — sent to the bot as /start <code>. */
export async function pairTelegram(): Promise<TelegramPairing> {
  if (usingMock)
    return {
      code: "SAMPLE42",
      bot: "OpenScreenTimeBot",
      deep_link: "https://t.me/OpenScreenTimeBot?start=SAMPLE42",
      expires_in_minutes: 10,
    };
  return request<TelegramPairing>("/api/me/telegram/pair", { method: "POST" });
}

/** Unpair every Telegram chat of this account. */
export async function unpairTelegram(): Promise<void> {
  if (usingMock) return;
  return request<void>("/api/me/telegram", { method: "DELETE" });
}

// ---- Family ----------------------------------------------------------------

/**
 * The whole home screen in one request: people, their day, their machines,
 * the profiles and anything waiting on a parent.
 *
 * Replaces the old fan-out (devices + profiles + one users call per device +
 * earn requests) with a single round trip that stays a single round trip as a
 * family grows.
 */
export async function getFamily(): Promise<FamilyResponse> {
  return read<FamilyResponse>("/api/family", () => mockFamily());
}

// ---- Devices ---------------------------------------------------------------

export async function listDevices(): Promise<Device[]> {
  const res = await read<{ devices: Device[] }>("/api/devices", () => ({
    devices: mockDevices,
  }));
  return res.devices;
}

export async function getDevice(id: string): Promise<DeviceDetail> {
  const res = await read<{
    device: Device;
    users: DeviceUser[];
    recent_events: Event[];
  }>(`/api/devices/${id}`, () => {
    const m = mockDeviceDetail(id);
    return { device: m, users: m.users, recent_events: m.recent_events };
  });
  return { ...res.device, users: res.users, recent_events: res.recent_events };
}

/**
 * Create a device (pending until the agent enrolls). The response carries the
 * one-time enroll token AND the device's parent code (authenticator secret),
 * both shown once. `member_id` is the enroll intent: the person this machine
 * is being set up for, so the server links its OS users to that account.
 */
export async function createDevice(
  name: string,
  member_id?: string,
): Promise<EnrollTokenResponse> {
  if (usingMock) return mockCreateDevice(name, member_id);
  return request<EnrollTokenResponse>("/api/devices", {
    method: "POST",
    body: JSON.stringify(member_id ? { name, member_id } : { name }),
  });
}

export async function updateDevice(
  id: string,
  patch: { name?: string; tamper_level?: TamperLevel },
): Promise<Device> {
  const res = await request<{ device: Device }>(`/api/devices/${id}`, {
    method: "PATCH",
    body: JSON.stringify(patch),
  });
  return res.device;
}

/** Ask the device to freeze. The server only queues the command; `locked`
 * flips once the agent's own state frame confirms — until then the device
 * shows `lock_pending`. */
export async function lockDevice(id: string): Promise<LockResponse> {
  if (usingMock) {
    const d = mockDevices.find((d) => d.id === id);
    if (d) {
      // The mock plays the agent too: pending for a beat, then confirmed.
      d.lock_pending = true;
      setTimeout(() => {
        d.lock_pending = false;
        d.locked = true;
      }, 1200);
    }
    return { command_id: "mock-cmd", queued: true, delivered: d?.status === "online" };
  }
  return request<LockResponse>(`/api/devices/${id}/lock`, { method: "POST" });
}

export async function unlockDevice(id: string): Promise<LockResponse> {
  if (usingMock) {
    const d = mockDevices.find((d) => d.id === id);
    if (d) {
      d.lock_pending = true;
      setTimeout(() => {
        d.lock_pending = false;
        d.locked = false;
      }, 900);
    }
    return { command_id: "mock-cmd", queued: true, delivered: d?.status === "online" };
  }
  return request<LockResponse>(`/api/devices/${id}/unlock`, { method: "POST" });
}

/** Allow (or end, with null) a window in which the device may be offline
 * without counting as trouble. Server: PUT /api/devices/{id}/offline-window. */
export async function setOfflineWindow(
  id: string,
  minutes: number | null,
): Promise<Device> {
  if (usingMock) {
    const d = mockDevices.find((d) => d.id === id);
    if (!d) throw new ApiError("not_found", "No such device", 404);
    d.offline_allowed_until =
      minutes === null ? null : new Date(Date.now() + minutes * 60_000).toISOString();
    return d;
  }
  const res = await request<{ device: Device }>(
    `/api/devices/${id}/offline-window`,
    { method: "PUT", body: JSON.stringify({ minutes }) },
  );
  return res.device;
}

/** Regenerate the one-time enroll token for a still-pending device (24 h TTL).
 * 409 once the device has enrolled. */
export async function regenEnrollToken(id: string): Promise<EnrollTokenResponse> {
  if (usingMock) return mockRegenEnrollToken(id);
  return request<EnrollTokenResponse>(`/api/devices/${id}/enroll-token`, {
    method: "POST",
    body: JSON.stringify({}),
  });
}

export async function deleteDevice(id: string): Promise<void> {
  return request<void>(`/api/devices/${id}`, { method: "DELETE" });
}

// ---- VPN profiles -----------------------------------------------------------

export async function listVpnProfiles(deviceId: string): Promise<VpnProfile[]> {
  const r = await request<{ profiles: VpnProfile[] }>(`/api/devices/${deviceId}/vpn`);
  return r.profiles;
}

export async function createVpnProfile(
  deviceId: string,
  name: string,
  config: string,
  kind?: VpnKind,
): Promise<void> {
  await request<unknown>(`/api/devices/${deviceId}/vpn`, {
    method: "POST",
    body: JSON.stringify({ name, config, kind }),
  });
}

export async function updateVpnProfile(id: string, name: string, config: string): Promise<void> {
  await request<unknown>(`/api/vpn-profiles/${id}`, {
    method: "PUT",
    body: JSON.stringify({ name, config }),
  });
}

export async function activateVpnProfile(id: string): Promise<void> {
  await request<unknown>(`/api/vpn-profiles/${id}/activate`, { method: "POST" });
}

export async function deactivateVpnProfile(id: string): Promise<void> {
  await request<unknown>(`/api/vpn-profiles/${id}/deactivate`, { method: "POST" });
}

export async function deleteVpnProfile(id: string): Promise<void> {
  await request<unknown>(`/api/vpn-profiles/${id}`, { method: "DELETE" });
}

// ---- Device users & profile assignment -------------------------------------

export async function listDeviceUsers(id: string): Promise<DeviceUser[]> {
  const res = await read<{ users: DeviceUser[] }>(
    `/api/devices/${id}/users`,
    () => ({ users: mockDeviceDetail(id).users }),
  );
  return res.users;
}

/** Grant extra screen time today (1–240 min) to one managed user. The server
 * credits today's ledger and pushes a `credit_time` command to the agent. */
export async function creditTime(
  deviceUserId: string,
  minutes: number,
): Promise<void> {
  if (usingMock) {
    mockCreditTime(deviceUserId, minutes);
    return;
  }
  await request<{ ok: boolean; minutes: number }>(
    `/api/device-users/${deviceUserId}/credit-time`,
    { method: "POST", body: JSON.stringify({ minutes }) },
  );
}

export async function assignProfile(
  deviceUserId: string,
  profile_id: string,
): Promise<void> {
  await request<{ ok: boolean }>(
    `/api/device-users/${deviceUserId}/assign-profile`,
    { method: "POST", body: JSON.stringify({ profile_id }) },
  );
}

/** Point an OS login on a computer at a person ("dad" is me, "m2011" is Mia).
 * Inside the confirm window: it decides who that login signs in as. */
export async function assignAccount(deviceUserId: string, account_id: string): Promise<void> {
  if (usingMock) return;
  await request<{ ok: boolean }>(`/api/device-users/${deviceUserId}/assign-account`, {
    method: "POST",
    body: JSON.stringify({ account_id }),
  });
}

// ---- Profiles --------------------------------------------------------------

export async function listProfiles(): Promise<Profile[]> {
  const res = await read<{ profiles: Profile[] }>("/api/profiles", () => ({
    profiles: mockProfiles,
  }));
  return res.profiles;
}

/**
 * `parent_pin` is sent as a top-level field alongside (not inside) `policy`:
 * absent/undefined preserves any existing hash, "" clears it, a non-empty
 * string sets a new one. The server hashes it — the plaintext never round-
 * trips back.
 */
export async function createProfile(
  name: string,
  policy: Policy,
  parent_pin?: string,
): Promise<Profile> {
  const res = await request<{ profile: Profile }>("/api/profiles", {
    method: "POST",
    body: JSON.stringify({
      name,
      kind: "custom",
      policy,
      ...(parent_pin !== undefined ? { parent_pin } : {}),
    }),
  });
  return res.profile;
}

export async function updateProfile(
  id: string,
  policy: Policy,
  parent_pin?: string,
): Promise<Profile> {
  if (usingMock) {
    const p = mockProfiles.find((p) => p.id === id);
    if (!p) throw new ApiError("not_found", "No such profile", 404);
    p.policy = policy;
    p.updated_at = new Date().toISOString();
    return { ...p };
  }
  const res = await request<{ profile: Profile }>(`/api/profiles/${id}`, {
    method: "PUT",
    body: JSON.stringify({
      policy,
      ...(parent_pin !== undefined ? { parent_pin } : {}),
    }),
  });
  return res.profile;
}

export async function deleteProfile(id: string): Promise<void> {
  return request<void>(`/api/profiles/${id}`, { method: "DELETE" });
}

// ---- Earn-time approval (contract §4) ---------------------------------------

export async function listEarnRequests(
  status?: EarnRequestStatus,
): Promise<EarnRequest[]> {
  const q = status ? `?status=${status}` : "";
  const res = await read<{ requests: EarnRequest[] }>(
    `/api/earn-requests${q}`,
    () => ({
      requests: mockEarnRequests.filter((r) => !status || r.status === status),
    }),
  );
  return res.requests;
}

export async function approveEarnRequest(id: string): Promise<EarnRequest> {
  const res = await request<{ request: EarnRequest }>(
    `/api/earn-requests/${id}/approve`,
    { method: "POST", body: JSON.stringify({}) },
  );
  return res.request;
}

export async function denyEarnRequest(id: string): Promise<EarnRequest> {
  const res = await request<{ request: EarnRequest }>(
    `/api/earn-requests/${id}/deny`,
    { method: "POST", body: JSON.stringify({}) },
  );
  return res.request;
}

// ---- Events ----------------------------------------------------------------

export interface EventFilter {
  device_id?: string;
  type?: EventType;
  severity?: Severity;
  limit?: number;
}

export async function listEvents(filter: EventFilter = {}): Promise<Event[]> {
  const qs = new URLSearchParams();
  if (filter.device_id) qs.set("device_id", filter.device_id);
  if (filter.type) qs.set("type", filter.type);
  if (filter.severity) qs.set("severity", filter.severity);
  if (filter.limit) qs.set("limit", String(filter.limit));
  const q = qs.toString();
  const res = await read<{ events: Event[] }>(
    `/api/events${q ? `?${q}` : ""}`,
    () => ({
      events: mockEvents.filter(
        (e) =>
          (!filter.device_id || e.device_id === filter.device_id) &&
          (!filter.type || e.type === filter.type) &&
          (!filter.severity || e.severity === filter.severity),
      ),
    }),
  );
  return res.events;
}

// ---- Passkeys (settings) ---------------------------------------------------

export async function listPasskeys(): Promise<Passkey[]> {
  const res = await read<{ passkeys: Passkey[] }>("/api/me/passkeys", () => ({
    passkeys: mockPasskeys,
  }));
  return res.passkeys;
}

/** Add another passkey to your account (inside the confirm window). */
export async function addPasskey(): Promise<void> {
  if (usingMock) return;
  const res = await request<{ publicKey: PublicKeyCredentialCreationOptionsJSON }>(
    "/api/me/passkeys/new/start",
    { method: "POST" },
  );
  const credential = await startRegistration({ optionsJSON: res.publicKey });
  await request("/api/me/passkeys/new/finish", {
    method: "POST",
    body: JSON.stringify({ credential }),
  });
}

export async function deletePasskey(id: string): Promise<void> {
  await request<{ ok: boolean }>(`/api/me/passkeys/${id}`, {
    method: "DELETE",
  });
}

// ---- Parent access tokens ---------------------------------------------------

export async function listParentTokens(): Promise<ParentToken[]> {
  const res = await read<{ tokens: ParentToken[] }>("/api/parent-tokens", () => ({
    tokens: [],
  }));
  return res.tokens;
}

export async function mintParentToken(label: string): Promise<MintedParentToken> {
  return request<MintedParentToken>("/api/parent-tokens", {
    method: "POST",
    body: JSON.stringify({ label }),
  });
}

export async function revokeParentToken(id: string): Promise<void> {
  await request<{ revoked: boolean }>(`/api/parent-tokens/${id}`, {
    method: "DELETE",
  });
}

// ---- Command queue ----------------------------------------------------------

export interface PingResult {
  ok: boolean;
  agent_version?: string;
  latency_ms?: number;
}

/** Liveness round-trip: enqueue a ping, then watch the command list until the
 *  agent acks it (or ~35s pass = no response). "It works" is a returned pong. */
export async function pingDevice(deviceId: string): Promise<PingResult> {
  if (usingMock) {
    await new Promise((r) => setTimeout(r, 600));
    return { ok: true, agent_version: "0.6.0", latency_ms: 380 };
  }
  const t0 = Date.now();
  const { command_id } = await request<{ command_id: string }>(
    `/api/devices/${deviceId}/ping`,
    { method: "POST" },
  );
  for (let i = 0; i < 35; i++) {
    await new Promise((r) => setTimeout(r, 1000));
    const cmds = await listCommands(deviceId);
    const c = cmds.find((x) => x.id === command_id);
    if (c && (c.status === "acked" || c.status === "failed")) {
      const res = (c.result ?? {}) as { pong?: boolean; agent_version?: string };
      return {
        ok: c.status === "acked" && res.pong === true,
        agent_version: res.agent_version,
        latency_ms: Date.now() - t0,
      };
    }
  }
  return { ok: false };
}

export async function listCommands(deviceId: string): Promise<CommandRow[]> {
  const r = await request<{ commands: CommandRow[] }>(`/api/devices/${deviceId}/commands`);
  return r.commands;
}

export async function cancelCommand(id: string): Promise<void> {
  await request<unknown>(`/api/commands/${id}/cancel`, { method: "POST" });
}

export async function getUsageHistory(
  deviceUserId: string,
  days: number,
): Promise<UsageHistoryResponse> {
  return request<UsageHistoryResponse>(`/api/device-users/${deviceUserId}/usage?days=${days}`);
}

// ---- Catalog (apps & categories) --------------------------------------------

/** GET /api/catalog — the built-in "block YouTube with one click" list. Names
 * only; the device holds the domain lists. Any session may read it. */
export async function getCatalog(): Promise<Catalog> {
  return read<Catalog>("/api/catalog", () => mockCatalog);
}

// ---- Members (children and self-tracking adults) -----------------------------

export async function createMember(m: NewMember): Promise<Account> {
  if (usingMock) return mockCreateMember(m);
  const res = await request<{ member: Account }>("/api/members", {
    method: "POST",
    body: JSON.stringify(m),
  });
  return res.member;
}

/** Everyone in the household — parents first (hub only). */
export async function listMembers(): Promise<Account[]> {
  const res = await read<{ members: Account[] }>("/api/members", () => ({
    members: mockHouseholdAccounts,
  }));
  return res.members;
}

export async function updateMember(id: string, patch: MemberPatch): Promise<Account> {
  if (usingMock) return mockUpdateMember(id, patch);
  const res = await request<{ member: Account }>(`/api/members/${id}`, {
    method: "PATCH",
    body: JSON.stringify(patch),
  });
  return res.member;
}

export async function deleteMember(id: string): Promise<void> {
  if (usingMock) return mockDeleteMember(id);
  await request<unknown>(`/api/members/${id}`, { method: "DELETE" });
}

/** Danger zone: block a child — cuts their login and locks their devices now. */
export async function blockMember(id: string): Promise<void> {
  if (usingMock) return;
  await request<unknown>(`/api/members/${id}/block`, { method: "POST" });
}

/** Danger zone: lift a block (devices stay locked until the parent resumes). */
export async function unblockMember(id: string): Promise<void> {
  if (usingMock) return;
  await request<unknown>(`/api/members/${id}/unblock`, { method: "POST" });
}

// ---- The person's own page ---------------------------------------------------

/** GET /api/me/today — the signed-in person's own day. Reading is free, and
 * this is the one read a member session can make besides /api/me. */
export async function getMeToday(): Promise<MeToday> {
  return read<MeToday>("/api/me/today", () => mockMeToday());
}

/** Set (or clear, with 0) the signed-in person's OWN daily goal. */
export async function setMyGoal(minutes: number): Promise<void> {
  if (usingMock) return;
  return request<void>("/api/me/goal", {
    method: "POST",
    body: JSON.stringify({ minutes }),
  });
}

/** Where today went — the parent's view of a person (`accountId`), or your
 * own when omitted. */
export async function getWhere(accountId?: string): Promise<WhereData> {
  const mock = (): WhereData => ({
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
  });
  if (accountId) return read<WhereData>(`/api/usage/where?account_id=${accountId}`, mock);
  return read<WhereData>("/api/me/where", mock);
}

/** The last two weeks of the person's own use, summed across their devices. */
export async function getMeHistory(): Promise<MeHistory> {
  return read<MeHistory>("/api/me/history", () => {
    // A believable sample week for design review: school-day dips, a weekend
    // spike, today still in progress.
    // Today (the last slot) stays low so the page's live "used today" wins.
    const pattern = [95, 110, 70, 125, 88, 160, 142, 90, 105, 74, 118, 96, 150, 0];
    const days = pattern.map((used, i) => {
      const d = new Date();
      d.setDate(d.getDate() - (pattern.length - 1 - i));
      return {
        day: d.toISOString().slice(0, 10),
        used_minutes: used,
        earned_minutes: i % 5 === 0 ? 15 : 0,
      };
    });
    return {
      days,
      today_by_device: [
        { name: "Living Room PC", used_minutes: 31 },
        { name: "Studio Laptop", used_minutes: 16 },
      ],
      goal_minutes: 120,
      goal_streak: 4,
    };
  });
}

/** POST /api/me/ask — "can I have more time?" to the parent. Not available
 * in the little bracket (no request UI) or to adults (no one to ask). */
export async function askForTime(minutes: number, reason?: string): Promise<void> {
  if (usingMock) {
    mockAskForTime(minutes);
    return;
  }
  await request<unknown>("/api/me/ask", {
    method: "POST",
    body: JSON.stringify(reason ? { minutes, reason } : { minutes }),
  });
}

// ---- Unlock codes (per device) ------------------------------------------------
// The device verifies these offline; the server holds the secret and shows the
// parent the current code. Reading it is a sensitive read: 428 without change
// mode on, so callers wrap it in guard().

export async function getUnlockCode(deviceId: string): Promise<UnlockCode> {
  if (usingMock) return mockUnlockCode(deviceId);
  return request<UnlockCode>(`/api/devices/${deviceId}/unlock-code`);
}

/** New secret: the old codes stop working once the device checks in, and the
 * recovery codes (keyed by the old secret) are cleared. */
export async function rotateUnlockCode(deviceId: string): Promise<UnlockCodeRotated> {
  if (usingMock) return mockRotateUnlockCode(deviceId);
  return request<UnlockCodeRotated>(`/api/devices/${deviceId}/unlock-code/rotate`, {
    method: "POST",
    body: JSON.stringify({}),
  });
}

/** Eight fresh one-time codes, shown once; replaces any previous set. */
export async function generateRecoveryCodes(deviceId: string): Promise<RecoveryCodes> {
  if (usingMock) return mockGenerateRecoveryCodes(deviceId);
  return request<RecoveryCodes>(`/api/devices/${deviceId}/recovery-codes`, {
    method: "POST",
    body: JSON.stringify({}),
  });
}

export async function getRecoveryCodes(deviceId: string): Promise<RecoveryCodesStatus> {
  if (usingMock) return mockRecoveryCodesStatus(deviceId);
  return request<RecoveryCodesStatus>(`/api/devices/${deviceId}/recovery-codes`);
}
