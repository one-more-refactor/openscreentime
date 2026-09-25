// A computer that can't filter the network is still doing screen time, and
// the console has to say exactly that — not "doing what it should", and
// never "something tried to get around the rules" for a missing package.
import { describe, expect, test } from "bun:test";
import type { Device, Event } from "../types";
import { degradedDevices, degradedSentence, degradedSummary, gapPhrase, isNotAnAttempt } from "./degraded";

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
    for (const kind of ["nft_probe_failed", "nm_disconnect", "resolv_conf_reassert_failed", "dns_no_local_resolver"]) {
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
