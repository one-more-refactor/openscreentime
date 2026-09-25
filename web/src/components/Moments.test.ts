// Moments tell the day's story. A missing package on a child's computer is
// not "something tried to get around the rules", and a check that failed
// every ten seconds is one moment, not a page of them.
import { describe, expect, test } from "bun:test";
import type { Event } from "../types";
import { momentsFor, momentsOf, recovered, sayMoment, sentence, standingGaps, tone } from "./Moments";

let n = 0;
function ev(type: Event["type"], kind: string, severity: Event["severity"] = "warn"): Event {
  n += 1;
  return {
    id: `e${n}`,
    tenant_id: "t",
    device_id: "d",
    device_user_id: null,
    type,
    severity,
    payload: { kind, message: `about ${kind}` },
    created_at: new Date(Date.now() - n * 10_000).toISOString(),
  };
}

describe("moments", () => {
  test("machinery an older agent called tamper is not a moment", () => {
    const events = [
      ...Array.from({ length: 30 }, () => ev("tamper", "nft_probe_failed")),
      ...Array.from({ length: 30 }, () => ev("tamper", "nm_disconnect")),
      ev("tamper", "resolv_conf_reassert_failed", "critical"),
    ];
    expect(momentsOf(events)).toEqual([]);
  });

  test("a gap is one plain sentence, once", () => {
    const events = [
      ev("enforcement_degraded", "dns_resolver_missing", "critical"),
      ev("enforcement_degraded", "dns_resolver_missing", "critical"),
      ev("enforcement_degraded", "firewall_not_installed", "critical"),
    ];
    const m = momentsOf(events);
    expect(m.map(sentence)).toEqual(["The computer can't filter websites", "The computer can't apply its firewall rules"]);
    expect(m.map(sentence).join(" ")).not.toMatch(/get around/);
  });

  // Acceptance round 2: "can't filter websites" twice after every agent
  // start (two gap kinds, one meaning), and "Resumed" twice (the console's
  // own event and the computer's confirmation).
  test("two gaps that mean the same thing are one line", () => {
    const m = momentsOf([
      ev("enforcement_degraded", "dns_no_local_resolver", "critical"),
      ev("enforcement_degraded", "dns_policy_not_loaded", "critical"),
      // …and again at the next agent start.
      ev("enforcement_degraded", "dns_no_local_resolver", "critical"),
      ev("enforcement_degraded", "dns_policy_not_loaded", "critical"),
    ]);
    expect(m.map(sentence)).toEqual(["The computer can't filter websites"]);
  });

  test("the same moment moments apart is one line; later, it's news again", () => {
    const now = Date.now();
    const at = (type: Event["type"], minsAgo: number, device = "d"): Event => ({
      ...ev(type, ""),
      device_id: device,
      created_at: new Date(now - minsAgo * 60_000).toISOString(),
    });
    const m = momentsOf(
      [
        at("unlock", 1), // the computer confirms
        at("unlock", 1.1), // the console's own
        at("lock", 3),
        at("unlock", 40), // an earlier resume: its own moment
        at("unlock", 1, "other"), // another computer's: its own too
      ],
      10,
      now,
    );
    expect(m.map((e) => `${sentence(e)}@${e.device_id}`)).toEqual([
      "Resumed@d",
      "Resumed@other",
      "Paused@d",
      "Resumed@d",
    ]);
  });

  test("with two computers, a gap says which one", () => {
    const e = { ...ev("enforcement_degraded", "dns_resolver_missing"), device_id: "studio" };
    expect(sayMoment(e, "Studio laptop")).toBe("Studio laptop can't filter websites");
    expect(sentence(e)).toBe("The computer can't filter websites");
  });

  test("a real attempt still reads as one", () => {
    const m = momentsOf([ev("tamper", "nft_flush", "critical"), ev("unlock", "")]);
    expect(sentence(m[0])).toMatch(/^Something tried to get around the rules/);
    expect(m).toHaveLength(2);
  });
});

