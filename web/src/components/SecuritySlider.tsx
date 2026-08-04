// ============================================================================
// SECURITY SLIDER — one control that means something.
//
// The old console exposed every policy field at once, which is how a profile
// ended up allowing five domains, blocking VPNs while a VPN profile was active,
// and bricking itself after seven days offline. Nobody chose that combination;
// it was assembled one plausible checkbox at a time.
//
// So the levels below are whole, tested postures, not additive toggles. Each
// one is a complete answer to "how locked down is this child". Individual
// fields stay editable behind a disclosure, because sometimes you genuinely
// need one exception — but the default path cannot produce a broken device.
//
// Two invariants hold at EVERY level, deliberately, and are not editable here:
//   - inbound SSH stays open, so a device is always recoverable
//   - offline_lockdown_days stays 0, so a device that loses the server does
//     not lock a child out of a machine they need for school
// ============================================================================
import type { Policy } from "../types";

export interface SecurityLevel {
  id: number;
  name: string;
  /** One line, in a parent's words, not the system's. */
  summary: string;
  /** What actually changes, for the disclosure. */
  detail: string[];
}

export const LEVELS: SecurityLevel[] = [
  {
    id: 0,
    name: "Open",
    summary: "No filtering or time limits. You can still see what's going on.",
    detail: [
      "Every site allowed",
      "No screen-time limit",
      "Usage still reported, so the history keeps building",
    ],
  },
  {
    id: 1,
    name: "Guided",
    summary: "Blocks malware and forces SafeSearch. No time limit.",
    detail: [
      "Malware and phishing blocked at the resolver",
      "SafeSearch forced on Google, YouTube and Bing",
      "No screen-time limit",
    ],
  },
  {
    id: 2,
    name: "Protected",
    summary: "Adult content blocked, screen time on. The sensible default.",
    detail: [
      "Adult content and malware blocked (Cloudflare for Families)",
      "Proxies, torrent sites, gambling and stranger-chat blocked by name",
      "Screen time with a daily limit and a bedtime",
      "Encrypted-DNS bypasses (DoH/DoT) and Tor blocked",
    ],
  },
  {
    id: 3,
    name: "Strict",
    summary: "Everything in Protected, plus a tighter day and no workarounds.",
    detail: [
      "Everything in Protected",
      "Shorter daily limit and a narrower window",
      "Anonymising tools and VPN ports blocked",
    ],
  },
  {
    id: 4,
    name: "Allowlist",
    summary: "Only sites you approve. Expect to maintain the list.",
    detail: [
      "Nothing resolves unless it is on the list",
      "Breaks app stores, game launchers and system updates until added",
      "Best for short periods, not as a permanent setting",
    ],
  },
];

/** Domains that get blocked by name once filtering is on. */
const BLOCKLIST = [
  "croxyproxy.com", "proxysite.com", "kproxy.com", "hidester.com",
  "4everproxy.com", "whoer.net", "hide.me", "vpnbook.com",
  "thepiratebay.org", "1337x.to", "torrentz2.eu", "rarbg.to",
  "pornhub.com", "xvideos.com", "xnxx.com", "onlyfans.com",
  "stake.com", "bet365.com", "roobet.com",
  "omegle.com", "chatroulette.com",
];

/**
 * Turn a level into a complete policy, preserving the parts a level has no
 * business touching (the parent PIN, earn-time tasks, per-app limits).
 */
export function policyForLevel(level: number, base: Policy): Policy {
  const next: Policy = structuredClone(base);

  // Invariants. A level never removes the way back into a device.
  next.firewall = {
    ...next.firewall,
    mode: level >= 4 ? "default_deny" : "allow_all",
    allow_inbound_ports: [22],
    allow_outbound_ports: level >= 4 ? [53, 80, 443] : [],
  };
  next.lockdown = {
    ...next.lockdown,
    offline_lockdown_days: 0,
    force_dns: level >= 1,
    block_doh: level >= 2,
    block_dot: level >= 2,
    block_tor: level >= 2,
    // Left off below Strict: a parent-managed VPN profile is a supported
    // feature, and enabling both makes the agent kill its own tunnel.
    block_vpn: level >= 3,
  };

  next.dns = {
    ...next.dns,
    mode: level >= 4 ? "default_deny" : "allow_all",
    safe_search: level >= 1,
    // 1.1.1.1 no filtering · 1.1.1.2 malware · 1.1.1.3 malware + adult
    upstream: level === 0 ? "1.1.1.1" : level === 1 ? "1.1.1.2" : "1.1.1.3",
    blocklist: level >= 2 ? BLOCKLIST : [],
    allowlist: level >= 4 ? (base.dns.allowlist ?? []) : ["*"],
  };

  next.screen_time = {
    ...next.screen_time,
    enabled: level >= 2,
    daily_limit_minutes: level >= 3 ? 45 : level >= 2 ? 60 : 0,
    schedule:
      level >= 3
        ? [
            { days: [1, 2, 3, 4, 5], start: "15:00", end: "19:00" },
            { days: [0, 6], start: "09:00", end: "19:00" },
          ]
        : level >= 2
          ? [
              { days: [1, 2, 3, 4, 5], start: "07:00", end: "20:00" },
              { days: [0, 6], start: "09:00", end: "20:00" },
            ]
          : [],
    bedtime: level >= 2 ? { start: "20:00", end: "07:00" } : null,
  };

  return next;
}

/** Best-effort read of which level a policy currently matches. */
export function levelForPolicy(p: Policy): number {
  if (p.dns.mode === "default_deny") return 4;
  if (p.lockdown?.block_vpn) return 3;
  if (p.screen_time?.enabled) return 2;
  if (p.dns.safe_search) return 1;
  return 0;
}

export function SecuritySlider({
  value,
  onChange,
  busy,
}: {
  value: number;
  onChange: (level: number) => void;
  busy?: boolean;
}) {
  const level = LEVELS[Math.max(0, Math.min(LEVELS.length - 1, value))];
  return (
    <div className="sec">
      <div className="sec-head">
        <p className="sec-title">Protection</p>
        <p className="sec-level">{level.name}</p>
      </div>

      <input
        className="sec-range"
        type="range"
        min={0}
        max={LEVELS.length - 1}
        step={1}
        value={value}
        disabled={busy}
        onChange={(e) => onChange(Number(e.target.value))}
        aria-label="Protection level"
        aria-valuetext={level.name}
      />

      <div className="sec-ticks" aria-hidden="true">
        {LEVELS.map((l) => (
          <span key={l.id} className="sec-tick" data-on={l.id <= value}>
            {l.name}
          </span>
        ))}
      </div>

      <p className="sec-summary">{level.summary}</p>

      <details className="sec-more">
        <summary>What this changes</summary>
        <ul>
          {level.detail.map((d) => (
            <li key={d}>{d}</li>
          ))}
        </ul>
        <p className="sec-invariant">
          At every level, remote access stays open and a device never locks
          itself out when it can't reach the server.
        </p>
      </details>
    </div>
  );
}
