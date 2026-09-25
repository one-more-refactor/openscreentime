// ============================================================================
// WhereTheTime — where today actually went (CONTRACT-0.6 §3).
//
// Three answers, in the order a person asks them:
//   when   — a 24-cell strip of the day, in local time;
//   apps   — apps open on their computers (a catalog id or a desktop app's
//            name, "Text Editor"), in minutes ("open", not
//            "in front" — the agent says what it can actually know);
//   sites  — the sites their computers looked up, as activity (a resolver
//            counts lookups, not seconds — the label is honest about it).
//
// What shows is exactly what the server allows for who is looking: the
// person sees all of their own; a parent sees a child's apps and sites, an
// older teen's apps only, and nothing of an adult's (the page never asks).
// ============================================================================
import { useEffect, useMemo, useState } from "react";
import * as api from "../api";
import type { Catalog, WhereData } from "../types";
import { AppGlyph } from "./AppGlyph";
import { duration } from "../lib/format";
import { wallTime } from "../lib/day";

function minutes(secs: number): string {
  const m = Math.round(secs / 60);
  return m < 1 ? "<1 min" : duration(m);
}

export function WhereTheTime({
  accountId,
  who,
  offsetSecs,
}: {
  accountId?: string;
  who: "they" | "you";
  /** the computer's clock (seconds east of UTC): the day's hours are its hours */
  offsetSecs?: number | null;
}) {
  const [data, setData] = useState<WhereData | null>(null);
  const [failed, setFailed] = useState(false);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const their = who === "they" ? "their" : "your";

  useEffect(() => {
    let alive = true;
    void api
      .getWhere(accountId)
      .then((d) => alive && setData(d))
      .catch(() => alive && setFailed(true));
    void api
      .getCatalog()
      .then((c) => alive && setCatalog(c))
      .catch(() => alive && setCatalog(null));
    return () => {
      alive = false;
    };
  }, [accountId]);

  const appName = useMemo(() => {
    const m = new Map<string, string>();
    for (const a of catalog?.apps ?? []) m.set(a.id, a.name);
    return m;
  }, [catalog]);

  // 24 hour buckets from the UTC hour rows, on the computer's clock.
  const hourCells = useMemo(() => {
    const cells = new Array<number>(24).fill(0);
    for (const h of data?.hours ?? []) {
      const ms = Date.parse(h.hour);
      if (!Number.isNaN(ms)) cells[Math.floor(wallTime(ms, offsetSecs).minute / 60)] += h.amount;
    }
    return cells;
  }, [data, offsetSecs]);

  // Nothing the server lets us show (or it said no): say nothing at all.
  if (failed) return null;
  const empty = !!data && data.apps.length === 0 && data.sites.length === 0;
  const maxApp = Math.max(...(data?.apps.map((a) => a.seconds) ?? []), 1);
  const maxSite = Math.max(...(data?.sites.map((s) => s.hits) ?? []), 1);
  const maxHour = Math.max(...hourCells, 1);

  return (
    <section className="section">
      <div className="section-head">
        <h2 className="h2">Where the time went</h2>
      </div>
      <div className="card card-pad wt">
        {!data ? (
          <span className="wait" style={{ height: 48 }} aria-label="Loading" />
        ) : empty ? (
          <p className="meta">Nothing yet today.</p>
        ) : (
          <>
            {hourCells.some((c) => c > 0) && (
              <div className="wt-hours" role="img" aria-label="When the day happened, hour by hour">
                {hourCells.map((c, i) => (
                  <span key={i} className="wt-hour">
                    <span className="wt-hour-fill" style={{ opacity: c === 0 ? 0 : 0.25 + 0.75 * (c / maxHour) }} />
                    {i % 6 === 0 && <span className="wt-hour-label num">{i}:00</span>}
                  </span>
                ))}
              </div>
            )}

            <div className="wt-cols">
              {data.apps.length > 0 && (
                <div>
                  <h3 className="wt-h">Apps</h3>
                  <ul className="wt-list">
                    {data.apps.slice(0, 6).map((a) => (
                      <li key={a.key} className="wt-row">
                        <AppGlyph id={a.key} name={appName.get(a.key) ?? a.key} size={26} />
                        <span className="wt-name">{appName.get(a.key) ?? a.key}</span>
                        <span className="wt-bar">
                          <span className="wt-bar-fill" style={{ width: `${(a.seconds / maxApp) * 100}%` }} />
                        </span>
                        <span className="wt-amount num">{minutes(a.seconds)}</span>
                      </li>
                    ))}
                  </ul>
                  <p className="wt-note">Apps count while they're open on {their} computers — not what was in front.</p>
                </div>
              )}

              {data.sites.length > 0 && (
                <div>
                  <h3 className="wt-h">Sites</h3>
                  <ul className="wt-list">
                    {data.sites.slice(0, 6).map((s) => (
                      <li key={s.key} className="wt-row">
                        <span className="wt-name wt-site">{s.key}</span>
                        <span className="wt-bar">
                          <span className="wt-bar-fill" style={{ width: `${(s.hits / maxSite) * 100}%` }} />
                        </span>
                        <span className="wt-amount num">{s.hits}×</span>
                      </li>
                    ))}
                  </ul>
                  <p className="wt-note">How often the computer looked a site up — activity, not a stopwatch.</p>
                </div>
              )}
            </div>

            {data.sites_hidden_shared && (
              <p className="wt-note">Sites aren't shown for a shared computer — they'd be everyone's, not just {their}s.</p>
            )}
            {data.sites_hidden_age && (
              <p className="wt-note">Sites aren't shown at their age — their browsing is their own now.</p>
            )}
          </>
        )}
      </div>
    </section>
  );
}
