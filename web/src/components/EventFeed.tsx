import type { Event } from "../types";
import { StatusLed, severityTone } from "./StatusLed";
import { relTime } from "../lib/format";

// Recent activity is the last-resort detail — the place you look when the
// glanceable views didn't answer your question. So it reads like a sentence,
// not like a syslog: no ALL-CAPS mono, no jargon, and every row says what
// happened to whom rather than naming an event type.
const typeLabel: Record<Event["type"], string> = {
  heartbeat: "Checked in",
  tamper: "Tampering",
  lock: "Locked",
  unlock: "Unlocked",
  policy_applied: "Rules applied",
  screen_time_exceeded: "Time ran out",
  screen_time_earned: "Time earned",
  enrolled: "Set up",
  ssh: "Remote session",
  earn_request: "Asked for time",
  evasion: "Clock tampering",
  enforcement_degraded: "Not enforced",
  vpn_profile: "VPN profile",
  parent_code_ok: "Parent code accepted",
  parent_code_failed: "Wrong parent code",
  parent_code_backup_used: "Backup code used",
  app_blocked: "App stopped",
};

// Never render a blank row: an unmapped (e.g. future) type shows its raw name
// rather than disappearing — the highest-signal events must not be invisible.
function labelFor(type: Event["type"]): string {
  return typeLabel[type] ?? String(type).replace(/_/g, " ");
}

function summarize(e: Event): string {
  const p = e.payload ?? {};
  switch (e.type) {
    case "tamper":
    case "enforcement_degraded":
    case "evasion":
      // tamper_event() writes {kind, message}; older rows used {detail}.
      return String(p.message ?? p.detail ?? p.kind ?? "tamper attempt detected");
    case "lock":
      return String(p.reason ?? "device locked");
    case "screen_time_earned":
      return `+${p.reward_minutes ?? "?"} min · ${p.task ?? "task"}`;
    case "screen_time_exceeded":
      return `${p.balance_minutes ?? 0} min left`;
    case "policy_applied":
      return `${p.profile ?? "rules"} · v${p.policy_version ?? "?"}`;
    case "enrolled":
      return String(p.hostname ?? "device set up");
    default:
      return "";
  }
}

interface Props {
  events: Event[];
  emptyLabel?: string;
}

export function EventFeed({ events, emptyLabel = "Nothing has happened yet." }: Props) {
  if (!events.length) {
    return (
      <p className="py-6 text-center text-sm" style={{ color: "var(--fg-faint)" }}>
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
          <span className="mt-1.5">
            <StatusLed tone={severityTone(e.severity)} pulse={e.severity === "critical"} />
          </span>
          <div className="flex-1 min-w-0">
            <div className="flex items-baseline gap-2 flex-wrap">
              <span
                className="text-sm"
                style={{
                  color: e.severity === "critical" ? "var(--accent)" : "var(--fg)",
                  fontWeight: 500,
                }}
              >
                {labelFor(e.type)}
              </span>
              <span className="text-sm truncate" style={{ color: "var(--fg-dim)" }}>
                {summarize(e)}
              </span>
            </div>
          </div>
          <time
            className="text-xs tabular-nums flex-none pt-1"
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
