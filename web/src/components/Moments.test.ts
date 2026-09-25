// Moments tell the day's story. A missing package on a child's computer is
// not "something tried to get around the rules", and a check that failed
// every ten seconds is one moment, not a page of them.
import { describe, expect, test } from "bun:test";
import type { Event } from "../types";
import { momentsOf, sentence } from "./Moments";
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
