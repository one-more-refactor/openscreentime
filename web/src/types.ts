// ============================================================================
// Shared types — mirror docs/API.md (Policy) and docs/DATA_MODEL.md (entities)
// EXACTLY. Keep identical to the Rust serde shapes. Treat unknown fields
// leniently (forward-compat); optional sub-objects may be absent.
// ============================================================================

// ---- Policy (the jsonb document) -------------------------------------------

// "allow_all" is the exact string the agent tests for — `Policy::is_default_deny`
// in the shared crate is `self.mode != "allow_all"`, so ANY other value means
// default-deny. The web previously declared "default_allow", which is not that
// string: choosing the permissive option in the console wrote a value the agent
// read as default-deny, and the UI showed "allow" while the device blocked
// everything. Same silent drift as the schedule/windows mismatch.
export type DnsMode = "default_deny" | "allow_all";
export type FirewallMode = "default_deny" | "allow_all";
export type UnlockChallenge = "math" | "wait" | "parent_pin";

export interface DnsPolicy {
  mode: DnsMode;
  allowlist: string[];
  blocklist: string[];
  safe_search: boolean;
  upstream: string;
}

export interface FirewallPolicy {
  mode: FirewallMode;
  allow_outbound_ports: number[];
  allow_inbound_ports: number[];
}

export interface TimeWindow {
  /** weekday numbers, 0 = Sunday */
  days: number[];
  /** "HH:MM" 24h */
  start: string;
  /** "HH:MM" 24h */
  end: string;
}

export interface Bedtime {
  start: string;
  end: string;
}

export interface ScreenTimePolicy {
  enabled: boolean;
  daily_limit_minutes: number;
  schedule: TimeWindow[];
  bedtime: Bedtime | null;
}

export interface EarnTask {
  id: string;
  label: string;
  reward_minutes: number;
}

export interface EarnTimePolicy {
  enabled: boolean;
  tasks: EarnTask[];
}

export interface LockoutPolicy {
  enabled: boolean;
  unlock_challenge: UnlockChallenge;
}

export interface GamificationPolicy {
  earn_time: EarnTimePolicy;
  lockout: LockoutPolicy;
}

export interface NetworkLockdown {
  force_dns: boolean;
  block_doh: boolean;
  block_dot: boolean;
  block_tor: boolean;
  block_vpn: boolean;
  /** Days the device may run without reaching the server before it hard-locks
   * itself (parent PIN always unlocks). 0 = never; omitted when 0 so preset
   * JSON stays byte-identical with the policy crate's serde output. */
  offline_lockdown_days?: number;
}

export interface Policy {
  version: number;
  dns: DnsPolicy;
  firewall: FirewallPolicy;
  screen_time: ScreenTimePolicy;
  gamification: GamificationPolicy;
  /** Absent (or omitted by the server) means all lockdown flags are off. */
  lockdown?: NetworkLockdown;
  /** Argon2 hash of the parent PIN, present only when a PIN is set. Never the
   * plaintext PIN — the editor writes a new PIN via a separate `parent_pin`
   * field on the save request, not through this property. */
  parent_pin_hash?: string | null;
}

// ---- Entities --------------------------------------------------------------

export type ProfileKind = "kids" | "teen" | "default" | "custom";

export interface Profile {
  id: string;
  tenant_id: string;
  name: string;
  kind: ProfileKind;
  is_preset: boolean;
  policy: Policy;
  created_at: string;
  updated_at: string;
}

export type DeviceStatus = "pending" | "online" | "offline" | "locked";
export type TamperLevel = 1 | 3;

export interface DeviceUser {
  id: string;
  device_id: string;
  os_username: string;
  display_name: string | null;
  profile_id: string;
  /** joined from profiles on device detail/user responses */
  profile_name?: string;
  profile_kind?: ProfileKind;
  /** joined from screen_time_ledger on GET /api/devices/:id/users */
  used_minutes_today?: number;
  earned_minutes_today?: number;
  /** present in mock data only; the server does not return it */
  created_at?: string;
}

/** One row of a device's command queue (GET /api/devices/:id/commands). */
export interface CommandRow {
  id: string;
  type: string;
  payload: Record<string, unknown>;
  status: "queued" | "sent" | "acked" | "failed" | "cancelled";
  result: Record<string, unknown> | null;
  created_at: string;
  sent_at: string | null;
  acked_at: string | null;
}

export interface Device {
  id: string;
  tenant_id: string;
  name: string;
  hostname: string;
  os: string;
  agent_version: string;
  status: DeviceStatus;
  tamper_level: TamperLevel;
  public_ip: string | null;
  last_seen: string | null;
  created_at: string;
  /** bumped when the device's VPN profiles change (cache-busting stamp) */
  vpn_updated_at?: string | null;
  /** while set and in the future, being unreachable is allowed, not trouble */
  offline_allowed_until?: string | null;
  /** present on list + detail responses */
  users?: DeviceUser[];
  /** command types still queued/sent — server-backed PENDING chips */
  pending_commands?: string[];
}

export type VpnKind = "wireguard" | "openvpn";

/** A named VPN profile (GET /api/devices/:id/vpn). Configs render MASKED —
 * secrets appear as ••• and survive edit round-trips server-side. */
export interface VpnProfile {
  id: string;
  name: string;
  kind: VpnKind;
  config_masked: string;
  status: "untested" | "testing" | "active" | "failed";
  last_error: string | null;
  last_tested_at: string | null;
  is_active: boolean;
  updated_at: string;
}

export interface DeviceDetail extends Device {
  users: DeviceUser[];
  recent_events: Event[];
}

