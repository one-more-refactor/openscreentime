// ============================================================================
// A computer that can't do everything its rules ask, said honestly.
//
// The agent reports what is really in force in its `state` frame
// (`last_state.gaps`, kinds from client/src/enforce): a stock Linux desktop
// without dnsmasq or nftables still enforces screen time, but filters no
// websites. The console must say so — never "every computer is doing what it
// should" — and must never call a missing package someone trying to get
// around the rules.
// ============================================================================
import type { Device, Event } from "../types";

/** What a gap means, as the end of "Mia's computer …". */
export function gapPhrase(kind: string): string {
  if (kind === "screen_time_no_freezer") return "can't stop the screen when time is up";
  if (kind.startsWith("dns_")) return "can't filter websites";
  if (kind.startsWith("firewall_")) return "can't apply its firewall rules";
  if (kind.startsWith("vpn_")) return "can't start its VPN";
  return "can't apply all of its rules";
}

/** A missing piece that re-running the install command brings. */
function fixedByReinstall(kind: string): boolean {
  return kind === "dns_resolver_missing" || kind === "firewall_not_installed";
}

/** Online computers whose rules aren't all in force right now. An offline
 * one says what it said last — that's "offline", shown elsewhere. */
export function degradedDevices(devices: Device[]): Device[] {
  return devices.filter((d) => d.status === "online" && (d.last_state?.gaps?.length ?? 0) > 0);
}

/** "can't filter websites, apply its firewall rules or start its VPN". */
function joinOr(parts: string[]): string {
  const [first, ...rest] = parts;
  if (!first) return "";
  const tail = rest.map((p) => p.replace(/^can't /, ""));
  if (tail.length === 0) return first;
  const last = tail.pop();
  return [first, ...tail].join(", ") + ` or ${last}`;
}

/** One device, in plain words: what it can't do, whether screen time still
 * works, and what fixes it. */
export function degradedSentence(d: Device): string {
  const gaps = d.last_state?.gaps ?? [];
  const phrases = [...new Set(gaps.map(gapPhrase))];
  let s = `${d.name} ${joinOr(phrases)}.`;
  if (!gaps.includes("screen_time_no_freezer")) s += " Screen time still works there.";
  if (gaps.some(fixedByReinstall)) s += " Run the install command on it again to add what's missing.";
  return s;
}

/** The whole fleet in one line, or null when every rule is in force. */
export function degradedSummary(devices: Device[]): string | null {
  const bad = degradedDevices(devices);
  if (bad.length === 0) return null;
  if (bad.length === 1) return degradedSentence(bad[0]);
  return `${bad.length} computers can't apply all of their rules. Computers says what each one is missing.`;
}

/** Tamper kinds that are machinery — a check that couldn't run, a network
 * that dropped, a level that was capped — not someone trying anything.
 * Older agents sent these as `tamper`; they never read as an accusation. */
const NOT_AN_ATTEMPT = new Set([
  "nft_probe_failed",
  "nm_disconnect",
  "resolv_conf_reassert_failed",
  "network_offline",
  "network_online",
  "dns_filter_restored",
  "tamper_level_capped",
  "boot_guidance",
  "agent_updated",
  "agent_update_rolled_back",
  "lock_screen_unavailable",
  "lock_without_offline_credential",
  "offline_lockdown_no_credential",
  "offline_hard_lockdown_lifted",
  "parent_pin_override",
]);

export function isNotAnAttempt(e: Event): boolean {
  if (e.type !== "tamper") return false;
  const kind = e.payload?.kind;
  return typeof kind === "string" && (NOT_AN_ATTEMPT.has(kind) || kind.startsWith("dns_"));
}
