// Small formatting helpers (relative time, minute/port rendering).

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

/** Days an offline device has been silent, if it crossed the "gone dark"
 * tamper threshold (offline for 7+ days) — null otherwise. A device that
 * briefly loses its network is merely "offline"; one silent for a week has
 * likely been wiped, blocked, or hidden. */
export function goneDarkDays(
  status: string,
  lastSeen: string | null,
): number | null {
  if (status !== "offline" || !lastSeen) return null;
  const then = new Date(lastSeen).getTime();
  if (Number.isNaN(then)) return null;
  const days = Math.floor((Date.now() - then) / 86_400_000);
  return days >= 7 ? days : null;
}

export function minutesToHm(mins: number): string {
  if (mins <= 0) return "0m";
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return [h ? `${h}h` : "", m ? `${m}m` : ""].filter(Boolean).join(" ") || "0m";
}

export function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

export const WEEKDAY_LABELS = ["S", "M", "T", "W", "T", "F", "S"];
