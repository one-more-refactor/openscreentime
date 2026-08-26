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
  /** One-click app / category blocks from the built-in catalog. Absent =
   * nothing blocked. */
  blocks?: AppBlocks;
}

/** Catalog-driven blocks (policy crate `AppBlocks`). Ids refer to
 * `GET /api/catalog`; the device expands them into DNS sinkholes + process
 * names. `custom_domains` are the ones a parent typed by hand. */
export interface AppBlocks {
  apps: string[];
  categories: string[];
  custom_domains: string[];
}

export const EMPTY_BLOCKS: AppBlocks = { apps: [], categories: [], custom_domains: [] };

/** GET /api/catalog — names only; the device holds the domain lists. */
export interface CatalogCategory {
  id: string;
  name: string;
  blurb: string;
  app_ids: string[];
}
export interface CatalogApp {
  id: string;
  name: string;
  category: string;
  has_native_client: boolean;
}
export interface Catalog {
  categories: CatalogCategory[];
  apps: CatalogApp[];
}

// ---- Entities --------------------------------------------------------------

export type ProfileKind =
  | "little"
  | "kid"
  | "younger_teen"
  | "older_teen"
  | "adult"
  | "custom"
  // pre-0.4 presets, still valid rows
  | "kids"
  | "teen"
  | "default";

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

/** Connection state only. Since 0.4 "locked" is its own flag (`Device.locked`),
 * never a status value — a paused laptop is still online. */
export type DeviceStatus = "pending" | "online" | "offline";

/** The agent's last `state` frame — what the kernel actually says. */
export interface DeviceLastState {
  locked: boolean;
  frozen_users: string[];
  enforcing: boolean;
  gaps: string[];
  agent_version?: string;
  active_users?: string[];
  [k: string]: unknown;
}
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
  /** The truth: the agent reported its screens frozen. */
  locked: boolean;
  /** A lock/unlock command is queued or sent and not yet confirmed. */
  lock_pending: boolean;
  last_state?: DeviceLastState | null;
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
  /** one-time recovery codes not yet used (0 when none were generated) */
  recovery_codes_unused?: number;
}

// ---- Family (GET /api/family) ----------------------------------------------

/** One device a person uses, as carried on a FamilyChild. */
export interface ChildDevice {
  /** the device */
  id: string;
  name: string;
  status: DeviceStatus;
  locked: boolean;
  lock_pending: boolean;
  /** this person's row on that device — what per-user actions address */
  device_user_id: string;
}

/**
 * A person, assembled server-side across every machine they use. The same OS
 * username on two devices is one child whose day is the sum of both.
 */
export interface FamilyChild {
  /** the member's account id — stable identity across devices, and the URL segment */
  key: string;
  account_id: string;
  name: string;
  age_bracket: AgeBracket;
  /** the parent's explicit pick, or null for "auto by bracket" */
  theme: Theme | null;
  /** what the person's own page actually renders */
  effective_theme: Theme;
  /** every device they use reports frozen */
  locked: boolean;
  used_minutes: number;
  earned_minutes: number;
  /** null = no limit configured (disabled or zero — never "0 left of 0") */
  limit_minutes: number | null;
  profile_id: string | null;
  profile_name: string | null;
  devices: ChildDevice[];
  /** earn requests waiting on a parent */
  pending_requests: number;
}

export interface FamilyResponse {
  children: FamilyChild[];
  devices: Device[];
  profiles: Profile[];
  requests: EarnRequest[];
  server_time: string;
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
  | "vpn_profile"
  | "parent_code_ok"
  | "parent_code_failed"
  | "parent_code_backup_used"
  | "app_blocked";

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
  /** The parent's explicit theme pick, or null = auto by bracket. */
  theme: Theme | null;
  /** The theme the person's own page renders. */
  effective_theme: Theme;
  /** The person's rules (a profile), for members. */
  profile_id: string | null;
  created_at: string;
}

/** How a person's own page looks — playful for small children, calm for
 * teens, plain for adults. Null on an account means "auto by bracket". */
export type Theme = "playful" | "calm" | "plain";

export const THEMES: { key: Theme; label: string; blurb: string }[] = [
  { key: "playful", label: "Playful", blurb: "Big friendly ring, bright colours — for little ones" },
  { key: "calm", label: "Calm", blurb: "Quieter stats and goals — for teens" },
  { key: "plain", label: "Plain", blurb: "A compact private dashboard — for adults" },
];

export function defaultThemeFor(b: AgeBracket): Theme {
  return b === "little" || b === "kid" ? "playful" : b === "adult" ? "plain" : "calm";
}

/** Age from a YYYY-MM-DD birthdate, bracketed the way the server does it:
 * the day you turn 6 you are a kid, 12 a younger teen, 16 an older teen,
 * 18 an adult. */
