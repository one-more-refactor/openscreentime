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

function sentence(e: Event): string {
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
    case "enforcement_degraded":
      return d ? `A rule couldn't be enforced: ${d}` : "A rule couldn't be enforced";
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

export function Moments({ events, max = 5 }: { events: Event[]; max?: number }) {
  const since = Date.now() - RECENT_MS;
  const moments = events
    .filter((e) => MOMENT_TYPES.has(e.type) && new Date(e.created_at).getTime() >= since)
    .slice(0, max);
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
