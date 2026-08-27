// ============================================================================
// WhereTheTime — where today actually went (CONTRACT-0.6 §3).
//
// Three answers, in the order a person asks them:
//   apps   — catalog apps open on their machines, in minutes ("open", not
//            "focused" — the agent says what it can actually know);
//   sites  — the domains their computers talked to, as activity (a resolver
//            counts lookups, not seconds — the label is honest about it);
//   hours  — a 24-cell strip of when the day happened, in local time.
//
// This is the piece that replaces reading logs: the day as a picture.
// ============================================================================
import { useEffect, useMemo, useState } from "react";
import * as api from "../api";
import type { Catalog, WhereData } from "../types";
import { AppGlyph } from "./AppGlyph";

function fmtMin(secs: number): string {
  const m = Math.round(secs / 60);
  if (m < 1) return "<1 min";
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r === 0 ? `${h} h` : `${h} h ${r.toString().padStart(2, "0")}`;
}

export function WhereTheTime({ accountId }: { accountId?: string }) {
  const [data, setData] = useState<WhereData | null>(null);
  const [catalog, setCatalog] = useState<Catalog | null>(null);

  useEffect(() => {
    void api
      .getWhere(accountId)
      .then(setData)
      .catch(() => setData(null));
    void api.getCatalog().then(setCatalog).catch(() => setCatalog(null));
  }, [accountId]);

  const appName = useMemo(() => {
    const m = new Map<string, string>();
    for (const a of catalog?.apps ?? []) m.set(a.id, a.name);
    return m;
  }, [catalog]);

  // 24 local-hour buckets from the UTC hour rows.
  const hourCells = useMemo(() => {
    const cells = new Array<number>(24).fill(0);
    for (const h of data?.hours ?? []) {
      const local = new Date(h.hour).getHours();
      cells[local] += h.amount;
    }
    return cells;
  }, [data]);

  if (!data || (data.apps.length === 0 && data.sites.length === 0)) return null;
  const maxApp = Math.max(...data.apps.map((a) => a.seconds), 1);
  const maxSite = Math.max(...data.sites.map((s) => s.hits), 1);
  const maxHour = Math.max(...hourCells, 1);

  return (
    <section className="ch-section">
      <h2 className="ch-h2">Where the time went</h2>

      {hourCells.some((c) => c > 0) && (
        <div className="wt-hours" role="img" aria-label="Activity by hour of the day">
          {hourCells.map((c, i) => (
            <span key={i} className="wt-hour" title={`${i}:00`}>
              <span
                className="wt-hour-fill"
                style={{ opacity: c === 0 ? 0 : 0.25 + 0.75 * (c / maxHour) }}
              />
              {i % 6 === 0 && <span className="wt-hour-label">{i}</span>}
            </span>
          ))}
        </div>
      )}

      {data.apps.length > 0 && (
        <ul className="wt-list">
          {data.apps.slice(0, 6).map((a) => (
            <li key={a.key} className="wt-row">
              <AppGlyph id={a.key} name={appName.get(a.key) ?? a.key} size={26} />
              <span className="wt-name">{appName.get(a.key) ?? a.key}</span>
              <span className="wt-bar">
                <span className="wt-bar-fill" style={{ width: `${(a.seconds / maxApp) * 100}%` }} />
              </span>
              <span className="wt-amount">{fmtMin(a.seconds)}</span>
            </li>
          ))}
        </ul>
      )}
      {data.apps.length > 0 && (
        <p className="wt-note">Apps count while open on their computers — not what was in front.</p>
      )}

      {data.sites.length > 0 && (
        <ul className="wt-list wt-sites">
          {data.sites.slice(0, 6).map((s) => (
            <li key={s.key} className="wt-row">
              <span className="wt-name wt-site">{s.key}</span>
              <span className="wt-bar">
                <span className="wt-bar-fill" style={{ width: `${(s.hits / maxSite) * 100}%` }} />
              </span>
              <span className="wt-amount">{s.hits}×</span>
            </li>
          ))}
        </ul>
      )}
      {data.sites.length > 0 && (
        <p className="wt-note">
          Sites are the computer's network activity (lookups, not a stopwatch) — a shared machine
          shows everyone's.
        </p>
      )}
    </section>
  );
}
