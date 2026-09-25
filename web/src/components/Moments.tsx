// ============================================================================
// Moments — the day's story, not the log (CONTRACT-0.6 §3).
//
// A person page shows the handful of moments that mattered: a stop, a pause,
// time given, something poking at the rules. Sentences with a tone dot and a
// time — never a feed. On a healthy day this renders NOTHING, which is the
// whole point.
// ============================================================================
import type { Event } from "../types";
import { ago } from "../lib/format";
import { gapPhrase, isNotAnAttempt } from "../lib/degraded";

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

/** One moment in words. `computer`: the computer's name, when the person has
 * more than one and "The computer" wouldn't say which. */
export function sayMoment(e: Event, computer?: string): string {
  const p = e.payload ?? {};
  const d = detail(p);
  switch (e.type) {
    case "lock":
      return "Paused";
    case "unlock":
      return "Resumed";
    case "screen_time_exceeded":
      return "Time's up for the day";
    case "screen_time_earned":
      return `Got ${p.reward_minutes ?? "some"} more minutes${p.task ? ` (${String(p.task)})` : ""}`;
    case "tamper":
      return d ? `Something tried to get around the rules: ${d}` : "Something tried to get around the rules";
    case "evasion":
      return d ? `The clock was changed: ${d}` : "The clock was changed";
    case "enforcement_degraded": {
      const kind = typeof p.kind === "string" ? p.kind : "";
      if (isGapKind(kind)) return `${computer ?? "The computer"} ${gapPhrase(kind)}`;
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

function tone(e: Event): "ok" | "warn" | "crit" {
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
export function momentsOf(events: Event[], max = 5, now = Date.now()): Event[] {
  const since = now - RECENT_MS;
  const told = new Set<string>();
  const lastTold = new Map<string, number>();
  const out: Event[] = [];
  const newestFirst = [...events].sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
  for (const e of newestFirst) {
    const t = Date.parse(e.created_at);
    if (!MOMENT_TYPES.has(e.type) || !(t >= since) || isNotAnAttempt(e)) continue;
    const key = `${e.device_id ?? ""}|${sentence(e)}`;
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

export function Moments({
  events,
  max = 5,
  computers,
}: {
  events: Event[];
  max?: number;
  /** device id → name, when the person has more than one computer */
  computers?: Record<string, string>;
}) {
  const moments = momentsOf(events, max);
  if (moments.length === 0) return null;
  return (
    <div className="card card-pad moments-card">
      <h3 className="wt-h">Moments</h3>
      <ul className="moments">
        {moments.map((e) => (
          <li key={e.id} className="moment" data-tone={tone(e)}>
            <span className="moment-dot" aria-hidden="true" />
            <span className="moment-text">{sayMoment(e, e.device_id ? computers?.[e.device_id] : undefined)}</span>
            <span className="moment-when">{ago(e.created_at)}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
