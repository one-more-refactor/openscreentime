// Small formatting helpers (relative time, minute/port rendering).

/** Server messages are lower-case fragments ("that code didn't match");
 * people read sentences. */
export function sentence(msg: string): string {
  const m = msg.trim();
  if (!m) return m;
  return m[0].toUpperCase() + m.slice(1) + (/[.!?…]$/.test(m) ? "" : ".");
}

export function relTime(iso: string | null): string {
  if (!iso) return "—";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "—";
  const diff = Date.now() - then;
  const s = Math.round(diff / 1000);
  if (s < 0) return "now";
  if (s < 60) return `${s}s`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h`;
  const d = Math.round(h / 24);
  if (d < 30) return `${d}d`;
  return new Date(iso).toLocaleDateString();
}