export type EventType =
  | "heartbeat"
  | "tamper"
  | "lock"
  | "unlock"
  | "policy_applied"
  | "screen_time_exceeded"
  | "screen_time_earned"
  | "enrolled"
  | "ssh"
  | "earn_request"
  | "evasion"
  | "enforcement_degraded"
  | "vpn_profile";

export type Severity = "info" | "warn" | "critical";

export interface Event {
  id: string;
  tenant_id: string;
  device_id: string | null;
  device_user_id: string | null;
  type: EventType;
  severity: Severity;
  payload: Record<string, unknown>;
  created_at: string;
}

export interface Admin {
  id: string;
  tenant_id: string;
  email: string;
  display_name: string;
  created_at: string;
}

export interface Tenant {
  id: string;
  name: string;
  created_at: string;
}

export interface Passkey {
  id: string;
  nickname: string;
  created_at: string;
  last_used_at: string | null;
}

/** A scoped parent access token (the raw value is never returned after mint). */
export interface ParentToken {
  id: string;
  label: string;
  created_at: string;
  last_used_at: string | null;
  revoked: boolean;
}

/** Response from minting a parent token — `token` is shown exactly once. */
export interface MintedParentToken {
  id: string;
  label: string;
  token: string;
}

// ---- People, roles, age brackets -------------------------------------------
// The account model the "everyone has an account" pivot introduces. Grounded
// in docs/AUTH.md. Kept alongside the legacy Admin/Tenant during the interleave
// so existing call sites keep working while new ones move to Account/Household.

export type Role = "owner" | "parent" | "member";
// owner   — the founding parent; full control of the household
// parent  — a co-parent / guardian with the same hub powers
// member  — a managed or self-tracking person (kid, teen, or adult)

export type AgeBracket =
  | "little" // 0–6   hard limits, no login, parent does everything
  | "kid" // 6–12  hard limits, can request/earn time
  | "younger_teen" // 12–16 goals + limits, wind-down before a hard stop
  | "older_teen" // 16–18 mostly self-set, parent can still cap
  | "adult"; // 18+   private self-tracking, self-imposed limits only

export const AGE_BRACKETS: { key: AgeBracket; label: string; range: string }[] = [
  { key: "little", label: "Little", range: "0–6" },
  { key: "kid", label: "Kid", range: "6–12" },
  { key: "younger_teen", label: "Younger teen", range: "12–16" },
  { key: "older_teen", label: "Older teen", range: "16–18" },
  { key: "adult", label: "Adult", range: "18+" },
];

export interface Household {
  id: string;
  name: string;
  created_at: string;
}

export interface Account {
  id: string;
  household_id: string;
  display_name: string;
  /** Members young enough not to log in may have no email. */
  email: string | null;
  role: Role;
  age_bracket: AgeBracket;
  /** YYYY-MM-DD; the source of truth the bracket is derived from. */
  birthdate: string | null;
  /** Older teens & adults track privately; the hub sees less of them. */
  self_managed: boolean;
  created_at: string;
}

export interface Me {
  /** The unified identity going forward. */
  account: Account;
  household: Household;
  /** @deprecated transition aliases while call sites migrate off Admin/Tenant. */
  admin: Admin;
  /** @deprecated transition alias — use `household`. */
  tenant: Tenant;
}

// ---- Two-factor / step-up ("reading is free, changing needs a factor") -----

export type SecondFactorMethod = "totp" | "email";

/** Error code the server returns from a mutation with no valid step-up grant. */
export const STEP_UP_REQUIRED = "step_up_required";

export interface TwoFactorStatus {
  /** An authenticator-app secret is enrolled and confirmed. */
  totp_enrolled: boolean;
  /** The account has an email a code can be sent to. */
  email_available: boolean;
}

/** Returned by TOTP enrollment start — the secret is shown exactly once. */
export interface TotpEnrollment {
  /** base32 secret for manual entry. */
  secret: string;
  /** otpauth://totp/… — render as a QR for scanning into the app. */
  otpauth_uri: string;
}

/** A successful step-up: the grant is valid until `expires_at`. */
export interface StepUpGrant {
  method: SecondFactorMethod;
  expires_at: string;
}

// ---- Command / action responses --------------------------------------------

export interface EnrollTokenResponse {
  device: Device;
  enroll_token: string;
}

/** POST /api/devices/:id/lock | /unlock. `delivered: false` means the command
 * is queued and the status will only flip once the agent reconnects and acks. */
export interface LockResponse {
  command_id: string;
  queued: boolean;
  delivered: boolean;
}

// ---- Earn-time approval (contract §4) ---------------------------------------

export type EarnRequestStatus = "pending" | "approved" | "denied";

export interface EarnRequest {
  id: string;
  tenant_id?: string;
  device_id: string;
  device_user_id: string;
  os_username: string;
  task_id: string;
  task_label: string;
  minutes: number;
  status: EarnRequestStatus;
  created_at: string;
  decided_at: string | null;
  /** joined by the server for the admin list */
  device_name?: string;
  user_display_name?: string | null;
}

// ---- Auth config (contract §6) ----------------------------------------------

export interface AuthConfig {
  oidc: boolean;
  oidc_name: string;
}

// ---- API error -------------------------------------------------------------

export interface ApiErrorBody {
  error: { code: string; message: string };
}

// ---- Screen-time history ----------------------------------------------------

export interface UsageDay {
  day: string; // YYYY-MM-DD
  used_minutes: number;
  earned_minutes: number;
}

export interface UsageHistoryResponse {
  days: UsageDay[];
  /** consecutive days with any usage, counted back from today (server-computed) */
  streak_days: number;
}
