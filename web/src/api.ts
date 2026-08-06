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
  CommandRow,
  VpnProfile,
  UsageHistoryResponse,
  ApiErrorBody,
  AuthConfig,
  Device,
  DeviceDetail,
  DeviceUser,
  DiscoveryResult,
  EarnRequest,
  EarnRequestStatus,
  EnrollTokenResponse,
  Event,
  EventType,
  LockResponse,
  Me,
  Passkey,
  ParentToken,
  MintedParentToken,
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
  mockCreditTime,
  mockDeviceDetail,
  mockDevices,
  mockDiscovery,
  mockRegenEnrollToken,
  mockCreateDevice,
  mockEarnRequests,
  mockEvents,
  mockMe,
  mockPasskeys,
  mockProfiles,
  mockTwoFactor,
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

// ---- Step-up 2FA -----------------------------------------------------------
// "Reading is free; every change needs a second factor." The mutation itself
// (in api calls below) returns STEP_UP_REQUIRED when no grant is live; the UI
// catches that, runs one of these flows, then retries. See docs/AUTH.md.

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

/** Verify a second factor; on success a short-lived step-up grant is set. */
export async function verifyStepUp(
  method: SecondFactorMethod,
  code: string,
): Promise<StepUpGrant> {
  if (usingMock) {
    if (code.replace(/\s/g, "") !== MOCK_STEPUP_CODE) {
      throw new ApiError("invalid_code", "That code didn't match. Try again.", 400);
    }
    // A five-minute grant, mirroring the server's window.
    const expires = new Date(Date.now() + 5 * 60_000).toISOString();
    return { method, expires_at: expires };
  }
  return request<StepUpGrant>("/api/auth/stepup/verify", {
    method: "POST",
    body: JSON.stringify({ method, code }),
  });
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

export async function createDevice(name: string): Promise<EnrollTokenResponse> {
  if (usingMock) return mockCreateDevice(name);
  return request<EnrollTokenResponse>("/api/devices", {
    method: "POST",
    body: JSON.stringify({ name }),
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

export async function lockDevice(id: string): Promise<LockResponse> {
  if (usingMock) {
    const d = mockDevices.find((d) => d.id === id);
    if (d) d.status = "locked";
    return { command_id: "mock-cmd", queued: true, delivered: true };
  }
  return request<LockResponse>(`/api/devices/${id}/lock`, { method: "POST" });
}

export async function unlockDevice(id: string): Promise<LockResponse> {
  if (usingMock) {
    const d = mockDevices.find((d) => d.id === id);
    if (d) d.status = "online";
    return { command_id: "mock-cmd", queued: true, delivered: true };
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

// ---- Discovery -------------------------------------------------------------

export async function scanDiscovery(device_id: string): Promise<void> {
  return request<void>("/api/discovery/scan", {
    method: "POST",
    body: JSON.stringify({ device_id }),
  });
}

/** Raw row from GET /api/discovery/results: a `discovery_result` event whose
 *  payload carries the found hosts. */
interface DiscoveryRow {
  id: string;
  device_id: string;
  payload: { hosts?: DiscoveryResult["hosts"] };
  created_at: string;
}

export async function getDiscoveryResults(): Promise<DiscoveryResult[]> {
  const res = await read<{ results: DiscoveryRow[] }>(
    "/api/discovery/results",
    () => ({
      results: [
        {
          id: mockDiscovery.id,
          device_id: mockDiscovery.device_id,
          payload: { hosts: mockDiscovery.hosts },
          created_at: mockDiscovery.created_at,
        },
      ],
    }),
  );
  return res.results.map((r) => ({
    id: r.id,
    device_id: r.device_id,
    created_at: r.created_at,
    hosts: r.payload.hosts ?? [],
  }));
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
