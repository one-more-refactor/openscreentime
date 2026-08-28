// ============================================================================
// AvatarRing — the family's glance (CONTRACT-0.6 "ring grid"). Identity lives
// in the center (emoji or monogram); STATE lives in the ring around it, so a
// two-second scan of round faces reads the whole household:
//   ring fill  = how much of today's target is spent (goal if set, else limit)
//   ring color = ok on a normal day; the accent only when spent/over
//   dashed+dim = paused (a sanctioned "away" state, same grammar as StateRing)
// No target (no goal, no limit) → a plain identity disc, no ring.
// ============================================================================
import { useEffect, useState } from "react";

function hueFor(seed: string): number {
  let h = 0;
  for (const ch of seed) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return h;
}
function initials(name: string): string {
  const p = name.trim().split(/\s+/).filter(Boolean);
  if (!p.length) return "?";
  return (p.length === 1 ? p[0].slice(0, 2) : p[0][0] + p[p.length - 1][0]).toUpperCase();
}

export function AvatarRing({
  name,
  seed,
  avatar,
  used,
  target,
  paused = false,
  size = 56,
}: {
  name: string;
  seed: string;
  avatar?: string | null;
  used: number;
  /** minutes the ring fills toward — goal if set, else the limit; null = none */
  target: number | null;
  paused?: boolean;
  size?: number;
}) {
  const hue = hueFor(seed);
  const stroke = Math.max(3, Math.round(size * 0.07));
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const pct = target && target > 0 ? Math.min(1, used / target) : 0;
  const over = target != null && target > 0 && used >= target;

  const [drawn, setDrawn] = useState(false);
  useEffect(() => {
    const t = requestAnimationFrame(() => setDrawn(true));
    return () => cancelAnimationFrame(t);
  }, []);

  const ringColor = paused ? "var(--fg-faint)" : over ? "var(--accent)" : "var(--ok)";

  return (
    <span
      className="avring"
      style={{ width: size, height: size, position: "relative", display: "inline-block", flex: "none" }}
      aria-hidden="true"
    >
      {target != null && (
        <svg
          width={size}
          height={size}
          viewBox={`0 0 ${size} ${size}`}
          style={{ position: "absolute", inset: 0 }}
        >
          <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="var(--line)" strokeWidth={stroke} />
          <circle
            cx={size / 2}
            cy={size / 2}
            r={r}
            fill="none"
            stroke={ringColor}
            strokeWidth={stroke}
            strokeLinecap="round"
            strokeDasharray={paused ? `${c * 0.02} ${c * 0.04}` : c}
            strokeDashoffset={paused ? 0 : drawn ? c * (1 - pct) : c}
            transform={`rotate(-90 ${size / 2} ${size / 2})`}
            style={{ transition: "stroke-dashoffset 700ms cubic-bezier(0.25,0.1,0.25,1)" }}
          />
        </svg>
      )}
      <span
        className="fam-avatar"
        style={{
          position: "absolute",
          // Inset the identity disc inside the ring.
          inset: target != null ? stroke + 2 : 0,
          width: "auto",
          height: "auto",
          fontSize: avatar ? size * 0.4 : size * 0.3,
          background: `hsl(${hue} 45% 88%)`,
          color: `hsl(${hue} 55% 26%)`,
          opacity: paused ? 0.6 : 1,
        }}
      >
        {avatar || initials(name)}
      </span>
    </span>
  );
}
