// ============================================================================
// A person, as the console shows one: their face (a chosen emoji, else a
// monogram on one of the eight avatar pairs) inside the house-clock ring.
//
// The ring is <Ring>: time used today toward their target (their goal if
// they set one, else the limit plus anything earned), clockwise from the tick.
// No target → the empty track (no limit set). Paused → dashed, the face dims.
// ============================================================================
import { avatarColors } from "../lib/avatar";
import { Ring, ringGeometry } from "./Ring";

export function initials(name: string): string {
  const p = name.trim().split(/\s+/).filter(Boolean);
  if (!p.length) return "?";
  return (p.length === 1 ? p[0].slice(0, 2) : p[0][0] + p[p.length - 1][0]).toUpperCase();
}

/** The face alone — a disc on the person's avatar pair. */
export function Avatar({
  name,
  seed,
  avatar,
  size = 56,
  dim = false,
}: {
  name: string;
  seed: string;
  /** parent-picked emoji face; falls back to the deterministic monogram */
  avatar?: string | null;
  size?: number;
  dim?: boolean;
}) {
  const disc = avatarColors(seed);
  return (
    <span
      className="avatar"
      data-mono={!avatar}
      style={{
        width: size,
        height: size,
        fontSize: Math.round(size * (avatar ? 0.48 : 0.37)),
        background: disc.bg,
        color: disc.ink,
        opacity: dim ? 0.7 : undefined,
      }}
      aria-hidden="true"
    >
      {avatar || initials(name)}
    </span>
  );
}

export function AvatarRing({
  name,
  seed,
  avatar,
  used,
  target,
  left,
  paused = false,
  size = 64,
  on = "card",
}: {
  name: string;
  seed: string;
  avatar?: string | null;
  /** minutes used today */
  used: number;
  /** minutes the ring fills toward — goal if set, else the limit; null = no limit */
  target: number | null;
  /** minutes left, for the amber transition (≤ 15) */
  left?: number | null;
  paused?: boolean;
  size?: number;
  on?: "card" | "paper";
}) {
  const g = ringGeometry(size);
  // The face sits inside the ring with a hair of air around it.
  const inset = Math.round(g.c - (g.r - g.sw / 2) + Math.max(2, size * 0.03));
  const frac = target && target > 0 ? used / target : target === 0 ? 1 : null;
  return (
    <span className="avring" style={{ width: size, height: size }} aria-hidden="true">
      <Ring size={size} used={frac} minutesLeft={left} paused={paused} on={on} />
      <span className="avring-face" style={{ inset }}>
        <Avatar name={name} seed={seed} avatar={avatar} size={size - 2 * inset} dim={paused} />
      </span>
    </span>
  );
}