export function bracketForBirthdate(ymd: string, today = new Date()): AgeBracket | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(ymd);
  if (!m) return null;
  const by = Number(m[1]), bm = Number(m[2]), bd = Number(m[3]);
  let age = today.getFullYear() - by;
  const had = today.getMonth() + 1 > bm || (today.getMonth() + 1 === bm && today.getDate() >= bd);
  if (!had) age -= 1;
  if (age < 0) return null;
  if (age < 6) return "little";
  if (age < 12) return "kid";
  if (age < 16) return "younger_teen";
  if (age < 18) return "older_teen";
  return "adult";
}

/** POST /api/members — a child (or self-tracking adult) the hub manages. */
export interface NewMember {
  display_name: string;
  birthdate?: string | null;
  age_bracket?: AgeBracket;
  theme?: Theme | null;
}

export type MemberPatch = Partial<{
  display_name: string;
  birthdate: string | null;
  age_bracket: AgeBracket;
  theme: Theme | null;
  profile_id: string;
}>;

/** GET /api/me/today — the person's own day, for their own page. */
export interface MeToday {
  used_minutes: number;
  earned_minutes: number;
  limit_minutes: number | null;
  left_minutes: number | null;
  locked: boolean;
  devices: { name: string; status: DeviceStatus; locked: boolean }[];
  blocks: AppBlocks;
  bracket: AgeBracket;
  theme: Theme;
  pending_request: boolean;
  bedtime: Bedtime | null;
  windows: TimeWindow[];
}

/** One day of a person's own history (GET /api/me/history). */
export interface MeHistoryDay {
  day: string; // YYYY-MM-DD
  used_minutes: number;
  earned_minutes: number;
}

/** The /me page's week: what you did, and where today went. */
export interface MeHistory {
  days: MeHistoryDay[];
  today_by_device: { name: string; used_minutes: number }[];
}

// ---- Unlock codes (per device, owned by the server) ---------------------------
// The 6-digit code a parent types on a child's computer — to unlock the
// screen, reopen time, or `sudo` — is verified offline by the device, but the
// secret never leaves the server and the agent. The console shows the CURRENT
// code (a sensitive read: step-up gated), never the secret, never a QR.

/** GET /api/devices/:id/unlock-code */
export interface UnlockCode {
  code: string;
  /** until this code rolls over (the period is 30 s) */
  seconds_left: number;
  period: number;
  device_name: string;
}

/** POST /api/devices/:id/unlock-code/rotate — a new secret; the recovery
 * codes were keyed by the old one and are gone. */
export interface UnlockCodeRotated extends UnlockCode {
  recovery_codes_cleared: boolean;
}

/** POST /api/devices/:id/recovery-codes — eight one-time 8-digit codes,
 * returned exactly once, "1234 5678" formatted. */
export interface RecoveryCodes {
  codes: string[];
  generated_at: string;
}

/** GET /api/devices/:id/recovery-codes — how many are left, never which. */
export interface RecoveryCodesStatus {
  unused: number;
  total: number;
  generated_at: string | null;
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

export type SecondFactorMethod = "totp" | "email" | "telegram";

/** Error code the server returns from a mutation with no valid step-up grant. */
export const STEP_UP_REQUIRED = "step_up_required";

export interface TwoFactorStatus {
  /** An authenticator-app secret is enrolled and confirmed. */
  totp_enrolled: boolean;
  /** The account has an email a code can be sent to. */
  email_available: boolean;
  /** A Telegram chat is paired — one tap on the phone is a factor. */
  telegram_available?: boolean;
}

/** Pairing state of the account's Telegram companion. */
export interface TelegramStatus {
  /** A bot token is configured on the server at all. */
  configured: boolean;
  /** The bot's @username (for the t.me link), once known. */
  bot: string | null;
  paired: boolean;
  username: string | null;
  paired_at: string | null;
}

/** A fresh pairing code, shown once. */
export interface TelegramPairing {
  code: string;
  bot: string | null;
  deep_link: string | null;
  expires_in_minutes: number;
}

/** Returned by TOTP enrollment start — the secret is shown exactly once. */
export interface TotpEnrollment {
  /** base32 secret for manual entry. */
  secret: string;
  /** otpauth://totp/… — render as a QR for scanning into the app. */
  otpauth_uri: string;
}

/** A successful step-up: change mode is on until `expires_at`. */
export interface StepUpGrant {
  method: SecondFactorMethod;
  expires_at: string;
  /** the one allowed extension has been used */
  extended: boolean;
}

/** GET /api/auth/stepup — is change mode on for this session, and until when. */
export interface ChangeModeStatus {
  armed_until: string | null;
  extended: boolean;
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
