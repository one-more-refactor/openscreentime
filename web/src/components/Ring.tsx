// ============================================================================
// Ring — the house clock. One primitive, one meaning, everywhere
// (brand/board.html § "What the ring means"):
//
//   the arc   = time USED today, from the tick at twelve, clockwise
//   the tick  = the start line of the day (drawn on rings of 40 px and up)
//   green     = a normal day; amber once 15 minutes or less are left;
//               red only when the ring is full — time's up
//   dashed    = paused (a parent's pause is calm, never red)
//   empty     = no limit set: just the track and the tick
//
// Never a spinner, a countdown, a hold-to-confirm, a code entry or a
// connection gauge. Those are a bar, a plain number, or nothing.
//
// Geometry is the board's (brand/build.py `ring()`), so a 64 px ring here is
// the same drawing as the 64 px ring on the board.
// ============================================================================
import type { ReactNode } from "react";

export type RingTone = "brand" | "warn" | "stop";

export interface RingGeometry {
  /** centre (x = y) */
  c: number;
  r: number;
  /** stroke width */
  sw: number;
  hasTick: boolean;
  tickW: number;
  tickLen: number;
}

const round1 = (n: number) => Math.round(n * 10) / 10;

/** The board's ring geometry for a diameter. */
export function ringGeometry(size: number, tick = true): RingGeometry {
  const sw = Math.max(2, Math.round(size * (size >= 120 ? 0.09 : size >= 48 ? 0.08 : 0.07)));
  const hasTick = tick && size >= 40;
  const r = size / 2 - sw / 2 - (hasTick ? sw * 0.35 : 0);
  return { c: size / 2, r, sw, hasTick, tickW: round1(sw * 0.43), tickLen: round1(sw * 1.6) };
}

/** A point on the ring `frac` of the way round, clockwise from twelve. */
export function ringPoint(c: number, r: number, frac: number): [number, number] {
  const a = -Math.PI / 2 + 2 * Math.PI * frac;
  return [c + r * Math.cos(a), c + r * Math.sin(a)];
}

/** The arc for `frac` (0 < frac < 1): starts at the tick, sweeps clockwise. */
export function arcPath(c: number, r: number, frac: number): string {
  const [sx, sy] = ringPoint(c, r, 0);
  const [ex, ey] = ringPoint(c, r, frac);
  const large = frac > 0.5 ? 1 : 0;
  const f = (n: number) => n.toFixed(2);
  return `M${f(sx)} ${f(sy)}A${f(r)} ${f(r)} 0 ${large} 1 ${f(ex)} ${f(ey)}`;
}

/** Green, then amber at 15 minutes or less, red only when the ring is full. */
export function ringTone(used: number, minutesLeft?: number | null): RingTone {
  if (used >= 1) return "stop";
  if (minutesLeft != null && minutesLeft <= 15) return "warn";
  return "brand";
}

const TONE: Record<RingTone, string> = {
  brand: "var(--brand)",
  warn: "var(--warn)",
  stop: "var(--stop)",
};

export interface RingProps {
  /** Diameter in px. */
  size: number;
  /** Fraction of today's time used, 0–1. `null` = no limit (an empty track). */
  used: number | null;
  /** Minutes left — only for the amber transition at 15 minutes or less. */
  minutesLeft?: number | null;
  /** A parent's pause: a dashed ring, no fill, no red. */
  paused?: boolean;
  /** What it sits on: a card (white) or the paper — picks the track colour. */
  on?: "card" | "paper";
  /** Force a tone (the board's brand mark uses this); normally derived. */
  tone?: RingTone;
  /** Draw the tick (only ever drawn at 40 px and up). */
  tick?: boolean;
  /** Spoken description; without one the ring is decoration (aria-hidden). */
  label?: string;
  className?: string;
  /** Content centred inside the ring — see <RingNumber>. */
  children?: ReactNode;
}

export function Ring({
  size,
  used,
  minutesLeft,
  paused = false,
  on = "card",
  tone,
  tick = true,
  label,
  className = "",
  children,
}: RingProps) {
  const g = ringGeometry(size, tick);
  const track = on === "paper" ? "var(--line-2)" : "var(--line)";
  // Zero minutes left is a full ring, whatever the rounding says.
  const frac = used == null ? 0 : minutesLeft === 0 ? 1 : Math.max(0, Math.min(1, used));
  const colour = TONE[tone ?? ringTone(frac, minutesLeft)];
  const circ = 2 * Math.PI * g.r;
  const state = paused ? "paused" : used == null ? "none" : frac >= 0.999 ? "full" : frac > 0 ? "fill" : "empty";
  const end = ringPoint(g.c, g.r, frac);

  return (
    <span
      // Not "ring": Tailwind owns that utility name (a blue focus ring).
      className={`time-ring ${className}`}
      style={{ width: size, height: size }}
      data-state={state}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden="true">
        {paused ? (
          <circle
            className="ring-dash"
            cx={g.c}
            cy={g.c}
            r={g.r}
            fill="none"
            stroke="var(--ink-3)"
            strokeWidth={g.sw}
            strokeLinecap="round"
            strokeDasharray={`${(circ * 0.012).toFixed(2)} ${(circ * 0.055).toFixed(2)}`}
          />
        ) : (
          <>
            <circle className="ring-track" cx={g.c} cy={g.c} r={g.r} fill="none" stroke={track} strokeWidth={g.sw} />
            {state === "full" && (
              <circle className="ring-full" cx={g.c} cy={g.c} r={g.r} fill="none" stroke={colour} strokeWidth={g.sw} />
            )}
            {state === "fill" && (
              <>
                <path
                  className="ring-arc"
                  d={arcPath(g.c, g.r, frac)}
                  pathLength={1}
                  fill="none"
                  stroke={colour}
                  strokeWidth={g.sw}
                />
                <circle className="ring-cap" cx={end[0].toFixed(2)} cy={end[1].toFixed(2)} r={g.sw / 2} fill={colour} />
              </>
            )}
          </>
        )}
        {g.hasTick && (
          <path
            className="ring-tick"
            d={`M${g.c} ${(g.c - g.r - g.tickLen / 2).toFixed(2)}v${g.tickLen}`}
            stroke="var(--ink)"
            strokeWidth={g.tickW}
            strokeLinecap="round"
            fill="none"
          />
        )}
      </svg>
      {children != null && <span className="ring-in">{children}</span>}
    </span>
  );
}

/**
 * A number inside a ring, sized to fit the inner circle: up to three
 * characters at 0.29 × the diameter, anything longer ("1 h 12") at 0.2 ×.
 * Never abbreviate to "1:12".
 */
export function RingNumber({ size, value, unit }: { size: number; value: string; unit?: string }) {
  const long = value.replace(/\s/g, "").length > 3;
  return (
    <>
      <b className="ring-num num" style={{ fontSize: Math.round(size * (long ? 0.2 : 0.29)) }}>
        {value}
      </b>
      {unit && (
        <span className="ring-unit" style={{ fontSize: Math.max(12.5, Math.round(size * 0.09)) }}>
          {unit}
        </span>
      )}
    </>
  );
}
