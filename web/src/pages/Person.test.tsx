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

  // Acceptance, step 6a: Give 15 left her ask pending. The grant goes to the
  // computer she asked from — the server answers the ask, the computer tells her.
  test("more time lands on the computer they asked from", async () => {
    const f = family();
    f.requests = [
      {
        id: "r1",
        device_id: "old-pc",
        device_user_id: "du-old",
        os_username: "mia",
        task_id: "ask",
        task_label: "Asked for more time",
        minutes: 15,
        status: "pending",
        created_at: new Date().toISOString(),
        decided_at: null,
      },
    ];
    apiImpl.getFamily = () => Promise.resolve(f);
    setup("/child/mia");
    // A plain ask reads as one: no quoted "reason".
    expect(await screen.findByText(/Asked for 15 more minutes/)).toBeTruthy();
    expect(screen.queryByText(/“/)).toBeNull();
    const hero = screen.getByRole("region", { name: "Today" });
    // The verb, not the answer on the request row.
    const give = within(hero)
      .getAllByRole("button", { name: "Give 15 min" })
      .find((b) => !b.closest('[aria-label="A request for time"]'));
    fireEvent.click(give!);
    await waitFor(() => expect(apiCalls.credit).toEqual([["du-old", 15]]));
  });

  // Acceptance, step 5: after the unlock code the console showed a red "0 min
  // left / time's up" while she was unlocked for 30 minutes.
  test("an unlock code's time shows as time, and says until when", async () => {
    const until = new Date(Date.now() + 29 * 60_000).toISOString();
    const f = family();
    f.children[0] = {
      ...f.children[0],
      used_minutes: 66,
      left_minutes: 29,
      rules: { allowed: true, reason: "limit", minutes_left: 29, stop_at: until, resume_at: null, override_until: until },
    };
    apiImpl.getFamily = () => Promise.resolve(f);
    setup("/child/mia");
    const hero = await screen.findByRole("region", { name: "Today" });
    expect(within(hero).getByText(/^Unlocked until \d\d:\d\d\.$/)).toBeTruthy();
    const ring = within(hero).getByRole("img");
    expect(ring.getAttribute("data-state")).not.toBe("full");
    expect(ring.getAttribute("aria-label")).toMatch(/29 min left/);
    expect(hero.textContent ?? "").not.toMatch(/Time's up/);
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

  // Acceptance round 4: the real server sent no account_id, so her page showed
  // her father's apps and Edit / Remove went to /api/members/undefined. Every
  // per-person call addresses her by her account id (the card's shape is
  // checked against the server's in test/shapes.test.ts).
  test("where the time went, Edit and Remove all address her, by her id", async () => {
    const f = family();
    f.children[0] = { ...f.children[0], key: "acc-mia-1", account_id: "acc-mia-1" };
    apiImpl.getFamily = () => Promise.resolve(f);
    setup("/child/acc-mia-1");
    await screen.findByRole("region", { name: "Today" });
    await waitFor(() => expect(apiCalls.where).toEqual(["acc-mia-1"]));

    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "Mia R" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(apiCalls.members).toEqual([["update", "acc-mia-1"]]));

    cleanup();
    setup("/child/acc-mia-1/rules");
    fireEvent.click(await screen.findByRole("button", { name: "Remove Mia" }));
    fireEvent.change(await screen.findByLabelText("Type Mia to confirm"), { target: { value: "Mia" } });
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove Mia" }));
    await waitFor(() =>
      expect(apiCalls.members).toEqual([
        ["update", "acc-mia-1"],
        ["delete", "acc-mia-1"],
      ]),
    );
  });

  // Acceptance round 3: 37 minutes of Firefox and Text Editor were "Nothing
  // yet today". The computer now names desktop apps; the page shows the name.
  test("where the time went names a desktop app as the computer does", async () => {
    apiImpl.getWhere = () =>
      Promise.resolve({
        apps: [
          { key: "Firefox ESR", seconds: 25 * 60 },
          { key: "Text Editor", seconds: 12 * 60 },
        ],
        sites: [],
        hours: [],
        sites_hidden_shared: false,
        sites_hidden_age: false,
      });
    setup("/child/mia");
    expect((await screen.findAllByText("Firefox ESR")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("Text Editor").length).toBeGreaterThan(0);
    expect(screen.getByText("25 min")).toBeTruthy();
    expect(screen.queryByText("Nothing yet today.")).toBeNull();
  });

  // Acceptance round 3: Philip's own snoozes showed on Mia's page as "Got
  // some more minutes", her code unlocks never showed, and a red "can't
  // filter websites" stayed after her computer fixed itself.
  test("her moments are hers: her unlock, her computer's gap as over — never a parent's snooze", async () => {
    const f = family();
    f.devices = f.devices.map((d) =>
      d.id === "mia-laptop"
        ? {
            ...d,
            owner_account_id: "mia",
            last_state: { locked: false, frozen_users: [], enforcing: true, gaps: [] },
          }
        : d.id === "old-pc"
          ? { ...d, owner_account_id: "leo" }
          : d,
    );
    apiImpl.getFamily = () => Promise.resolve(f);
    const ago = (m: number) => new Date(Date.now() - m * 60_000).toISOString();
    const base = { tenant_id: "t", severity: "info" as const };
    apiImpl.listEvents = (id) =>
      Promise.resolve(
        id === "mia-laptop"
          ? [
              // Philip's own login on her computer gave himself 15 minutes.
              { ...base, id: "snooze", device_id: "mia-laptop", device_user_id: "du-philip", type: "screen_time_earned",
                payload: { user: "philip", minutes: 15, via: "self" }, created_at: ago(2) },
              { ...base, id: "code", device_id: "mia-laptop", device_user_id: "du-mia", type: "parent_code_ok",
                payload: { via: "lock_screen", user: "mia" }, created_at: ago(5) },
              { ...base, id: "gap", device_id: "mia-laptop", device_user_id: null, type: "enforcement_degraded",
                severity: "critical", payload: { kind: "dns_resolver_stopped" }, created_at: ago(9) },
            ]
          : id === "old-pc"
            ? // Leo's computer, which she also uses: its pause isn't her story.
              [{ ...base, id: "pause", device_id: "old-pc", device_user_id: null, type: "lock", payload: {},
                 created_at: ago(3) }]
            : [],
      );
    setup("/child/mia");
    expect(await screen.findByText("Unlocked with the unlock code")).toBeTruthy();
    const fixed = screen.getByText("Mia's laptop couldn't filter websites for a while — it's working again");
    expect(fixed.closest("li")?.getAttribute("data-tone")).toBe("ok");
    expect(screen.queryByText(/more minutes/)).toBeNull();
    expect(screen.queryByText("Paused")).toBeNull();
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

  test("a computer that can't filter websites is named right at the blocklist", async () => {
    // Acceptance round 2: "what's here is really blocked, on every computer
    // they use" — on a computer that filtered nothing.
    const f = family();
    f.devices = f.devices.map((d) =>
      d.id === "mia-laptop"
        ? {
            ...d,
            last_state: {
              locked: false,
              frozen_users: [],
              enforcing: false,
              gaps: ["dns_no_local_resolver", "dns_policy_not_loaded"],
            },
          }
        : d,
    );
    apiImpl.getFamily = () => Promise.resolve(f);
    setup("/child/mia/rules");
    const note = await screen.findByText(
      "Not blocked on Mia's laptop yet — it can't filter websites. Screen time still works.",
    );
    // At the rule: inside the Blocked section, above the list.
    const section = screen.getByRole("heading", { name: "Blocked" }).closest("section") as HTMLElement;
    expect(section.contains(note)).toBe(true);
    expect(document.body.textContent ?? "").not.toMatch(/really blocked/);
  });

  test("with every computer filtering, the promise stands", async () => {
    setup("/child/mia/rules");
    expect(await screen.findByText(/really blocked, on every computer they use/)).toBeTruthy();
    expect(screen.queryByText(/Not blocked on/)).toBeNull();
  });

  test("an adult sets their own rules — no editor for anyone else", async () => {
    setup("/child/jo/rules");
    expect(await screen.findByRole("heading", { name: "Jo sets their own rules" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Daily limit" })).toBeNull();
    expect(screen.queryByLabelText("Block a site by name")).toBeNull();
  });
});
