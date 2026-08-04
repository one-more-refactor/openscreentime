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
import { useEffect, useState } from "react";
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
    name: "Off",
    summary: "Nothing is blocked and there is no time limit.",
    detail: ["Every website works", "No daily limit, no bedtime"],
  },
  {
    id: 1,
    name: "Safe search",
    summary: "Blocks dangerous sites and forces safe search. No time limit.",
    detail: [
      "Sites known for viruses and scams are blocked",
      "Google, YouTube and Bing are forced into their safe modes",
      "No daily limit, no bedtime",
    ],
  },
  {
    id: 2,
    name: "Protected",
    summary: "Adult sites blocked, 60 minutes a day, bedtime at 20:00.",
    detail: [
      "Adult sites, plus viruses and scams, are blocked",
      "Gambling, torrent and stranger-chat sites are blocked by name",
      "60 minutes a day · allowed 07:00–20:00 · bedtime 20:00–07:00",
      "The usual tricks for getting around a filter stop working",
    ],
  },
  {
    id: 3,
    name: "Strict",
    summary: "Same blocking, but 45 minutes a day and only 15:00–19:00.",
    detail: [
      "Everything Protected blocks",
      "45 minutes a day · allowed 15:00–19:00 on school days",
      "Apps that hide what a computer is doing are blocked too",
    ],
  },
  {
    id: 4,
    name: "Approved sites only",
    summary: "Everything is blocked except sites you add by hand.",
    detail: [
      "Only sites on your list will open — everything else stops",
      "App stores, game launchers and updates will break until you add them",
      "Meant for short periods, not for every day",
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
  // Dragging previews; nothing is written until "Apply". The old version fired
  // a policy write for every notch the thumb crossed, so sliding from Off to
  // Strict briefly applied two settings nobody chose — and a parent had no way
  // to see what they were about to do before it happened.
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const level = LEVELS[Math.max(0, Math.min(LEVELS.length - 1, draft))];
  const dirty = draft !== value;
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
        value={draft}
        disabled={busy}
        onChange={(e) => setDraft(Number(e.target.value))}
        aria-label="Protection level"
        aria-valuetext={level.name}
      />

      <div className="sec-ticks" aria-hidden="true">
        {LEVELS.map((l) => (
          <span key={l.id} className="sec-tick" data-on={l.id <= draft}>
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
          Whatever you choose, this computer can never lock you out of it — you
          can always get back in with the recovery PIN, even with no internet.
        </p>
      </details>

      {dirty && (
        <div className="sec-apply">
          <p className="sec-apply-note">
            Change from <strong>{LEVELS[value].name}</strong> to{" "}
            <strong>{level.name}</strong>?
          </p>
          <button className="ch-btn" onClick={() => setDraft(value)} disabled={busy}>
            Cancel
          </button>
          <button
            className="ch-btn ch-btn-yes"
            onClick={() => onChange(draft)}
            disabled={busy}
          >
            {busy ? "Applying…" : "Apply"}
          </button>
        </div>
      )}
    </div>
  );
}
