// A computer that can't filter the network is still doing screen time, and
// the console has to say exactly that — not "doing what it should", and
// never "something tried to get around the rules" for a missing package.
import { describe, expect, test } from "bun:test";
import type { Device, Event } from "../types";
import {
  cantFilter,
  degradedDevices,
  degradedSentence,
  degradedSummary,
  gapPhrase,
  isNotAnAttempt,
  notBlockedSentence,
} from "./degraded";

function device(name: string, gaps: string[], status: Device["status"] = "online"): Device {
  return {
    id: name,
    tenant_id: "t",
    name,
    hostname: name,
    os: "linux",
    agent_version: "0.6.1",
    status,
    locked: false,
    lock_pending: false,
    last_state: { locked: false, frozen_users: [], enforcing: gaps.length === 0, gaps },
    tamper_level: 1,
    public_ip: null,
    last_seen: new Date().toISOString(),
    created_at: new Date().toISOString(),
  };
}

function event(type: Event["type"], kind: string): Event {
  return {
    id: kind,
    tenant_id: "t",
    device_id: "d",
    device_user_id: null,
    type,
    severity: "warn",
    payload: { kind },
    created_at: new Date().toISOString(),
  };
}

describe("a computer that can't apply all of its rules", () => {
  test("stock Debian: no dnsmasq, no nftables", () => {
    const mia = device("Mia's computer", ["dns_resolver_missing", "firewall_not_installed"]);
    expect(degradedDevices([mia])).toEqual([mia]);
    expect(degradedSentence(mia)).toBe(
      "Mia's computer can't filter websites or apply its firewall rules. Screen time still works there. " +
        "Run the install command on it again to add what's missing.",
    );
  });

  test("a resolv.conf still owned by systemd-resolved is fixed by re-running the install", () => {
    for (const kind of ["dns_policy_not_loaded", "dns_resolv_conf_not_a_file"]) {
      expect(degradedSentence(device("Mia's computer", [kind]))).toBe(
        "Mia's computer can't filter websites. Screen time still works there. " +
          "Run the install command on it again to add what's missing.",
      );
    }
  });

  test("a computer that can't freeze doesn't claim screen time works", () => {
    const s = degradedSentence(device("Old laptop", ["screen_time_no_freezer"]));
    expect(s).toBe("Old laptop can't stop the screen when time is up.");
  });

  test("healthy and offline computers are not degraded", () => {
    const ok = device("Fine", []);
    const off = device("Off", ["dns_resolver_missing"], "offline");
    expect(degradedDevices([ok, off])).toEqual([]);
    expect(degradedSummary([ok, off])).toBeNull();
  });

  test("several at once", () => {
    const s = degradedSummary([device("A", ["dns_no_local_resolver"]), device("B", ["vpn_not_running"])]);
    expect(s).toBe("2 computers can't apply all of their rules. Computers says what each one is missing.");
  });

  test("every gap has words", () => {
    expect(gapPhrase("dns_policy_not_loaded")).toBe("can't filter websites");
    expect(gapPhrase("firewall_not_applied")).toBe("can't apply its firewall rules");
    expect(gapPhrase("network_apply_failed")).toBe("can't apply all of its rules");
  });
});

describe("what is not an attempt to get around the rules", () => {
  test("machinery an older agent sent as tamper", () => {
    for (const kind of [
      "nft_probe_failed",
      "nm_disconnect",
      "resolv_conf_reassert_failed",
      "dns_no_local_resolver",
      "agent_updated",
    ]) {
      expect(isNotAnAttempt(event("tamper", kind))).toBe(true);
    }
  });

  test("real attempts still are", () => {
    for (const kind of ["nft_flush", "resolv_conf_drift", "evasion_confirmed", "clock_rollback"]) {
      expect(isNotAnAttempt(event("tamper", kind))).toBe(false);
    }
    expect(isNotAnAttempt(event("enforcement_degraded", "nft_probe_failed"))).toBe(false);
  });
});

// Acceptance round 2: Mia's rules said "what's here is really blocked, on
// every computer they use" and Philip's own page "Focus hours until 05:00 —
// the sites below open again then", on a computer that filtered nothing. A
// rule that blocks websites now says where it can't, right at the rule.
describe("a block promised only where it holds", () => {
  test("one computer that can't filter websites", () => {
    const studio = device("Studio laptop", ["dns_no_local_resolver", "dns_policy_not_loaded"]);
    expect(cantFilter([studio])).toEqual([studio]);
    expect(notBlockedSentence([studio])).toBe(
      "Not blocked on Studio laptop yet — it can't filter websites. Screen time still works.",
    );
  });

  test("several, by name — and only the ones that can't", () => {
    const a = device("Studio laptop", ["dns_resolver_missing"]);
    const b = device("Desk", ["dns_policy_not_loaded", "firewall_not_installed"]);
    const fine = device("Mia's computer", ["firewall_not_installed"]);
    expect(notBlockedSentence([a, fine, b])).toBe(
      "Not blocked on Studio laptop or Desk yet — they can't filter websites. Screen time still works.",
    );
  });

  test("every computer filtering, or offline, promises nothing false", () => {
    expect(notBlockedSentence([device("Fine", [])])).toBeNull();
    expect(notBlockedSentence([device("Off", ["dns_resolver_missing"], "offline")])).toBeNull();
    expect(notBlockedSentence([])).toBeNull();
  });

  test("screen time is only promised where it works", () => {
    const s = notBlockedSentence([device("Old laptop", ["dns_resolver_missing", "screen_time_no_freezer"])]);
    expect(s).toBe("Not blocked on Old laptop yet — it can't filter websites.");
  });

  test("the own page's computers carry their gaps flat", () => {
    expect(notBlockedSentence([{ name: "Philip's computer", status: "online", gaps: ["dns_policy_not_loaded"] }])).toBe(
      "Not blocked on Philip's computer yet — it can't filter websites. Screen time still works.",
    );
  });
});
