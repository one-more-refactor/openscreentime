// ============================================================================
// Moments — the day's story, not the log (CONTRACT-0.6 §3).
//
// A person page shows the handful of moments that mattered: a stop, a pause,
// time given, an unlock code, something poking at the rules. Sentences with a
// tone dot and a time — never a feed. On a healthy day this renders NOTHING,
// which is the whole point.
//
// Whose moment it is: an event the computer filed under a login is that
// person's; one with no login is the computer's own (a pause, a gap, a lost
// connection) and is told where that computer is told — the Computers page,
// and the page of the person it belongs to (`momentsFor`).
// ============================================================================
import type { Event } from "../types";
import { ago } from "../lib/format";
import { gapArea, gapPhrase, isNotAnAttempt } from "../lib/degraded";

/** The types a parent should ever see as a moment; everything else is
 * machinery. */
const MOMENT_TYPES = new Set<Event["type"]>([
  "lock",
  "unlock",
  "screen_time_exceeded",
  "screen_time_earned",
  "tamper",
  "evasion",
  "enforcement_degraded",
]);

/** An unlock code, or a recovery code, typed at the lock. The same check in
 * `sudo` or `ost unlock` is not a moment of the person's day. */
function codeAtTheLock(e: Event): boolean {
  return (e.type === "parent_code_ok" || e.type === "parent_code_backup_used") && e.payload?.via === "lock_screen";
}

/** The computer was out of touch with the server past its grace (it kept
 * its rules): machinery, not an attempt — but worth a line. */
function outOfTouch(e: Event): boolean {
  return e.type === "tamper" && e.payload?.kind === "network_offline";
}

function isMoment(e: Event): boolean {
  if (codeAtTheLock(e) || outOfTouch(e)) return true;
  return MOMENT_TYPES.has(e.type) && !isNotAnAttempt(e);
}

function detail(p: Record<string, unknown>): string | null {
  const v = p.message ?? p.detail ?? p.kind;
  return typeof v === "string" && v ? v : null;
}

/** A standing gap the agent reports (client/src/enforce): something this
 * computer can't do, not something anyone did. */
function isGapKind(kind: string): boolean {
  return (
    /^(dns|firewall|vpn)_/.test(kind) || kind === "screen_time_no_freezer" || kind === "network_apply_failed"
  );
}

/** What each computer says it can't do right now (its `state` frame's gaps),
 * by device id — only for computers that are online and have said so. */
export type StandingGaps = Record<string, string[]>;

export function standingGaps(
  devices: { id: string; status: string; last_state?: { gaps?: string[] | null } | null }[],
): StandingGaps {
  const out: StandingGaps = {};
  for (const d of devices) {
    if (d.status === "online" && d.last_state) out[d.id] = d.last_state.gaps ?? [];
  }
  return out;
}

/** A gap the computer has since reported gone: it said it couldn't do
 * something, and now — online, reporting — it no longer says so. */
export function recovered(e: Event, standing?: StandingGaps): boolean {
  if (e.type !== "enforcement_degraded" || !e.device_id || !standing) return false;
  const kind = typeof e.payload?.kind === "string" ? e.payload.kind : "";
  const now = standing[e.device_id];
  if (!isGapKind(kind) || !now) return false;
  return !now.some((g) => gapArea(g) === gapArea(kind));
}

/** One moment in words. `computer`: the computer's name, when the person has
 * more than one and "The computer" wouldn't say which. `standing`: what the
 * computers say now, so a gap that went away reads as over. */
export function sayMoment(e: Event, computer?: string, standing?: StandingGaps): string {
  const p = e.payload ?? {};
  const d = detail(p);
  const it = computer ?? "The computer";
  switch (e.type) {
    case "lock":
      return "Paused";
    case "unlock":
      return "Resumed";
    case "screen_time_exceeded":
      return "Time's up for the day";
    case "screen_time_earned": {
      const minutes = p.reward_minutes ?? p.minutes;
      if (p.via === "self") return `Took ${minutes ?? 15} more minutes`;
      return `Got ${minutes ?? "some"} more minutes${p.task ? ` (${String(p.task)})` : ""}`;
    }
    case "parent_code_ok":
      return "Unlocked with the unlock code";
    case "parent_code_backup_used":
      return "Unlocked with a recovery code";
    case "tamper":
      if (outOfTouch(e)) return `${it} was out of touch with the server for a while — it kept its rules`;
      return d ? `Something tried to get around the rules: ${d}` : "Something tried to get around the rules";
    case "evasion":
      return d ? `The clock was changed: ${d}` : "The clock was changed";
    case "enforcement_degraded": {
      const kind = typeof p.kind === "string" ? p.kind : "";
      if (isGapKind(kind)) {
        return recovered(e, standing)
          ? `${it} ${gapPhrase(kind).replace(/^can't/, "couldn't")} for a while — it's working again`
          : `${it} ${gapPhrase(kind)}`;
      }
      return d ? `A rule couldn't be enforced: ${d}` : "A rule couldn't be enforced";
    }
    default:
      return String(e.type).replace(/_/g, " ");
  }
}

