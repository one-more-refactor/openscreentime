// Moments tell the day's story. A missing package on a child's computer is
// not "something tried to get around the rules", and a check that failed
// every ten seconds is one moment, not a page of them.
import { describe, expect, test } from "bun:test";
import type { Event } from "../types";
import { momentsOf, sayMoment, sentence } from "./Moments";
import { installCommand } from "./EnrollCommand";

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

describe("the install command", () => {
  test("https: as it was", () => {
    expect(installCommand("T", "https://ost.example.org")).toBe(
      "curl -fsSL https://ost.example.org/install.sh | sudo OST_TOKEN=T sh -s -- --server https://ost.example.org",
    );
  });

  test("plain http: says --insecure-http, or the installer refuses", () => {
    expect(installCommand("T", "http://ost-host.local:18080")).toBe(
      "curl -fsSL http://ost-host.local:18080/install.sh | sudo OST_TOKEN=T sh -s -- " +
        "--server http://ost-host.local:18080 --insecure-http",
    );
  });
});
