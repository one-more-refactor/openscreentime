// ============================================================================
// StateRing — a device's connection state as a ring, same circular language as
// the family avatars and the agent's unlock dial. The ring IS the data:
//   color   = state (ok / warn / blocked / idle)
//   arc     = connection freshness (full = heard just now, fading = silence)
//   dashed  = allowed to be away (sanctioned absence, not trouble)
// ============================================================================
import { useEffect, useState } from "react";

export type RingTone = "ok" | "warn" | "crit" | "idle";

interface Props {
  /** 0..1 — how much of the ring is lit (connection freshness). */
  arc: number;
  tone: RingTone;
  dashed?: boolean;
  /** one or two characters shown in the middle */
  label: string;
  size?: number;
}

const TONE_VAR: Record<RingTone, string> = {
  ok: "var(--ok)",
  warn: "var(--warn)",
  crit: "var(--accent)",
  idle: "var(--fg-faint)",
};

export function StateRing({ arc, tone, dashed = false, label, size = 44 }: Props) {
  const stroke = 2.5;
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const clamped = Math.max(0.06, Math.min(1, arc));
  // Draw in from zero on mount so the ring visibly fills to its value.
  const [drawn, setDrawn] = useState(false);
  useEffect(() => {
    const t = requestAnimationFrame(() => setDrawn(true));
    return () => cancelAnimationFrame(t);
  }, []);

  return (
    <span
      className="statering"
      style={{ width: size, height: size }}
      aria-hidden="true"
    >
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`}>
        {/* the track */}
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          fill="none"
          stroke="var(--line)"
          strokeWidth={stroke}
        />
        {/* the state arc, from 12 o'clock */}
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          fill="none"
          stroke={TONE_VAR[tone]}
          strokeWidth={stroke}
          strokeLinecap="round"
          strokeDasharray={dashed ? "3 5" : c}
          strokeDashoffset={dashed ? 0 : drawn ? c * (1 - clamped) : c}
          transform={`rotate(-90 ${size / 2} ${size / 2})`}
          style={
            dashed
              ? undefined
              : { transition: "stroke-dashoffset 600ms var(--ease)" }
          }
        />
      </svg>
      <span className="statering-label">{label}</span>
    </span>
  );
}
