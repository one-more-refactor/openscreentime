// ============================================================================
// My rules — what a self-managed person sets for themselves (the hub for
// their own computer, or an adult): a daily limit, focus hours, and the sites
// they block for themselves. Stored as their own policy and enforced by the
// agent like any other; the meaning of a window is the policy crate's
// (policy/src/rules.rs `focus_blocking`), held to the shared vectors in
// policy/tests/schedule-vectors.json:
//
//   * no sites → nothing is blocked, whatever the hours say;
//   * no focus hours → the sites are blocked all day, every day;
//   * with hours → only inside them: an end of 00:00 is midnight, 00:00 –
//     00:00 is all day, an end before the start runs past midnight into the
//     next day (the tail belongs to the day it started on).
// ============================================================================
import type { MyRules, TimeWindow } from "../types";
import { span, windowProblem } from "./schedule";

export const DAY_LETTERS = ["S", "M", "T", "W", "T", "F", "S"];
export const DAY_SHORT = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
/** Monday first, the way a week reads. */
export const WEEK_ORDER = [1, 2, 3, 4, 5, 6, 0];

/** "Mon – Fri", "Every day", "Weekends", "Mon, Wed, Fri". */
export function describeDays(days: number[]): string {
  const set = new Set(days.filter((d) => d >= 0 && d <= 6));
  if (set.size === 7) return "Every day";
  if (set.size === 0) return "No days";
  const key = [...set].sort().join("");
  if (key === "12345") return "Mon – Fri";
  if (key === "06") return "Weekends";
  // A run inside Monday-first order reads as a range.
  const ordered = WEEK_ORDER.filter((d) => set.has(d));
  const idx = ordered.map((d) => WEEK_ORDER.indexOf(d));
  const run = idx.every((v, i) => i === 0 || v === idx[i - 1] + 1);
  if (run && ordered.length >= 3) return `${DAY_SHORT[ordered[0]]} – ${DAY_SHORT[ordered[ordered.length - 1]]}`;
  return ordered.map((d) => DAY_SHORT[d]).join(", ");
}

/** A typed site as the server stores it — "https://www.Reddit.com/r/x" →
 * "www.reddit.com" — or why it can't be one. Mirrors the server's check. */
export function normalizeSite(raw: string): { site: string } | { problem: string } {
  const d = raw
    .trim()
    .toLowerCase()
    .replace(/^[a-z][a-z0-9+.-]*:\/\//, "")
    .replace(/[/?#].*$/, "")
    .replace(/:\d+$/, "")
    .replace(/^\.+|\.+$/g, "");
  if (!d) return { problem: "Type a site, like reddit.com." };
  if (d.length > 253 || !d.includes(".") || !/^[a-z0-9._-]+$/.test(d)) {
    return { problem: `${raw.trim()} isn't a site name — try something like reddit.com.` };
  }
  return { site: d };
}

/** Why focus hours can't be saved, or null. `null` hours are fine: all day. */
export function focusProblem(hours: TimeWindow | null): string | null {
  if (hours === null) return null;
  if (hours.days.length === 0) return "Pick at least one day.";
  return windowProblem(hours.start, hours.end);
}

/** Why a set of rules can't be saved, or null — the server says the same. */
export function rulesProblem(r: MyRules): string | null {
  if (!Number.isInteger(r.daily_limit_minutes) || r.daily_limit_minutes < 0 || r.daily_limit_minutes > 1440) {
    return "A day only has 24 hours.";
  }
  const f = focusProblem(r.focus_hours);
  if (f) return f;
  if (r.sites.length > 200) return "That's a lot of sites — 200 at most.";
  for (const s of r.sites) {
    const n = normalizeSite(s);
    if ("problem" in n) return n.problem;
  }
  return null;
}

/** Is the self-block on at this local weekday (0 = Sunday) and minute? */
export function focusBlocking(
  f: { sites: string[]; hours: TimeWindow | null },
  day: number,
  minute: number,
): boolean {
  if (f.sites.length === 0) return false;
  if (!f.hours) return true;
  const sp = span(f.hours.start, f.hours.end);
  if (!sp) return true; // unreadable hours never unblock what was asked for
  const [s, e] = sp;
  return f.hours.days.some((d) => {
    if (d < 0 || d > 6) return false;
    if (s < e) return day === d && minute >= s && minute < e;
    return (day === d && minute >= s) || (day === (d + 1) % 7 && minute < e);
  });
}

/** When the focus stretch in progress ends, "HH:MM" ("midnight" for 24:00),
 * or null when nothing is blocking or it never ends (no hours). */
export function focusEndsAt(f: { sites: string[]; hours: TimeWindow | null }, now: Date): string | null {
  if (!f.hours || !focusBlocking(f, now.getDay(), now.getHours() * 60 + now.getMinutes())) return null;
  const sp = span(f.hours.start, f.hours.end);
  if (!sp) return null;
  return sp[1] === 1440 ? "midnight" : f.hours.end;
}

/** When the next focus stretch starts, as { day, start } — for "focus hours
 * start at 9:00". Looks a week ahead; null with no hours or no sites. */
export function nextFocusStart(
  f: { sites: string[]; hours: TimeWindow | null },
  now: Date,
): { day: number; start: string; today: boolean } | null {
  if (!f.hours || f.sites.length === 0) return null;
  const sp = span(f.hours.start, f.hours.end);
  if (!sp) return null;
  const nowMin = now.getHours() * 60 + now.getMinutes();
  for (let i = 0; i < 8; i++) {
    const day = (now.getDay() + i) % 7;
    if (!f.hours.days.includes(day)) continue;
    if (i === 0 && sp[0] <= nowMin) continue;
    return { day, start: f.hours.start, today: i === 0 };
  }
  return null;
}