/** A moment in words, the computer unnamed. */
export function sentence(e: Event): string {
  return sayMoment(e);
}

export function tone(e: Event, standing?: StandingGaps): "ok" | "warn" | "crit" {
  if (recovered(e, standing)) return "ok";
  if (e.severity === "critical") return "crit";
  if (e.severity === "warn") return "warn";
  return "ok";
}

/** The story of the last two days — older moments are history, not news. */
const RECENT_MS = 48 * 3600_000;

/** The same moment told twice within this long is one moment: the console's
 * own "Resumed" and the computer's confirmation of it, or two "can't filter
 * websites" gaps an agent reports at every start. */
const SAME_MOMENT_MS = 10 * 60_000;

/** The moments worth telling, newest first: recent, never machinery dressed
 * up as an attempt, and each thing said once —
 *
 *  - trouble (a gap, an attempt) once per computer and sentence while it's
 *    news: a check that failed a hundred times is one moment, and two gap
 *    kinds that both mean "can't filter websites" are one line;
 *  - anything else once per computer and sentence within a few minutes:
 *    "Resumed" from the console and "Resumed" from the computer are one. */
export function momentsOf(events: Event[], max = 5, now = Date.now(), standing?: StandingGaps): Event[] {
  const since = now - RECENT_MS;
  const told = new Set<string>();
  const lastTold = new Map<string, number>();
  const out: Event[] = [];
  const newestFirst = [...events].sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
  for (const e of newestFirst) {
    const t = Date.parse(e.created_at);
    if (!isMoment(e) || !(t >= since)) continue;
    const key = `${e.device_id ?? ""}|${sayMoment(e, undefined, standing)}`;
    if (e.type === "tamper" || e.type === "enforcement_degraded") {
      if (told.has(key)) continue;
      told.add(key);
    } else {
      const last = lastTold.get(key);
      if (last !== undefined && last - t <= SAME_MOMENT_MS) continue;
      lastTold.set(key, t);
    }
    out.push(e);
    if (out.length >= max) break;
  }
  return out;
}

/** A person's moments out of their computers' events: those filed under one
 * of their logins, and — the computer's own, with no login — only from a
 * computer that is theirs. Someone else's snooze on a shared computer, or a
 * gap on a computer they merely use, is not their story. */
export function momentsFor(
  events: Event[],
  person: { logins: Set<string>; theirs: (deviceId: string) => boolean },
): Event[] {
  return events.filter((e) =>
    e.device_user_id ? person.logins.has(e.device_user_id) : !!e.device_id && person.theirs(e.device_id),
  );
}

export function Moments({
  events,
  max = 5,
  computers,
  standing,
  bare = false,
}: {
  events: Event[];
  max?: number;
  /** device id → name, when the person has more than one computer */
  computers?: Record<string, string>;
  /** what the computers say they can't do now (see `standingGaps`) */
  standing?: StandingGaps;
  /** inside a card already (a computer's): no card of its own */
  bare?: boolean;
}) {
  const moments = momentsOf(events, max, Date.now(), standing);
  if (moments.length === 0) return null;
  return (
    <div className={bare ? "moments-bare" : "card card-pad moments-card"}>
      <h3 className="wt-h">Moments</h3>
      <ul className="moments">
        {moments.map((e) => (
          <li key={e.id} className="moment" data-tone={tone(e, standing)}>
            <span className="moment-dot" aria-hidden="true" />
            <span className="moment-text">
              {sayMoment(e, e.device_id ? computers?.[e.device_id] : undefined, standing)}
            </span>
            <span className="moment-when">{ago(e.created_at)}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
