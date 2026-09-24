// ============================================================================
// Allowed hours and bedtime — what a window MEANS, in the editor's words.
//
// The meaning itself lives in one place, the policy crate's rules function
// (policy/src/rules.rs), which the agent enforces with and the server
// validates with. This file only mirrors the parts the editor needs to say
// things before saving, and is held to the same shared test vectors
// (policy/tests/schedule-vectors.json):
//
//   * a day with no window is "any time" — unrestricted;
//   * an end of 00:00 is midnight, and 00:00 – 00:00 is all day;
//   * an end before the start runs past midnight into the next day;
//   * a window whose start equals its end is empty — the server refuses it;
//   * a bedtime covering the whole day is refused too.
//
// The console never computes "time left" itself: the server sends it
// (`left_minutes`, and `rules` for when screens actually stop).
// ============================================================================

/** "HH:MM" → minutes after midnight, or null when unreadable. */
export function parseHm(s: string): number | null {
  const m = /^\s*(\d{1,2}):(\d{1,2})\s*$/.exec(s);
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2]);
  return h < 24 && min < 60 ? h * 60 + min : null;
}

/** start/end as a span, end in 1..1440 (00:00 = midnight); null if empty. */
function span(start: string, end: string): [number, number] | null {
  const s = parseHm(start);
  const e0 = parseHm(end);
  if (s === null || e0 === null) return null;
  const e = e0 === 0 ? 1440 : e0;
  return s === e ? null : [s, e];
}

/** Why an allowed-hours window can't be saved, or null if it's fine. */
export function windowProblem(start: string, end: string): string | null {
  if (parseHm(start) === null || parseHm(end) === null) return "Times look like 07:30.";
  if (span(start, end) === null) {
    return "Start and end are the same, so no time is allowed. Pick real hours, or choose Any time.";
  }
  return null;
}

/** Why a bedtime can't be saved, or null if it's fine. */
export function bedtimeProblem(start: string, end: string): string | null {
  if (parseHm(start) === null || parseHm(end) === null) return "Times look like 20:30.";
  const sp = span(start, end);
  if (sp === null || (sp[0] === 0 && sp[1] === 1440)) {
    return "That bedtime covers the whole day. Pick when it starts and when it ends.";
  }
  return null;
}

/** Plain words for a window: "18:00 – midnight", "20:00 – 01:00 (next day)". */
export function describeWindow(start: string, end: string): string {
  const sp = span(start, end);
  if (sp === null) return `${start} – ${end}`;
  if (sp[0] === 0 && sp[1] === 1440) return "all day";
  if (sp[1] === 1440) return `${start} – midnight`;
  if (sp[1] < sp[0]) return `${start} – ${end} (next day)`;
  return `${start} – ${end}`;
}
