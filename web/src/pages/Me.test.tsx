// A person's own page: one look for everyone. What matters: the ring fills
// with time used (the same direction the parent's card fills — it used to
// empty here), a child sees their rules and a way to ask, and someone keeping
// their own time gets "My computer" — their own limit, focus hours and sites,
// validated exactly the way the agent reads them, with no "parent" anywhere.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

import type { MeToday, MyRules } from "../types";
// Registers the shared module mocks; must be imported before the components.
import { apiCalls, apiImpl, resetApiMock, resetUiMocks } from "../test/mockApi";

const { MemoryRouter } = await import("react-router-dom");
const { SessionProvider } = await import("../lib/session");
const { Me } = await import("./Me");
const { arcPath, ringGeometry } = await import("../components/Ring");

function today(p: Partial<MeToday> = {}): MeToday {
  return {
    used_minutes: 30,
    earned_minutes: 0,
    limit_minutes: 120,
    left_minutes: 90,
    locked: false,
    devices: [{ name: "Desk laptop", status: "online", locked: false }],
    blocks: { apps: [], categories: [], custom_domains: [] },
    bracket: "kid",
    theme: "playful",
    pending_request: false,
    bedtime: { start: "20:00", end: "07:00" },
    windows: [],
    self_managed: false,
    ...p,
  };
}

const mine: MyRules = {
  daily_limit_minutes: 180,
  focus_hours: { days: [1, 2, 3, 4, 5], start: "09:00", end: "12:00" },
  sites: ["reddit.com"],
};

function setup() {
  render(
    <MemoryRouter initialEntries={["/me"]}>
      <SessionProvider>
        <Me />
      </SessionProvider>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  resetApiMock();
  resetUiMocks();
  try {
    localStorage.clear();
  } catch {
    /* fine */
  }
});
afterEach(cleanup);

describe("my day (a child)", () => {
  test("the ring fills with time used, like every ring", async () => {
    apiImpl.getMeToday = () => Promise.resolve(today());
    setup();
    const ring = await screen.findByRole("img", { name: /used of/ });
    expect(ring.getAttribute("aria-label")).toBe("30 min used of 2 h, 1 h 30 min left");
    const g = ringGeometry(176);
    const d = ring.querySelector(".ring-arc")?.getAttribute("d");
    // A quarter of the day used: the arc runs a quarter of the way round.
    expect(d).toBe(arcPath(g.c, g.r, 0.25));
    expect(d).not.toBe(arcPath(g.c, g.r, 0.75));
  });

  test("time's up is a full ring", async () => {
    apiImpl.getMeToday = () => Promise.resolve(today({ used_minutes: 120, left_minutes: 0 }));
    setup();
    const ring = await screen.findByRole("img", { name: /used of/ });
    expect(ring.getAttribute("data-state")).toBe("full");
    expect(screen.getByText(/That's all the screen time for today/)).toBeTruthy();
  });

  test("their rules in one glance, and a way to ask — no editing", async () => {
    apiImpl.getMeToday = () => Promise.resolve(today());
    setup();
    expect(await screen.findByRole("heading", { name: "Your rules" })).toBeTruthy();
    expect(screen.getByText("20:00 – 07:00")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Ask for 15 more minutes" })).toBeTruthy();
    expect(screen.queryByText("My daily limit")).toBeNull();
  });
});

describe("my computer (keeping my own time)", () => {
  beforeEach(() => {
    apiImpl.getMeToday = () =>
      Promise.resolve(
        today({
          bracket: "adult",
          self_managed: true,
          limit_minutes: 180,
          left_minutes: 150,
          bedtime: null,
          focus: { sites: mine.sites, hours: mine.focus_hours },
        }),
      );
    apiImpl.getMyRules = () => Promise.resolve({ ...mine });
  });

  test("my limit, my focus hours, my sites — and no one else in the copy", async () => {
    setup();
    expect(await screen.findByRole("heading", { name: "My daily limit" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "My focus hours" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Sites I block for myself" })).toBeTruthy();
    expect(screen.getByText("reddit.com")).toBeTruthy();
    expect(screen.getByText(/wait a minute and take 15 more/)).toBeTruthy();
    expect(document.body.textContent ?? "").not.toMatch(/parent/i);
  });

  test("focus hours refuse an empty window and save one that ends at midnight", async () => {
    setup();
    const card = (await screen.findByRole("heading", { name: "My focus hours" })).closest(".me-mrule") as HTMLElement;
    fireEvent.click(within(card).getByRole("button", { name: "Edit" }));
    const from = within(card).getByLabelText("Focus from");
    const until = within(card).getByLabelText("Focus until");
    fireEvent.change(from, { target: { value: "15:00" } });
    fireEvent.change(until, { target: { value: "15:00" } });
    expect(within(card).getByRole("alert").textContent).toMatch(/same/);
    expect((within(card).getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);

    // No day picked is refused too.
    fireEvent.change(until, { target: { value: "00:00" } });
    for (const d of ["Mon", "Tue", "Wed", "Thu", "Fri"]) fireEvent.click(within(card).getByRole("button", { name: d }));
    expect(within(card).getByRole("alert").textContent).toMatch(/day/);
    fireEvent.click(within(card).getByRole("button", { name: "Sat" }));

    fireEvent.change(from, { target: { value: "22:00" } });
    fireEvent.click(within(card).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(apiCalls.myRules).toHaveLength(1));
    expect(apiCalls.myRules[0]).toEqual({
      ...mine,
      focus_hours: { days: [6], start: "22:00", end: "00:00" },
    });
  });

  test("a site is checked before it's saved, and saved the way the server stores it", async () => {
    setup();
    const card = (await screen.findByRole("heading", { name: "Sites I block for myself" })).closest(
      ".me-mrule",
    ) as HTMLElement;
    fireEvent.click(within(card).getByRole("button", { name: "Add a site" }));
    const input = within(card).getByLabelText("A site to block");
    fireEvent.change(input, { target: { value: "not a site" } });
    fireEvent.click(within(card).getByRole("button", { name: "Block it" }));
    expect(await within(card).findByText(/isn't a site name/)).toBeTruthy();
    expect(apiCalls.myRules).toHaveLength(0);

    fireEvent.change(input, { target: { value: "https://www.YouTube.com/watch?v=1" } });
    fireEvent.click(within(card).getByRole("button", { name: "Block it" }));
    await waitFor(() => expect(apiCalls.myRules).toHaveLength(1));
    expect(apiCalls.myRules[0].sites).toEqual(["reddit.com", "www.youtube.com"]);
  });
});
