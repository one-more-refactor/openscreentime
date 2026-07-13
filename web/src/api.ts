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
  Me,
  Passkey,
  Policy,
  Profile,
  Severity,
  SshSessionResponse,
  TamperLevel,
} from "./types";

import {
  mockDeviceDetail,
  mockDevices,
  mockDiscovery,
  mockEarnRequests,
  mockEvents,
  mockMe,
  mockPasskeys,
  mockProfiles,
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

export async function lockDevice(id: string): Promise<void> {
  return request<void>(`/api/devices/${id}/lock`, { method: "POST" });
}

export async function unlockDevice(id: string): Promise<void> {
  return request<void>(`/api/devices/${id}/unlock`, { method: "POST" });
}

export async function deleteDevice(id: string): Promise<void> {
  return request<void>(`/api/devices/${id}`, { method: "DELETE" });
}

// ---- SSH (contract §3) -------------------------------------------------------

export async function openSsh(id: string): Promise<SshSessionResponse> {
  return request<SshSessionResponse>(`/api/devices/${id}/ssh`, {
    method: "POST",
    body: JSON.stringify({}),
  });
}

export async function closeSsh(sessionId: string): Promise<void> {
  return request<void>(`/api/ssh/${sessionId}/close`, {
    method: "POST",
    body: JSON.stringify({}),
  });
}

/** Cookie-authenticated browser terminal WebSocket URL for a session. */
export function sshWsUrl(sessionId: string): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}/api/ssh/${sessionId}/ws`;
}

// ---- Device users & profile assignment -------------------------------------

export async function listDeviceUsers(id: string): Promise<DeviceUser[]> {
  const res = await read<{ users: DeviceUser[] }>(
    `/api/devices/${id}/users`,
    () => ({ users: mockDeviceDetail(id).users }),
  );
  return res.users;
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

export async function createProfile(
  name: string,
  policy: Policy,
): Promise<Profile> {
  const res = await request<{ profile: Profile }>("/api/profiles", {
    method: "POST",
    body: JSON.stringify({ name, kind: "custom", policy }),
  });
  return res.profile;
}

export async function updateProfile(
  id: string,
  policy: Policy,
): Promise<Profile> {
  const res = await request<{ profile: Profile }>(`/api/profiles/${id}`, {
    method: "PUT",
    body: JSON.stringify({ policy }),
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
