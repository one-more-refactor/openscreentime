// ============================================================================
// The day, in sentences — what the rules verdict the server computed (the
// agent's own rules function, on the computer's clock) means for a person.
// Shared by the person page and the person's own page so both say the same
// thing about the same moment, only the pronoun differs.
//
// Everything here is on the COMPUTER's clock, not this browser's: bedtime at
// 20:00 is 20:00 where the computer is, "tomorrow" is the computer's
// tomorrow, the week is the computer's week. A parent reading the console in
// another time zone (or a server-side clock in UTC) must not move a child's
// bedtime or their focus hours. The server sends the times with the
// computer's offset, and the offset itself (`utc_offset_secs`).
// ============================================================================
import type { RulesVerdict } from "../types";

/** A moment read on a wall clock: the date, the weekday, the minute of the day. */
export interface WallTime {
  /** YYYY-MM-DD */
  date: string;
  /** 0 = Sunday … 6 = Saturday */
  day: number;
  /** minutes after midnight */
  minute: number;
  /** "HH:MM" */
  hm: string;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** The UTC offset an RFC 3339 time was written in, in seconds (null if none). */
export function isoOffsetSecs(iso: string): number | null {
  const m = /([+-])(\d{2}):?(\d{2})$/.exec(iso.trim());
  if (m) return (m[1] === "-" ? -1 : 1) * (Number(m[2]) * 3600 + Number(m[3]) * 60);
  return /z$/i.test(iso.trim()) ? 0 : null;
}

/** `ms` read on the wall clock of a computer `offsetSecs` east of UTC. A
 * computer that never said is on UTC — never this browser's clock: the
 * server files its day as UTC's (`COALESCE(utc_offset_secs, 0)`), and a
 * browser in Berlin reading its own date put today's minutes on two days of
 * "My week" (acceptance round 4). */
export function wallTime(ms: number, offsetSecs: number | null | undefined): WallTime {
  const d = new Date(ms + (offsetSecs ?? 0) * 1000);
  return {
    date: `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`,
    day: d.getUTCDay(),
    minute: d.getUTCHours() * 60 + d.getUTCMinutes(),
    hm: `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`,
  };
}

/** Right now on the computer's clock. */
export function deviceNow(offsetSecs: number | null | undefined, now = new Date()): WallTime {
  return wallTime(now.getTime(), offsetSecs);
}

/** The date `n` days before `date` (YYYY-MM-DD), with its weekday. */
export function daysBefore(date: string, n: number): { date: string; day: number } {
  const [y, m, d] = date.split("-").map(Number);
  const t = new Date(Date.UTC(y, m - 1, d) - n * 86_400_000);
  return {
    date: `${t.getUTCFullYear()}-${pad(t.getUTCMonth() + 1)}-${pad(t.getUTCDate())}`,
    day: t.getUTCDay(),
  };
}

const dayNumber = (date: string) => {
  const [y, m, d] = date.split("-").map(Number);
  return Date.UTC(y, m - 1, d) / 86_400_000;
};

const WEEKDAY = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** "20:00", or "tomorrow at 07:00", or "Sat at 09:00" — 24-hour, like every
 * time the console shows, on the clock of the computer that sent it. */
export function whenLabel(iso: string, now = new Date()): string {
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return "";
  const off = isoOffsetSecs(iso);
  const at = wallTime(ms, off);
  const today = wallTime(now.getTime(), off);
  const diff = dayNumber(at.date) - dayNumber(today.date);
  if (diff === 0) return at.hm;
  if (diff === 1) return `tomorrow at ${at.hm}`;
  return `${WEEKDAY[at.day]} at ${at.hm}`;
}

/** When a parent's override (an unlock code, a grant) is what keeps the screen
 * on — the stop is when it ends — the time it ends, as `whenLabel` says it. */
export function unlockedUntil(r: RulesVerdict | null | undefined, now = new Date()): string | null {
  if (!r?.allowed || !r.override_until || !r.stop_at) return null;
  const ov = Date.parse(r.override_until);
  const stop = Date.parse(r.stop_at);
  if (Number.isNaN(ov) || Number.isNaN(stop) || ov <= now.getTime()) return null;
  return Math.abs(stop - ov) < 60_000 ? whenLabel(r.override_until, now) : null;
}

/** How close a stop has to be for a page to follow each minute closely. */
export const NEAR_STOP_MS = 5 * 60_000;

/** A stop lands within five minutes (or just landed): the page showing it
 * should refresh often, so "1 min left" becomes "Time's up" when the
 * computer's does, not half a minute later. */
export function stopIsNear(r: RulesVerdict | null | undefined, now = Date.now()): boolean {
  if (!r?.allowed || !r.stop_at) return false;
  const at = Date.parse(r.stop_at);
  if (Number.isNaN(at)) return false;
  return at - now <= NEAR_STOP_MS && now - at <= 60_000;
}

type Who = "they" | "you";

/** One sentence about what stops the screen next, or null when nothing does. */
export function stopSentence(r: RulesVerdict | null | undefined, who: Who, now = new Date()): string | null {
  if (!r) return null;
  const their = who === "they" ? "their" : "your";
  if (!r.allowed) {
    const until = r.resume_at ? whenLabel(r.resume_at, now) : null;
    switch (r.reason) {
      case "limit":
        return "Time's up for today. It starts again tomorrow.";
      case "bedtime":
        return until ? `Bedtime until ${until}.` : "It's bedtime.";
      case "outside_hours":
        return until ? `Outside ${their} allowed hours until ${until}.` : `Outside ${their} allowed hours.`;
      case "paused":
        return who === "they" ? "Paused." : "Your computer is paused.";
      default:
        return null;
    }
  }
  const unlocked = unlockedUntil(r, now);
  if (unlocked) return `Unlocked until ${unlocked}.`;
  if (!r.stop_at) return null;
  // "at 20:00", or "tomorrow at 00:40" — never "at tomorrow at".
  const label = whenLabel(r.stop_at, now);
  const at = /^\d/.test(label) ? `at ${label}` : label;
  switch (r.reason) {
    case "bedtime":
      return `Screens stop ${at} for bedtime.`;
    case "outside_hours":
      return `Screens stop ${at}, when ${their} allowed hours end.`;
    case "limit":
      return `If ${who} keep going, ${their} time runs out ${at}.`;
    default:
      return null;
  }
}
