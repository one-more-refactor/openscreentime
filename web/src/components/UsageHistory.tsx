import { useState } from "react";
import { getUsageHistory } from "../api";
import { useAsync } from "../lib/useAsync";
import type { UsageDay } from "../types";

/** Per-day screen-time history for one device user: stacked used+earned bars,
 *  7/30-day range, streak tile. Colors follow the entities established by the
 *  today-bar: used = fg-dim ink, earned = ok green — with a legend and a
 *  per-bar tooltip so identity never rides on color alone. */
export function UsageHistory({ deviceUserId }: { deviceUserId: string }) {
  const [days, setDays] = useState<7 | 30>(30);
  const hist = useAsync(() => getUsageHistory(deviceUserId, days), [deviceUserId, days]);
  const [hover, setHover] = useState<number | null>(null);

  if (hist.loading && !hist.data) {
    return <p className="label text-muted mt-2">HISTORY…</p>;
  }
  if (hist.error || !hist.data) {
    return <p className="label text-muted mt-2">HISTORY UNAVAILABLE</p>;
  }

  // Fill calendar gaps so a quiet day renders as an empty slot, not a missing bar.
  const byDay = new Map(hist.data.days.map((d) => [d.day, d]));
  const series: UsageDay[] = [];
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date();
    d.setDate(d.getDate() - i);
    const key = d.toISOString().slice(0, 10);
    series.push(byDay.get(key) ?? { day: key, used_minutes: 0, earned_minutes: 0 });
  }
  const max = Math.max(...series.map((d) => d.used_minutes + d.earned_minutes), 60);
  const totalUsed = series.reduce((a, d) => a + d.used_minutes, 0);
  const H = 72;

  const hm = (m: number) => (m >= 60 ? `${Math.floor(m / 60)}H ${m % 60}M` : `${m}M`);
  const hovered = hover !== null ? series[hover] : null;

  return (
    <div className="mt-3">
      <div className="flex items-center gap-3 mb-2">
        <span className="label text-muted">LAST {days} DAYS</span>
        <button
          type="button"
          className="label underline-offset-2 hover:underline"
          onClick={() => setDays(days === 30 ? 7 : 30)}
        >
          SHOW {days === 30 ? "7" : "30"}
        </button>
        <span className="label tabular-nums ml-auto" style={{ color: "var(--fg-dim)" }}>
          {hm(totalUsed)} TOTAL
          {hist.data.streak_days > 0 ? ` · STREAK ${hist.data.streak_days}D` : ""}
        </span>
      </div>
      <div
        className="flex items-end gap-[2px]"
        style={{ height: H }}
        role="img"
        aria-label={`Screen time per day over the last ${days} days`}
        onMouseLeave={() => setHover(null)}
      >
        {series.map((d, i) => {
          const uh = Math.round((d.used_minutes / max) * (H - 4));
          const eh = Math.round((d.earned_minutes / max) * (H - 4));
          return (
            <div
              key={d.day}
              className="flex-1 flex flex-col justify-end h-full cursor-default"
              onMouseEnter={() => setHover(i)}
            >
              <span
                style={{
                  height: eh,
                  background: "var(--ok)",
                  borderRadius: "1px 1px 0 0",
                  marginBottom: eh > 0 && uh > 0 ? 2 : 0,
                  opacity: hover === null || hover === i ? 1 : 0.35,
                }}
              />
              <span
                style={{
                  height: Math.max(uh, d.used_minutes > 0 ? 2 : 0),
                  background: "var(--fg-dim)",
                  borderRadius: uh > 0 && eh === 0 ? "1px 1px 0 0" : 0,
                  opacity: hover === null || hover === i ? 1 : 0.35,
                }}
              />
              <span className="h-px flex-none" style={{ background: "var(--line)" }} />
            </div>
          );
        })}
      </div>
      <div className="flex items-center gap-4 mt-2 min-h-[1rem]">
        <span className="label text-muted flex items-center gap-1.5">
          <i className="w-2 h-2 rounded-[1px] inline-block" style={{ background: "var(--fg-dim)" }} />
          USED
        </span>
        <span className="label text-muted flex items-center gap-1.5">
          <i className="w-2 h-2 rounded-[1px] inline-block" style={{ background: "var(--ok)" }} />
          EARNED
        </span>
        {hovered && (
          <span className="label tabular-nums ml-auto">
            {hovered.day.slice(5).replace("-", "/")} · {hm(hovered.used_minutes)} USED
            {hovered.earned_minutes > 0 ? ` · +${hm(hovered.earned_minutes)} EARNED` : ""}
          </span>
        )}
      </div>
    </div>
  );
}
