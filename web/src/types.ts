// ============================================================================
// Shared types — mirror docs/API.md (Policy) and docs/DATA_MODEL.md (entities)
// EXACTLY. Keep identical to the Rust serde shapes. Treat unknown fields
// leniently (forward-compat); optional sub-objects may be absent.
// ============================================================================

// ---- Policy (the jsonb document) -------------------------------------------

export type DnsMode = "default_deny" | "default_allow";
export type FirewallMode = "default_deny" | "default_allow";
export type UnlockChallenge = "math" | "wait" | "parent_pin";
export type StreakNudge = "bedtime" | "breaks" | string;

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

export interface AppLimit {
  match: string;
  daily_limit_minutes: number;
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

export interface StreaksPolicy {
  enabled: boolean;
  nudges: StreakNudge[];
}

export interface GamificationPolicy {
  earn_time: EarnTimePolicy;
  lockout: LockoutPolicy;
  streaks: StreaksPolicy;
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
  app_limits: AppLimit[];
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
  /** present on list + detail responses */
  users?: DeviceUser[];
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
  | "streak"
  | "enrolled"
  | "discovery_result"
  | "ssh"
  | "earn_request";

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

export interface Me {
  admin: Admin;
  tenant: Tenant;
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

export interface SshSession {
  id: string;
  device_id: string;
  admin_id: string;
  status: "opening" | "open" | "closed" | "failed";
  created_at: string;
  closed_at: string | null;
}

export interface SshSessionResponse {
  session: SshSession;
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

// ---- Discovery -------------------------------------------------------------

export interface DiscoveryHost {
  ip: string;
  mac: string;
  hostname?: string;
  open_ports: number[];
  vendor?: string;
}

export interface DiscoveryResult {
  id: string;
  device_id: string;
  created_at: string;
  hosts: DiscoveryHost[];
}

// ---- API error -------------------------------------------------------------

export interface ApiErrorBody {
  error: { code: string; message: string };
}
