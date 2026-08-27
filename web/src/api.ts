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
  type RegistrationResponseJSON,
  type AuthenticationResponseJSON,
} from "@simplewebauthn/browser";

import type {
  Account,
  Catalog,
  MemberPatch,
  MeHistory,
  MeToday,
  NewMember,
  ChangeModeStatus,
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
  StepUpGrant,
  SecondFactorMethod,
  TamperLevel,
  TotpEnrollment,
  TwoFactorStatus,
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
  mockMe,
  mockMeToday,
  mockPasskeys,
  mockProfiles,
  mockTwoFactor,
  mockChangeMode,
  mockUnlockCode,
  mockRotateUnlockCode,
  mockGenerateRecoveryCodes,
  mockRecoveryCodesStatus,
  mockUpdateMember,
  MOCK_STEPUP_CODE,
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

// ---- Auth ------------------------------------------------------------------

export const auth = {
  // webauthn-rs serializes challenges wrapped in `{ publicKey: {...} }`;
  // @simplewebauthn/browser wants the inner options object.
  async registerStart(email: string, display_name: string) {
    const res = await request<{
      publicKey: PublicKeyCredentialCreationOptionsJSON;
    }>("/api/auth/register/start", {
      method: "POST",
      body: JSON.stringify({ email, display_name }),
    });
    return res.publicKey;
  },

  async registerFinish(email: string, credential: RegistrationResponseJSON) {
    return request<{ admin: Me["admin"] }>("/api/auth/register/finish", {
      method: "POST",
      body: JSON.stringify({ email, credential }),
    });
  },

  async loginStart(email: string) {
    const res = await request<{
      publicKey: PublicKeyCredentialRequestOptionsJSON;
    }>("/api/auth/login/start", {
      method: "POST",
      body: JSON.stringify({ email }),
    });
    return res.publicKey;
  },

  async loginFinish(credential: AuthenticationResponseJSON) {
    return request<void>("/api/auth/login/finish", {
      method: "POST",
      body: JSON.stringify({ credential }),
    });
  },

  async logout() {
    return request<void>("/api/auth/logout", { method: "POST" });
  },

  /**
   * Device-voucher autologin: the installed client mints a one-time voucher the
   * local browser reads; the server verifies the device token + that this
   * account is permitted on the device, then issues a session. Contract:
   * voucher in → session out, server-verified (docs/AUTH.md).
   */
  async voucher(voucher: string) {
    return request<void>("/api/auth/voucher", {
      method: "POST",
      body: JSON.stringify({ voucher }),
    });
  },

  /** Full register ceremony: start → browser prompt → finish. */
  async register(email: string, display_name: string) {
    const options = await this.registerStart(email, display_name);
    const credential = await startRegistration({ optionsJSON: options });
    return this.registerFinish(email, credential);
  },

  /** Full login ceremony: start → browser prompt → finish. */
  async login(email: string) {
    const options = await this.loginStart(email);
    const credential = await startAuthentication({ optionsJSON: options });
    return this.loginFinish(credential);
  },
};

/** GET /api/auth/config — public; reports whether OIDC SSO is available. */
export async function getAuthConfig(): Promise<AuthConfig> {
  const res = await read<{ auth: AuthConfig }>("/api/auth/config", () => ({
    auth: { oidc: true, oidc_name: "Authentik" },
  }));
  return res.auth;
}

// ---- Session ---------------------------------------------------------------

export async function getMe(): Promise<Me> {
  return read<Me>("/api/me", () => mockMe);
}

// ---- Change mode (step-up 2FA) ----------------------------------------------
// "Reading is free; changing needs a second factor — once." A verified factor
// turns change mode on for 15 minutes (the server's step-up grant); the
// console locks it again on request, on expiry, or on reload if it lapsed.
// A mutation attempted without it returns STEP_UP_REQUIRED. See docs/AUTH.md.

export async function getTwoFactorStatus(): Promise<TwoFactorStatus> {
  return read<TwoFactorStatus>("/api/me/2fa", () => mockTwoFactor);
}

/** Begin authenticator-app enrollment — secret + otpauth URI, shown once. */
export async function startTotpEnrollment(): Promise<TotpEnrollment> {
  if (usingMock) {
    const secret = "JBSWY3DPEHPK3PXP";
    return {
      secret,
      otpauth_uri: `otpauth://totp/OpenScreenTime:${mockMe.account.email}?secret=${secret}&issuer=OpenScreenTime`,
    };
  }
  return request<TotpEnrollment>("/api/me/2fa/totp/start", { method: "POST" });
}

/** Confirm the authenticator by proving one live code before it counts. */
export async function confirmTotpEnrollment(code: string): Promise<void> {
  if (usingMock) {
    if (code.replace(/\s/g, "") !== MOCK_STEPUP_CODE) {
      throw new ApiError("invalid_code", "That code didn't match. Try again.", 400);
    }
    return;
  }
  return request<void>("/api/me/2fa/totp/confirm", {
    method: "POST",
    body: JSON.stringify({ code }),
  });
}

/** Ask the server to email a step-up code. Dev builds log it server-side. */
export async function startEmailStepUp(): Promise<void> {
  if (usingMock) return;
  return request<void>("/api/auth/stepup/email/start", { method: "POST" });
}

/** Verify a second factor; on success change mode is on for 15 minutes. */
export async function verifyStepUp(
  method: SecondFactorMethod,
  code: string,
): Promise<StepUpGrant> {
  if (usingMock) {
    if (code.replace(/\s/g, "") !== MOCK_STEPUP_CODE) {
      throw new ApiError("invalid_code", "That code didn't match. Try again.", 400);
    }
    return { method, ...mockChangeMode.enter() };
  }
  return request<StepUpGrant>("/api/auth/stepup/verify", {
    method: "POST",
    body: JSON.stringify({ method, code }),
  });
}

// ---- Client-first login (CONTRACT-0.6) --------------------------------------
// The browser asks by name; the person's own computer approves. PKCE-style:
// the verifier below never leaves this browser.

export interface DeviceLoginStart {
  request_id: string;
  devices: string[];
  expires_in_secs: number;
}

function b64url(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

/** A fresh PKCE pair: keep the verifier, send only the challenge. */
export async function pkcePair(): Promise<{ verifier: string; challenge: string }> {
  const raw = new Uint8Array(32);
  crypto.getRandomValues(raw);
  const verifier = b64url(raw);
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return { verifier, challenge: b64url(new Uint8Array(digest)) };
}

export async function startDeviceLogin(
  username: string,
  code_challenge: string,
): Promise<DeviceLoginStart> {
  if (usingMock)
    return { request_id: "mock-req", devices: ["Living Room PC"], expires_in_secs: 120 };
  return request<DeviceLoginStart>("/api/auth/device/start", {
    method: "POST",
    body: JSON.stringify({ username, code_challenge }),
  });
}

/** One poll. `status` is "pending" until the human at the machine answers. */
export async function finishDeviceLogin(
  request_id: string,
  code_verifier: string,
): Promise<{ status: string; role?: string }> {
  if (usingMock) return { status: "approved", role: "admin" };
  return request<{ status: string; role?: string }>("/api/auth/device/finish", {
    method: "POST",
    body: JSON.stringify({ request_id, code_verifier }),
  });
}

/** Ask the server to send one confirm-tap to the paired Telegram chat. */
export async function startTelegramStepUp(): Promise<void> {
  if (usingMock) return;
  return request<void>("/api/auth/stepup/telegram/start", { method: "POST" });
}

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

/** Is change mode on for this session (survives a reload), and until when. */
export async function getChangeMode(): Promise<ChangeModeStatus> {
  return read<ChangeModeStatus>("/api/auth/stepup", () => mockChangeMode.status());
}

/** Lock it down again, now. */
export async function lockChangeMode(): Promise<ChangeModeStatus> {
  if (usingMock) return mockChangeMode.lock();
  return request<ChangeModeStatus>("/api/auth/stepup/lock", { method: "POST" });
}

/** Another 15 minutes from now — once per grant (409 `already_extended`). */
export async function extendChangeMode(): Promise<ChangeModeStatus> {
  if (usingMock) return mockChangeMode.extend();
  return request<ChangeModeStatus>("/api/auth/stepup/extend", { method: "POST" });
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

// ---- The person's own page ---------------------------------------------------

/** GET /api/me/today — the signed-in person's own day. Reading is free, and
 * this is the one read a member session can make besides /api/me. */
export async function getMeToday(): Promise<MeToday> {
  return read<MeToday>("/api/me/today", () => mockMeToday());
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
