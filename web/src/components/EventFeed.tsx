import type { Event } from "../types";
import { StatusLed, severityTone } from "./StatusLed";
import { relTime } from "../lib/format";

const typeLabel: Record<Event["type"], string> = {
  heartbeat: "HEARTBEAT",
  tamper: "TAMPER",
  lock: "LOCK",
  unlock: "UNLOCK",
  policy_applied: "POLICY APPLIED",
  screen_time_exceeded: "TIME EXCEEDED",
  screen_time_earned: "TIME EARNED",
  streak: "STREAK",
  enrolled: "ENROLLED",
  discovery_result: "DISCOVERY",
};

function summarize(e: Event): string {
  const p = e.payload ?? {};
  switch (e.type) {
    case "tamper":
      return String(p.detail ?? p.kind ?? "tamper attempt detected");
    case "lock":
      return String(p.reason ?? "device locked");
    case "screen_time_earned":
      return `+${p.reward_minutes ?? "?"} min · ${p.task ?? "task"}`;
    case "screen_time_exceeded":
      return `balance ${p.balance_minutes ?? 0} min`;
    case "policy_applied":
      return `v${p.policy_version ?? "?"} · ${p.profile ?? ""}`;
    case "streak":
      return `${p.streak_days ?? 0}-day streak`;
    case "discovery_result":
      return `${p.hosts_found ?? "?"} hosts found`;
    case "enrolled":
      return String(p.hostname ?? "device enrolled");
    default:
      return "";
  }
}

interface Props {
  events: Event[];
  emptyLabel?: string;
}

// Audit log rows: severity LED + mono type + summary + relative time.
export function EventFeed({ events, emptyLabel = "NO EVENTS" }: Props) {
  if (!events.length) {
    return (
      <p className="label py-6 text-center" style={{ color: "var(--fg-faint)" }}>
        {emptyLabel}
      </p>
    );
  }
  return (
    <ul className="flex flex-col">
      {events.map((e) => (
        <li
          key={e.id}
          className="flex items-start gap-3 py-2.5 border-b last:border-b-0"
          style={{ borderColor: "var(--line)" }}
        >
          <span className="mt-1">
            <StatusLed tone={severityTone(e.severity)} pulse={e.severity === "critical"} />
          </span>
          <div className="flex-1 min-w-0">
            <div className="flex items-baseline gap-2 flex-wrap">
              <span
                className="dot text-[0.6875rem]"
                style={{
                  color:
                    e.severity === "critical" ? "var(--accent)" : "var(--fg)",
                }}
              >
                {typeLabel[e.type]}
              </span>
              <span className="text-xs truncate" style={{ color: "var(--fg-dim)" }}>
                {summarize(e)}
              </span>
            </div>
          </div>
          <time
            className="text-[0.625rem] tabular-nums flex-none pt-0.5"
            style={{ color: "var(--fg-faint)" }}
            dateTime={e.created_at}
          >
            {relTime(e.created_at)}
          </time>
        </li>
      ))}
    </ul>
  );
}
