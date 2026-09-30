// My rules must mean exactly what the agent enforces. The focus rows and the
// window rows are the policy crate's own vectors (policy/tests/schedule-
// vectors.json — `focus_blocking` and `validate_screen_time` read the same
// file), so the page can't promise a window the computer reads differently.
import { describe, expect, test } from "bun:test";
import vectors from "../../../policy/tests/schedule-vectors.json";
import { parseHm } from "./schedule";
import {
  describeDays,
  focusBlocking,
  focusEndsAt,
  focusProblem,
  nextFocusStart,
  normalizeSite,
  rulesProblem,
} from "./myRules";
import { deviceNow } from "./day";

describe("focus hours block the sites exactly when the agent does", () => {
  for (const v of vectors.focus) {
    const when = `${["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][v.at.day]} ${v.at.time}`;
    const hours = v.hours ? `${v.hours.start}–${v.hours.end} on ${v.hours.days.join(",")}` : "no hours (all day)";
    test(`${hours} at ${when}: ${v.blocking ? "blocked" : "open"}`, () => {
      const minute = parseHm(v.at.time)!;
      expect(focusBlocking({ sites: ["reddit.com"], hours: v.hours }, v.at.day, minute)).toBe(v.blocking);
    });
  }

  test("no sites blocks nothing, whatever the hours", () => {
    expect(focusBlocking({ sites: [], hours: null }, 1, 600)).toBe(false);
  });
});

describe("focus hours are saved only when they can mean something", () => {
  // Every allowed-hours window vector, as focus hours on weekdays.
  for (const w of vectors.windows) {
    test(`${w.start}–${w.end} is ${w.valid ? "fine" : "refused"}`, () => {
      expect(focusProblem({ days: [1, 2, 3, 4, 5], start: w.start, end: w.end }) === null).toBe(w.valid);
    });
  }

  test("any time — no hours — is fine: the sites are blocked all day", () => {
    expect(focusProblem(null)).toBeNull();
  });

  test("hours need a day", () => {
    expect(focusProblem({ days: [], start: "09:00", end: "12:00" })).toMatch(/day/);
  });

  test("a window to midnight, and one across it, are fine", () => {
    expect(focusProblem({ days: [0, 1, 2, 3, 4, 5, 6], start: "18:00", end: "00:00" })).toBeNull();
    expect(focusProblem({ days: [5], start: "22:00", end: "02:00" })).toBeNull();
  });
});

describe("the whole form", () => {
  const ok = { daily_limit_minutes: 180, focus_hours: { days: [1, 2, 3, 4, 5], start: "09:00", end: "12:00" }, sites: ["reddit.com"] };

  test("a sensible set of rules passes", () => {
    expect(rulesProblem(ok)).toBeNull();
    expect(rulesProblem({ daily_limit_minutes: 0, focus_hours: null, sites: [] })).toBeNull();
  });

  test("a day only has 24 hours", () => {
    expect(rulesProblem({ ...ok, daily_limit_minutes: 1441 })).toMatch(/24 hours/);
    expect(rulesProblem({ ...ok, daily_limit_minutes: -15 })).not.toBeNull();
  });

  test("an empty window or a non-site is refused, with a sentence", () => {
    expect(rulesProblem({ ...ok, focus_hours: { days: [1], start: "10:00", end: "10:00" } })).toMatch(/same/);
    expect(rulesProblem({ ...ok, sites: ["not a site"] })).toMatch(/isn't a site/);
  });
});

describe("sites, as the server stores them", () => {
  test("a pasted address becomes its site", () => {
    expect(normalizeSite("https://www.Reddit.com/r/all?x=1")).toEqual({ site: "www.reddit.com" });
    expect(normalizeSite("  news.ycombinator.com.  ")).toEqual({ site: "news.ycombinator.com" });
    expect(normalizeSite("example.org:8080")).toEqual({ site: "example.org" });
  });

  test("anything else says why", () => {
    expect("problem" in normalizeSite("")).toBe(true);
    expect("problem" in normalizeSite("reddit")).toBe(true);
    expect("problem" in normalizeSite("evil.com x")).toBe(true);
  });
});

test("days read like a week", () => {
  expect(describeDays([1, 2, 3, 4, 5])).toBe("Mon – Fri");
  expect(describeDays([0, 1, 2, 3, 4, 5, 6])).toBe("Every day");
  expect(describeDays([6, 0])).toBe("Weekends");
  expect(describeDays([1, 3, 5])).toBe("Mon, Wed, Fri");
  expect(describeDays([2, 3, 4])).toBe("Tue – Thu");
});

test("the end of focus is said the way it's set — midnight is midnight", () => {
  const mon1030 = { day: 1, minute: 10 * 60 + 30 }; // a Monday, on the computer's clock
  expect(focusEndsAt({ sites: ["x.com"], hours: { days: [1], start: "09:00", end: "12:00" } }, mon1030)).toBe("12:00");
  expect(focusEndsAt({ sites: ["x.com"], hours: { days: [1], start: "09:00", end: "00:00" } }, mon1030)).toBe("midnight");
  expect(focusEndsAt({ sites: ["x.com"], hours: { days: [2], start: "09:00", end: "12:00" } }, mon1030)).toBeNull();
});

// Acceptance, step 9: focus hours 00:00–02:00 every day, the computer (UTC)
// at 00:3x inside them with the site blocked — and the console, reading the
// browser's Berlin clock (02:3x), said "Next focus hours: Sat at 00:00. Until
// then the sites below are open." Focus hours are the computer's hours.
test("focus hours are read on the computer's clock, not the browser's", () => {
  const f = { sites: ["example.org"], hours: { days: [0, 1, 2, 3, 4, 5, 6], start: "00:00", end: "02:00" } };
  const at = new Date(Date.UTC(2026, 8, 25, 0, 37)); // Fri 00:37 UTC = 02:37 in Berlin
  expect(focusEndsAt(f, deviceNow(0, at))).toBe("02:00");
  // Berlin's clock said the opposite — "Next focus hours: Sat at 00:00".
  expect(focusEndsAt(f, deviceNow(2 * 3600, at))).toBeNull();
  expect(nextFocusStart(f, deviceNow(2 * 3600, at))).toEqual({ day: 6, start: "00:00", today: false });
});
