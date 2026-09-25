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

export function sentence(e: Event): string {
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
      if (isGapKind(kind)) return `The computer ${gapPhrase(kind)}`;
      return d ? `A rule couldn't be enforced: ${d}` : "A rule couldn't be enforced";
    }
    default:
      return String(e.type).replace(/_/g, " ");
  }
}

function tone(e: Event): "ok" | "warn" | "crit" {
  if (e.severity === "critical") return "crit";
  if (e.severity === "warn") return "warn";
  return "ok";
}

/** The story of the last two days — older moments are history, not news. */
const RECENT_MS = 48 * 3600_000;

/** The moments worth telling, newest first: recent, never machinery dressed
 * up as an attempt, and one line per kind of trouble — a check that failed
 * a hundred times is one moment, not a hundred. */
export function momentsOf(events: Event[], max = 5, now = Date.now()): Event[] {
  const since = now - RECENT_MS;
  const seen = new Set<string>();
  return events
    .filter((e) => MOMENT_TYPES.has(e.type) && new Date(e.created_at).getTime() >= since)
    .filter((e) => !isNotAnAttempt(e))
    .filter((e) => {
      if (e.type !== "tamper" && e.type !== "enforcement_degraded") return true;
      const key = `${e.type}:${String(e.payload?.kind ?? "")}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    })
    .slice(0, max);
}

export function Moments({ events, max = 5 }: { events: Event[]; max?: number }) {
  const moments = momentsOf(events, max);
  if (moments.length === 0) return null;
  return (
    <div className="card card-pad moments-card">
      <h3 className="wt-h">Moments</h3>
      <ul className="moments">
        {moments.map((e) => (
          <li key={e.id} className="moment" data-tone={tone(e)}>
            <span className="moment-dot" aria-hidden="true" />
            <span className="moment-text">{sentence(e)}</span>
            <span className="moment-when">{ago(e.created_at)}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