// Acceptance round 3, the console's Moments on a real computer: Philip's own
// snoozes told on Mia's page, Mia's code unlocks never told, and a red
// "can't filter websites" that stayed after the computer fixed itself 4 s
// later.
describe("whose moment, and is it over", () => {
  const at = (e: Partial<Event> & Pick<Event, "type">, minsAgo = 1): Event => ({
    id: `m${(n += 1)}`,
    tenant_id: "t",
    device_id: "mia-pc",
    device_user_id: null,
    severity: "info",
    payload: {},
    created_at: new Date(Date.now() - minsAgo * 60_000).toISOString(),
    ...e,
  });
  const gap = at({ type: "enforcement_degraded", severity: "critical", payload: { kind: "dns_resolver_stopped" } }, 3);

  test("a gap the computer reports gone reads as over, not red", () => {
    // Online, and its state frame lists no DNS gap any more.
    const standing = standingGaps([{ id: "mia-pc", status: "online", last_state: { gaps: [] } }]);
    expect(recovered(gap, standing)).toBe(true);
    expect(sayMoment(gap, undefined, standing)).toBe("The computer couldn't filter websites for a while — it's working again");
    expect(tone(gap, standing)).toBe("ok");
    const [m] = momentsOf([gap], 5, Date.now(), standing);
    expect(sayMoment(m, "Mia's computer", standing)).toBe(
      "Mia's computer couldn't filter websites for a while — it's working again",
    );
  });

  test("a gap still standing — or one nobody can vouch for — stays as it was", () => {
    const still = standingGaps([{ id: "mia-pc", status: "online", last_state: { gaps: ["dns_no_local_resolver"] } }]);
    expect(sayMoment(gap, undefined, still)).toBe("The computer can't filter websites");
    expect(tone(gap, still)).toBe("crit");
    // Another area's gap doesn't vouch for the filter.
    const other = standingGaps([{ id: "mia-pc", status: "online", last_state: { gaps: ["firewall_not_installed"] } }]);
    expect(recovered(gap, other)).toBe(true);
    // Offline, or never said: unknown — it says what it said.
    for (const d of [
      { id: "mia-pc", status: "offline", last_state: { gaps: [] } },
      { id: "mia-pc", status: "online", last_state: null },
    ]) {
      expect(tone(gap, standingGaps([d]))).toBe("crit");
    }
    expect(tone(gap)).toBe("crit");
  });

  test("the unlock code at the lock is a moment; the same code in sudo isn't", () => {
    const lock = at({ type: "parent_code_ok", device_user_id: "du-mia", payload: { via: "lock_screen", user: "mia" } });
    const sudo = at({ type: "parent_code_ok", device_user_id: "du-mia", payload: { via: "pam", user: "mia" } }, 2);
    const spare = at(
      { type: "parent_code_backup_used", severity: "warn", device_user_id: "du-mia", payload: { via: "lock_screen" } },
      30,
    );
    // The agent's own note of the same unlock is machinery, not a second line.
    const note = at({ type: "tamper", device_user_id: "du-mia", payload: { kind: "parent_pin_override" } });
    expect(momentsOf([lock, sudo, spare, note]).map((e) => sentence(e))).toEqual([
      "Unlocked with the unlock code",
      "Unlocked with a recovery code",
    ]);
  });

  test("more time says whose it was", () => {
    const given = at({ type: "screen_time_earned", device_user_id: "du-mia", payload: { reward_minutes: 15 } });
    const snooze = at({ type: "screen_time_earned", device_user_id: "du-philip", payload: { minutes: 15, via: "self" } });
    expect(sentence(given)).toBe("Got 15 more minutes");
    expect(sentence(snooze)).toBe("Took 15 more minutes");
  });

  test("a lost connection is the computer's moment, said once", () => {
    const off = at({ type: "tamper", severity: "warn", payload: { kind: "network_offline" } });
    const back = at({ type: "tamper", payload: { kind: "network_online" } });
    expect(momentsOf([off, back, { ...off, id: "again" }]).map((e) => sentence(e))).toEqual([
      "The computer was out of touch with the server for a while — it kept its rules",
    ]);
  });

  test("a person's moments: their logins, and the computer's own only on a computer of theirs", () => {
    const events = [
      at({ type: "screen_time_earned", device_user_id: "du-philip", payload: { minutes: 15, via: "self" } }),
      at({ type: "parent_code_ok", device_user_id: "du-mia", payload: { via: "lock_screen" } }),
      gap,
      at({ type: "lock", device_id: "family-pc" }),
    ];
    const mia = momentsFor(events, { logins: new Set(["du-mia"]), theirs: (id) => id === "mia-pc" });
    expect(mia.map((e) => e.type)).toEqual(["parent_code_ok", "enforcement_degraded"]);
    // Leo uses Mia's computer too, with his own login: her computer's gap and
    // Philip's snooze aren't his story.
    const leo = momentsFor(events, { logins: new Set(["du-leo"]), theirs: (id) => id === "family-pc" });
    expect(leo.map((e) => e.type)).toEqual(["lock"]);
  });
});
