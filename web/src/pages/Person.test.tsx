// The person page is two pages now: Today (the glance and the verbs) and
// Rules (the editor). What matters: each has its own job and nothing of the
// other's, there is one blocklist and one way to stop screens, the words are
// the product's words, and an adult's rules stay theirs.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

import type { Device, Event, FamilyChild, FamilyResponse, Policy } from "../types";
// Registers the shared module mocks; must be imported before the components.
import { apiCalls, apiImpl, armConfirm, resetApiMock, resetUiMocks } from "../test/mockApi";

const { MemoryRouter, Route, Routes } = await import("react-router-dom");
const { ConfirmProvider } = await import("../lib/confirm");
const { resetFamily } = await import("../lib/family");
const { Person } = await import("./Person");
const { arcPath, ringGeometry } = await import("../components/Ring");

const kidPolicy: Policy = {
  version: 1,
  dns: { mode: "allow_all", allowlist: ["*"], blocklist: ["9gag.com"], safe_search: true, upstream: "1.1.1.3" },
  firewall: { mode: "allow_all", allow_outbound_ports: [], allow_inbound_ports: [] },
  screen_time: {
    enabled: true,
    daily_limit_minutes: 60,
    schedule: [{ days: [1, 2, 3, 4, 5], start: "15:00", end: "19:00" }],
    bedtime: { start: "20:00", end: "07:00" },
  },
  gamification: {
    earn_time: { enabled: true, tasks: [{ id: "reading", label: "Read for 20 min", reward_minutes: 15 }] },
    lockout: { enabled: true, unlock_challenge: "parent_pin" },
  },
  blocks: { apps: [], categories: ["adult"], custom_domains: [] },
};

function device(id: string, status: Device["status"]): Device {
  return {
    id,
    tenant_id: "t",
    name: id,
    hostname: id,
    os: "linux",
    agent_version: "0.6.1",
    status,
    locked: false,
    lock_pending: false,
    tamper_level: 1,
    public_ip: null,
    last_seen: new Date().toISOString(),
    created_at: new Date().toISOString(),
    recovery_codes_unused: 0,
  };
}

function person(p: Partial<FamilyChild> & Pick<FamilyChild, "key" | "name" | "age_bracket">): FamilyChild {
  return {
    account_id: p.key,
    theme: null,
    effective_theme: "plain",
    locked: false,
    used_minutes: 30,
    earned_minutes: 0,
    limit_minutes: null,
    profile_id: null,
    profile_name: null,
    devices: [],
    pending_requests: 0,
    ...p,
  };
}

function family(): FamilyResponse {
  return {
    children: [
      person({
        key: "mia",
        name: "Mia",
        age_bracket: "kid",
        used_minutes: 20,
        limit_minutes: 60,
        left_minutes: 40,
        profile_id: "p-mia",
        managed: true,
        self_managed: false,
        devices: [
          // An offline computer first: a grant must still land on the online one.
          { id: "old-pc", name: "Old PC", status: "offline", locked: false, lock_pending: false, device_user_id: "du-old" },
          { id: "mia-laptop", name: "Mia's laptop", status: "online", locked: false, lock_pending: false, device_user_id: "du-mia" },
        ],
      }),
      person({
        key: "jo",
        name: "Jo",
        age_bracket: "adult",
        managed: false,
        self_managed: true,
        used_minutes: 72,
        devices: [{ id: "jo-desk", name: "Desk", status: "online", locked: false, lock_pending: false, device_user_id: "du-jo" }],
      }),
    ],
    devices: [device("old-pc", "offline"), device("mia-laptop", "online"), device("jo-desk", "online")],
    profiles: [
      {
        id: "p-mia",
        tenant_id: "t",
        name: "Mia's rules",
        kind: "kid",
        is_preset: false,
        policy: kidPolicy,
        created_at: "",
        updated_at: "",
      },
    ],
    requests: [],
    server_time: new Date().toISOString(),
  };
}

function setup(path: string) {
  render(
    <MemoryRouter initialEntries={[path]}>
      <ConfirmProvider>
        <Routes>
          <Route path="/child/:key" element={<Person tab="today" />} />
          <Route path="/child/:key/rules" element={<Person tab="rules" />} />
        </Routes>
      </ConfirmProvider>
    </MemoryRouter>,
  );
}

/** A "time's up" moment on this login's computer, a minute ago. */
function timesUp(deviceUserId: string): Event {
  return {
    id: `e-${deviceUserId}`,
    tenant_id: "t",
    device_id: deviceUserId === "du-jo" ? "jo-desk" : "mia-laptop",
    device_user_id: deviceUserId,
    type: "screen_time_exceeded",
    severity: "info",
    payload: {},
    created_at: new Date(Date.now() - 60_000).toISOString(),
  };
}

/** Words the product retired: one credential name, one stop verb. */
const RETIRED = /parent code|\bPIN\b|authenticator|Protection|Block account|Locked-down|When time runs out|Math problem/i;

beforeEach(() => {
  resetApiMock();
  resetUiMocks();
  resetFamily();
  armConfirm();
  apiImpl.getFamily = () => Promise.resolve(family());
});
afterEach(cleanup);

