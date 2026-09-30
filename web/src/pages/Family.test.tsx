// The Family card is the wall everyone reads. Acceptance round 2 caught it
// printing the limit as time "used" ("17 min used" when she had used 21) and
// "Time's up" for someone stopped only by bedtime ("philip is out of time"
// with 0 minutes used). The card now says the verdict: the real reason, and
// the time really used.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cleanup, render, screen, within } from "@testing-library/react";

import type { FamilyChild, FamilyResponse, RulesVerdict } from "../types";
// Registers the shared module mocks; must be imported before the components.
import { apiImpl, armConfirm, resetApiMock, resetUiMocks } from "../test/mockApi";

const { MemoryRouter } = await import("react-router-dom");
const { SessionProvider } = await import("../lib/session");
const { ConfirmProvider } = await import("../lib/confirm");
const { resetFamily } = await import("../lib/family");
const { Family, familyVerdict, stopHeadline } = await import("./Family");

/** An ISO time on a computer at UTC+0, `h`:`m` today or tomorrow. */
function at(h: number, m = 0, tomorrow = false): string {
  const d = new Date();
  d.setUTCHours(h, m, 0, 0);
  if (tomorrow) d.setUTCDate(d.getUTCDate() + 1);
  return d.toISOString().replace("Z", "+00:00");
}

function stopped(reason: RulesVerdict["reason"], resume: string | null): RulesVerdict {
  return {
    allowed: false,
    reason,
    minutes_left: 0,
    stop_at: new Date().toISOString(),
    resume_at: resume,
    utc_offset_secs: 0,
  };
}

function kid(key: string, p: Partial<FamilyChild>): FamilyChild {
  return {
    key,
    account_id: key,
    name: key,
    age_bracket: "kid",
    theme: null,
    effective_theme: "playful",
    locked: false,
    used_minutes: 0,
    earned_minutes: 0,
    limit_minutes: 60,
    left_minutes: 60,
    profile_id: null,
    profile_name: null,
    devices: [{ id: `${key}-pc`, name: `${key}'s computer`, status: "online", locked: false, lock_pending: false, device_user_id: `du-${key}` }],
    pending_requests: 0,
    managed: true,
    self_managed: false,
    utc_offset_secs: 0,
    ...p,
  };
}

// Mia used 21 of her 17 (a parent's unlock ran out); philip is stopped by
// bedtime with nothing used; Theo is outside his hours; Ada is fine.
const mia = kid("Mia", { used_minutes: 21, limit_minutes: 17, left_minutes: 0, rules: stopped("limit", null) });
const philip = kid("philip", { used_minutes: 0, limit_minutes: 60, left_minutes: 0, rules: stopped("bedtime", at(7, 0, true)) });
const theo = kid("Theo", {
  used_minutes: 12,
  limit_minutes: 90,
  left_minutes: 0,
  rules: stopped("outside_hours", at(15, 0, true)),
});
const ada = kid("Ada", { used_minutes: 10, limit_minutes: 60, left_minutes: 50 });

function family(children: FamilyChild[]): FamilyResponse {
  return { children, devices: [], profiles: [], requests: [], server_time: new Date().toISOString() };
}

function setup(children: FamilyChild[]) {
  apiImpl.getFamily = () => Promise.resolve(family(children));
  render(
    <MemoryRouter initialEntries={["/"]}>
      <SessionProvider>
        <ConfirmProvider>
          <Family />
        </ConfirmProvider>
      </SessionProvider>
    </MemoryRouter>,
  );
}

async function card(name: string): Promise<HTMLElement> {
  return (await screen.findByRole("link", { name })).closest("li") as HTMLElement;
}

beforeEach(() => {
  resetApiMock();
  resetUiMocks();
  resetFamily();
  armConfirm();
});
afterEach(cleanup);

describe("a stop says its reason", () => {
  test("the headline per reason, on the computer's clock", () => {
    expect(stopHeadline(mia)).toBe("Time's up for today");
    expect(stopHeadline(philip)).toBe("Bedtime until tomorrow at 07:00");
    expect(stopHeadline(theo)).toBe("Outside allowed hours until tomorrow at 15:00");
    expect(stopHeadline(kid("P", { left_minutes: 0, rules: stopped("paused", null) }))).toBe("Paused");
    expect(stopHeadline(ada)).toBeNull();
    // At 03:00 on the computer, a bedtime ending at 07:00 reads as just the time.
    const early = kid("E", { left_minutes: 0, rules: stopped("bedtime", "2026-09-25T07:00:00+00:00") });
    expect(stopHeadline(early, new Date("2026-09-25T03:00:00Z"))).toBe("Bedtime until 07:00");
  });

  test("used is what was used — never the limit", async () => {
    setup([mia, philip, theo, ada]);
    const m = await card("Mia");
    expect(within(m).getByText("Time's up for today")).toBeTruthy();
    expect(m.textContent).toContain("21 min used");
    expect(m.textContent).not.toContain("17 min used");
    expect(within(m).getByText("Time's up")).toBeTruthy();

    const p = await card("philip");
    expect(within(p).getByText("Bedtime until tomorrow at 07:00")).toBeTruthy();
    expect(p.textContent).toContain("0 min used");
    expect(p.textContent).not.toMatch(/Time's up|1 h used/);
    expect(within(p).getByText("Bedtime")).toBeTruthy();

    const t = await card("Theo");
    expect(within(t).getByText("Outside allowed hours until tomorrow at 15:00")).toBeTruthy();
    expect(t.textContent).toContain("12 min used");
    expect(within(t).getByText("Outside hours")).toBeTruthy();

    const a = await card("Ada");
    expect(a.textContent).toContain("50 min left of 1 h");
  });

  test("the page's one sentence names each reason", () => {
    expect(familyVerdict([mia, philip, theo, ada])).toBe(
      "Mia is out of time. It's bedtime for philip. Theo is outside their allowed hours.",
    );
    expect(familyVerdict([philip])).toBe("It's bedtime for philip.");
    expect(familyVerdict([philip])).not.toMatch(/out of time/);
    expect(familyVerdict([ada])).toBe("Everyone is within their time.");
  });

  test("bedtime stops a child with no daily limit just the same", async () => {
    const sam = kid("Sam", {
      used_minutes: 5,
      limit_minutes: null,
      left_minutes: null,
      rules: stopped("bedtime", at(7, 0, true)),
    });
    setup([sam]);
    const s = await card("Sam");
    expect(within(s).getByText("Bedtime until tomorrow at 07:00")).toBeTruthy();
    expect(s.textContent).not.toMatch(/no limit set/);
  });

  test("an older server without a verdict still says time's up at zero", () => {
    const old = kid("Old", { used_minutes: 30, limit_minutes: 30, left_minutes: 0, rules: null });
    expect(stopHeadline(old)).toBe("Time's up for today");
  });
});
