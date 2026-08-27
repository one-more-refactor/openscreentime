// ============================================================================
// Moments — the day's story, not the log (CONTRACT-0.6 §3).
//
// A parent's page shows the handful of moments that mattered: a stop, a
// pause, time earned, a tamper. Sentences with a tone dot and a time — never
// a feed, never at the bottom as a syslog. On a healthy day this renders
// NOTHING, which is the whole point: the raw event feed still exists for the
// operator, with the machinery (Devices).
// ============================================================================
import type { Event } from "../types";
import { relTime } from "../lib/format";

/** The types a parent should ever see as a moment; everything else is
 * machinery (heartbeats, policy versions, VPN profiles → Devices). */
const MOMENT_TYPES = new Set<Event["type"]>([
  "lock",
  "unlock",
  "screen_time_exceeded",
  "screen_time_earned",
  "tamper",
  "evasion",
  "enforcement_degraded",
]);

function sentence(e: Event): string {
  const p = e.payload ?? {};
  switch (e.type) {
    case "lock":
      return "Screen paused";
    case "unlock":
      return "Screen resumed";
    case "screen_time_exceeded":
      return "Time ran out for the day";
    case "screen_time_earned":
      return `Earned ${p.reward_minutes ?? "?"} minutes back (${p.task ?? "a task"})`;
    case "tamper":
      return `Tampering: ${p.message ?? p.detail ?? p.kind ?? "something poked the protections"}`;
    case "evasion":
      return `Clock games: ${p.message ?? p.detail ?? "the clock was moved"}`;
    case "enforcement_degraded":
      return `The lock isn't biting: ${p.message ?? p.detail ?? "a protection failed silently"}`;
    default:
      return String(e.type).replace(/_/g, " ");
  }
}

function tone(e: Event): "ok" | "warn" | "crit" {
  if (e.severity === "critical") return "crit";
  if (e.severity === "warn") return "warn";
  return "ok";
}

export function Moments({ events, max = 6 }: { events: Event[]; max?: number }) {
  const moments = events.filter((e) => MOMENT_TYPES.has(e.type)).slice(0, max);
  if (moments.length === 0) return null;
  return (
    <section className="ch-section">
      <h2 className="ch-h2">Moments</h2>
      <ul className="moments">
        {moments.map((e) => (
          <li key={e.id} className="moment" data-tone={tone(e)}>
            <span className="moment-dot" aria-hidden="true" />
            <span className="moment-text">{sentence(e)}</span>
            <span className="moment-when">{relTime(e.created_at)}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
