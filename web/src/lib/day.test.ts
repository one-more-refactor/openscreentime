// The day is the COMPUTER's day. These hold the console to the computer's
// clock (a parent in Berlin looking at a laptop on UTC must not see its
// bedtime move), and to what the computer says about an override.
import { describe, expect, test } from "bun:test";
import { daysBefore, deviceNow, isoOffsetSecs, stopIsNear, stopSentence, unlockedUntil, whenLabel } from "./day";
import type { RulesVerdict } from "../types";

// Friday 2026-09-25, 23:30 UTC — already Saturday 01:30 in Berlin.
const now = new Date(Date.UTC(2026, 8, 25, 23, 30));

describe("the computer's clock", () => {
  test("an RFC 3339 time says its own offset", () => {
    expect(isoOffsetSecs("2026-09-25T23:30:00+02:00")).toBe(7200);
    expect(isoOffsetSecs("2026-09-25T23:30:00-04:30")).toBe(-16200);
    expect(isoOffsetSecs("2026-09-25T23:30:00Z")).toBe(0);
    expect(isoOffsetSecs("2026-09-25T23:30:00")).toBeNull();
  });

  test("now, where the computer is", () => {
    expect(deviceNow(0, now)).toEqual({ date: "2026-09-25", day: 5, minute: 23 * 60 + 30, hm: "23:30" });
    expect(deviceNow(7200, now)).toEqual({ date: "2026-09-26", day: 6, minute: 90, hm: "01:30" });
  });

  test("a week back from the computer's today", () => {
    expect(daysBefore("2026-09-25", 0)).toEqual({ date: "2026-09-25", day: 5 });
    expect(daysBefore("2026-09-25", 6)).toEqual({ date: "2026-09-19", day: 6 });
    expect(daysBefore("2026-03-01", 1)).toEqual({ date: "2026-02-28", day: 6 });
  });

  test("a stop is said on the computer's clock, whatever the browser's", () => {
    // Bedtime at the computer's 23:45 is "23:45" — not Berlin's "tomorrow at 01:45".
    expect(whenLabel("2026-09-25T23:45:00+00:00", now)).toBe("23:45");
    expect(whenLabel("2026-09-26T07:00:00+00:00", now)).toBe("tomorrow at 07:00");
    expect(whenLabel("2026-09-28T07:00:00+00:00", now)).toBe("Mon at 07:00");
    // A computer in Berlin: its tomorrow began half an hour ago.
    expect(whenLabel("2026-09-26T07:00:00+02:00", now)).toBe("07:00");
    expect(whenLabel("nonsense", now)).toBe("");
  });
});

describe("an override, said plainly", () => {
  const unlocked: RulesVerdict = {
    allowed: true,
    reason: "limit",
    minutes_left: 29,
    stop_at: "2026-09-25T23:59:00+00:00",
    resume_at: null,
    override_until: "2026-09-25T23:59:00+00:00",
    utc_offset_secs: 0,
  };

  // Acceptance, step 5: after the unlock code the console said "time's up" while
  // she was unlocked for 30 minutes.
  test("an unlock code's time is 'unlocked until', not time's up", () => {
    expect(unlockedUntil(unlocked, now)).toBe("23:59");
    expect(stopSentence(unlocked, "they", now)).toBe("Unlocked until 23:59.");
  });

  test("a grant with budget to spare is just more time", () => {
    const more = { ...unlocked, stop_at: "2026-09-26T00:40:00+00:00" };
    expect(unlockedUntil(more, now)).toBeNull();
    expect(stopSentence(more, "they", now)).toBe("If they keep going, their time runs out tomorrow at 00:40.");
    expect(stopSentence({ ...more, stop_at: "2026-09-25T23:50:00+00:00" }, "you", now)).toBe(
      "If you keep going, your time runs out at 23:50.",
    );
  });

  test("an override that ended, or a stop in force, says nothing about it", () => {
    expect(unlockedUntil({ ...unlocked, override_until: "2026-09-25T23:00:00+00:00" }, now)).toBeNull();
    expect(unlockedUntil({ ...unlocked, allowed: false }, now)).toBeNull();
    expect(unlockedUntil({ ...unlocked, override_until: null }, now)).toBeNull();
  });
});

describe("a stop close enough to follow closely", () => {
  const at = (iso: string | null, allowed = true): RulesVerdict => ({
    allowed,
    reason: "limit",
    minutes_left: 1,
    stop_at: iso,
    resume_at: null,
  });
  const t = now.getTime();
  test("within five minutes, or just landed", () => {
    expect(stopIsNear(at("2026-09-25T23:34:00+00:00"), t)).toBe(true);
    expect(stopIsNear(at("2026-09-25T23:35:00+00:00"), t)).toBe(true);
    expect(stopIsNear(at("2026-09-25T23:29:30+00:00"), t)).toBe(true);
  });
  test("far off, long gone, stopped already, or none", () => {
    expect(stopIsNear(at("2026-09-25T23:40:00+00:00"), t)).toBe(false);
    expect(stopIsNear(at("2026-09-25T23:20:00+00:00"), t)).toBe(false);
    expect(stopIsNear(at("2026-09-25T23:31:00+00:00", false), t)).toBe(false);
    expect(stopIsNear(at(null), t)).toBe(false);
    expect(stopIsNear(null, t)).toBe(false);
  });
});
