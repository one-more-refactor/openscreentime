// The two ways the console says time (docs/PRODUCT.md §4), plus the one
// sentence helper. Everything a person reads goes through these.

/** Server messages are lower-case fragments ("that code didn't match");
 * people read sentences. */
export function sentence(msg: string): string {
  const m = msg.trim();
  if (!m) return m;
  return m[0].toUpperCase() + m.slice(1) + (/[.!?…]$/.test(m) ? "" : ".");
}

/** An amount of time: "45 min", "1 h 30 min", "2 h". */
export function duration(mins: number): string {
  const m = Math.max(0, Math.round(mins));
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r ? `${h} h ${r} min` : `${h} h`;
}

/** The short form for inside a ring or a tight row: "27", "1 h 12". */
export function durationShort(mins: number): string {
  const m = Math.max(0, Math.round(mins));
  if (m < 60) return `${m}`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r ? `${h} h ${String(r).padStart(2, "0")}` : `${h} h`;
}

/** How long ago: "just now", "20 min ago", "3 h ago", "2 days ago". */
export function ago(iso: string | null | undefined): string {
  if (!iso) return "never";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "never";
  const mins = Math.round((Date.now() - then) / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins} min ago`;
  const h = Math.round(mins / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  if (d < 30) return `${d} ${d === 1 ? "day" : "days"} ago`;
  return new Date(iso).toLocaleDateString();
}