describe("today", () => {
  test("the day and the verbs — and none of the rules editor", async () => {
    setup("/child/mia");
    const hero = await screen.findByRole("region", { name: "Today" });
    expect(within(hero).getByText("min left")).toBeTruthy();
    expect(within(hero).getByRole("button", { name: "Pause" })).toBeTruthy();
    expect(within(hero).getByRole("button", { name: "Give 15 min" })).toBeTruthy();
    expect(within(hero).getByRole("button", { name: "Give 30 min" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Keys" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Their computers" })).toBeTruthy();
    // The editor lives on the other tab.
    expect(screen.queryByRole("heading", { name: "Daily limit" })).toBeNull();
    expect(screen.queryByRole("button", { name: /remove/i })).toBeNull();
    // Both halves are one tap apart.
    expect(screen.getByRole("link", { name: "Rules" }).getAttribute("href")).toBe("/child/mia/rules");
    expect(document.body.textContent ?? "").not.toMatch(RETIRED);
  });

  test("the ring fills with time used, not time left", async () => {
    setup("/child/mia");
    const hero = await screen.findByRole("region", { name: "Today" });
    // 20 of 60 used: a third of the way round from the tick, not two thirds.
    const ring = within(hero).getByRole("img");
    expect(ring.getAttribute("aria-label")).toBe("20 min used of 1 h, 40 min left");
    const g = ringGeometry(152);
    const d = ring.querySelector(".ring-arc")?.getAttribute("d");
    expect(d).toBe(arcPath(g.c, g.r, 20 / 60));
    expect(d).not.toBe(arcPath(g.c, g.r, 40 / 60));
  });

  test("more time lands on the computer that's online", async () => {
    setup("/child/mia");
    fireEvent.click(await screen.findByRole("button", { name: "Give 15 min" }));
    await waitFor(() => expect(apiCalls.credit).toEqual([["du-mia", 15]]));
  });

  test("an adult's day stays theirs: minutes only, never where it went", async () => {
    setup("/child/jo");
    expect(await screen.findByText(/keep the details of their day to themselves/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Give 15 min" })).toBeNull();
    expect(apiCalls.where).toEqual([]);
  });

  test("an adult has no moments on their page, and none are fetched", async () => {
    apiImpl.listEvents = () => Promise.resolve([timesUp("du-jo")]);
    setup("/child/jo");
    expect(await screen.findByText(/keep the details of their day to themselves/)).toBeTruthy();
    await new Promise((r) => setTimeout(r, 20));
    expect(apiCalls.events).toEqual([]);
    expect(screen.queryByText("Time's up for the day")).toBeNull();
  });

  test("a child's moments still show", async () => {
    apiImpl.listEvents = (id) => Promise.resolve(id === "mia-laptop" ? [timesUp("du-mia")] : []);
    setup("/child/mia");
    expect(await screen.findByText("Time's up for the day")).toBeTruthy();
    expect(apiCalls.events.length).toBeGreaterThan(0);
  });
});

describe("rules", () => {
  test("the whole editor, one blocklist, and remove at the bottom", async () => {
    setup("/child/mia/rules");
    expect(await screen.findByRole("heading", { name: "Daily limit" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "When screens can be on" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Blocked" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Earning time" })).toBeTruthy();
    // One place to block a site by name — not two lists.
    expect(screen.getAllByLabelText("Block a site by name")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Remove Mia" })).toBeTruthy();
    // The glance lives on the other tab.
    expect(screen.queryByRole("button", { name: "Give 15 min" })).toBeNull();
    expect(document.body.textContent ?? "").not.toMatch(RETIRED);
  });

  test("a site typed by name joins the one list, and the old list folds into it", async () => {
    setup("/child/mia/rules");
    // The old editor's dns.blocklist entry shows in the same list.
    expect(await screen.findByText("9gag.com")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Block a site by name"), { target: { value: "https://Example.org/page" } });
    fireEvent.click(screen.getByRole("button", { name: "Block" }));
    await waitFor(() => expect(apiCalls.profileSaves).toHaveLength(1));
    const saved = apiCalls.profileSaves[0].policy;
    expect(saved.blocks?.custom_domains).toEqual(["9gag.com", "example.org"]);
    expect(saved.dns.blocklist).toEqual([]);
  });

  test("allowed hours refuse an empty window and keep one that runs past midnight", async () => {
    setup("/child/mia/rules");
    const row = (await screen.findByText("School days")).closest(".row") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: "Change" }));
    const from = within(row).getByLabelText("School days from");
    const until = within(row).getByLabelText("School days until");
    fireEvent.change(from, { target: { value: "15:00" } });
    fireEvent.change(until, { target: { value: "15:00" } });
    expect(within(row).getByRole("alert").textContent).toMatch(/same/);
    expect((within(row).getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.change(from, { target: { value: "20:00" } });
    fireEvent.change(until, { target: { value: "01:00" } });
    fireEvent.click(within(row).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(apiCalls.profileSaves).toHaveLength(1));
    expect(apiCalls.profileSaves[0].policy.screen_time.schedule).toEqual([
      { days: [1, 2, 3, 4, 5], start: "20:00", end: "01:00" },
    ]);
  });

  test("an adult sets their own rules — no editor for anyone else", async () => {
    setup("/child/jo/rules");
    expect(await screen.findByRole("heading", { name: "Jo sets their own rules" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Daily limit" })).toBeNull();
    expect(screen.queryByLabelText("Block a site by name")).toBeNull();
  });
});
