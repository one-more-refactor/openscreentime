// ============================================================================
// ME — a person's own page. The console a child sees when they open
// OpenScreenTime on their own computer (a member session can reach nothing
// else), and what a parent sees under "My screen time" for themselves.
//
// One question, answered at a glance: how much time do I have left today?
// Then, in order: ask for more (if their bracket may), what's blocked, when
// screens are off, which computers count. Nothing else.
//
// Three looks, keyed by the person's effective theme and scoped by a class on
// the page root so the rest of the console is untouched (me.css):
//   playful — little/kid: one huge friendly ring, chunky rounded type, warm
//             bright palette. Duolingo energy without a mascot.
//   calm    — teens: a quieter ring, a stats row, goals, blocked as a list.
//   plain   — adults: a compact private dashboard; no ring, no asking.
// Enforcement words stay plain in all three: when it's stopped, it says so.
// ============================================================================
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import * as api from "../api";
import type { Catalog, MeHistory, MeToday, Theme } from "../types";
import { useSession } from "../lib/session";
import { useCountUp } from "../lib/useCountUp";
import { useTheme } from "../lib/theme";
import { Wordmark } from "../components/Wordmark";
import { WhereTheTime } from "../components/WhereTheTime";

// ---- the ring ----------------------------------------------------------------

function Ring({
  pct,
  size,
  stroke,
  children,
  spent,
}: {
  /** 0..1 of the day still available */
  pct: number;
  size: number;
  stroke: number;
  spent: boolean;
  children: ReactNode;
}) {
  // Fills from empty on mount — the page arrives, the day draws itself.
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    const t = requestAnimationFrame(() => setMounted(true));
    return () => cancelAnimationFrame(t);
  }, []);
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const offset = mounted ? c * (1 - Math.max(0, Math.min(1, pct))) : c;
  return (
    <div className="me-ring" style={{ width: size, height: size }} data-spent={spent}>
      <svg viewBox={`0 0 ${size} ${size}`} width={size} height={size} aria-hidden="true">
        <circle className="me-ring-track" cx={size / 2} cy={size / 2} r={r} strokeWidth={stroke} fill="none" />
        <circle
          className="me-ring-fill"
          cx={size / 2}
          cy={size / 2}
          r={r}
          strokeWidth={stroke}
          fill="none"
          strokeLinecap="round"
          strokeDasharray={c}
          strokeDashoffset={offset}
          transform={`rotate(-90 ${size / 2} ${size / 2})`}
        />
      </svg>
      <div className="me-ring-inner">{children}</div>
    </div>
  );
}

function fmt(m: number): string {
  if (m < 60) return `${m}`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r === 0 ? `${h}h` : `${h}h ${String(r).padStart(2, "0")}`;
}
function unitFor(m: number): string {
  return m < 60 ? (m === 1 ? "minute" : "minutes") : "";
}

// ---- shared pieces ---------------------------------------------------------------

function AskForTime({
  today,
  theme,
  onAsked,
}: {
  today: MeToday;
  theme: Theme;
  onAsked: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  // Little ones have no request UI; adults have no one to ask.
  if (today.bracket === "little" || today.bracket === "adult") return null;

  if (today.pending_request)
    return (
      <p className="me-asked">
        {theme === "playful" ? "You asked. A parent will answer soon." : "Asked — waiting for a parent."}
      </p>
    );

  async function ask(minutes: number) {
    setBusy(true);
    setErr(null);
    try {
      await api.askForTime(minutes);
      onAsked();
    } catch (e) {
      setErr(e instanceof Error ? e.message : "That didn't go through. Try again.");
    } finally {
      setBusy(false);
    }
  }

  if (theme === "playful")
    return (
      <div className="me-ask">
        <button className="me-ask-big" disabled={busy} onClick={() => void ask(15)}>
          {busy ? "Asking…" : "Ask for 15 more minutes"}
        </button>
        {err && <p className="me-err">{err}</p>}
      </div>
    );

  return (
    <div className="me-ask">
      <p className="me-ask-label">Ask for more time</p>
      <div className="me-ask-row">
        {[15, 30, 60].map((m) => (
          <button key={m} className="me-ask-pill" disabled={busy} onClick={() => void ask(m)}>
            +{m} min
          </button>
        ))}
      </div>
      {err && <p className="me-err">{err}</p>}
    </div>
  );
}

/** Which apps are blocked, as names — the catalog gives us the words. */
function useBlockedNames(today: MeToday | null, catalog: Catalog | null) {
  return useMemo(() => {
    if (!today || !catalog) return { apps: [] as { id: string; name: string }[], cats: [] as string[], sites: [] as string[] };
    const catSet = new Set(today.blocks.categories);
    const viaCat = new Set(catalog.apps.filter((a) => catSet.has(a.category)).map((a) => a.id));
    const apps = catalog.apps
      .filter((a) => today.blocks.apps.includes(a.id) && !viaCat.has(a.id))
      .map((a) => ({ id: a.id, name: a.name }));
    const cats = catalog.categories.filter((c) => catSet.has(c.id)).map((c) => c.name);
    return { apps, cats, sites: today.blocks.custom_domains };
  }, [today, catalog]);
}

function Blocked({ today, catalog, theme }: { today: MeToday; catalog: Catalog | null; theme: Theme }) {
  const b = useBlockedNames(today, catalog);
  const nothing = b.apps.length === 0 && b.cats.length === 0 && b.sites.length === 0;
  if (nothing) return null;
  const title =
    theme === "playful" ? "Not on this computer" : theme === "calm" ? "Blocked" : "What you've blocked";
  return (
    <section className="me-section">
      <h2 className="me-h2">{title}</h2>
      <div className="me-blocked">
        {b.cats.map((c) => (
          <span key={c} className="me-blocked-item" data-kind="cat">{c}</span>
        ))}
        {b.apps.map((a) => (
          <span key={a.id} className="me-blocked-item">{a.name}</span>
        ))}
        {b.sites.map((s) => (
          <span key={s} className="me-blocked-item" data-kind="site">{s}</span>
        ))}
      </div>
    </section>
  );
}

// ---- first visit ---------------------------------------------------------------
// The transparency intro, in the console (CONTRACT-0.6 §3): the first time a
// member opens their page, it says plainly what a parent can and cannot see.
// Shown once per browser; the full version lives in docs/TRANSPARENCY.md and
// as the device's first-run intro.

function FirstVisit({ theme }: { theme: Theme }) {
  const KEY = "ost-intro-seen";
  const [seen, setSeen] = useState(() => {
    try {
      return localStorage.getItem(KEY) === "1";
    } catch {
      return true;
    }
  });
  if (seen) return null;
  const dismiss = () => {
    try {
      localStorage.setItem(KEY, "1");
    } catch {
      /* private mode: show again next time, no harm */
    }
    setSeen(true);
  };
  return (
    <section className="me-section me-intro">
      <h2 className="me-h2">{theme === "playful" ? "What this is" : "Before anything else"}</h2>
      <p className="me-intro-p">
        {theme === "playful"
          ? "Your grown-ups can see how long you've been on the computer and which apps were open — like a clock, not a camera."
          : "Your parents can see: your minutes, which apps and sites your computer used, and the moments the rules kicked in."}
      </p>
      <p className="me-intro-p">
        {theme === "playful"
          ? "They can NOT read your messages, see your screen, or watch what you type. Ever."
          : "They can NOT read messages, see your screen, record keystrokes, or open a remote shell — that last one doesn't even exist in this software."}
      </p>
      <button className="me-link" onClick={dismiss}>
        Okay, got it
      </button>
    </section>
  );
}

// ---- the week ----------------------------------------------------------------
// "Know what you did" is the floor of any motivation: seven bars, today
// telling you how it compares to your usual, and where today's minutes went.

function fmtLong(m: number): string {
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r === 0 ? `${h} h` : `${h} h ${r.toString().padStart(2, "0")}`;
}

function Week({
  history,
  today,
  theme,
}: {
  history: MeHistory;
  today: MeToday;
  theme: Theme;
}) {
  // The last 7 calendar days, today last, missing days as zero.
  const byDay = new Map(history.days.map((d) => [d.day, d]));
  const week = Array.from({ length: 7 }, (_, i) => {
    const d = new Date();
    d.setDate(d.getDate() - (6 - i));
    const key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
    return {
      key,
      letter: d.toLocaleDateString(undefined, { weekday: "short" }).slice(0, 2),
      used: byDay.get(key)?.used_minutes ?? 0,
      isToday: i === 6,
    };
  });
  // Live truth beats a possibly momentarily-stale history row for today.
  week[6].used = Math.max(week[6].used, today.used_minutes);

  const limit = today.limit_minutes !== null ? today.limit_minutes + today.earned_minutes : null;
  const max = Math.max(...week.map((d) => d.used), limit ?? 0, 30);

  // "How does today compare to my usual?" — the average of the earlier days
  // in the strip that saw any use. Fewer than three and we stay quiet.
  const prior = week.slice(0, 6).filter((d) => d.used > 0);
  const avg = prior.length >= 3 ? Math.round(prior.reduce((s, d) => s + d.used, 0) / prior.length) : null;
  const delta = avg !== null ? week[6].used - avg : null;
  const compare =
    delta === null
      ? null
      : Math.abs(delta) < 10
        ? "Right around your usual so far."
        : delta < 0
          ? `${fmtLong(-delta)} less than your usual day — nice.`
          : `${fmtLong(delta)} more than your usual day.`;

  const devices = history.today_by_device;

  return (
    <section className="me-section me-week-wrap">
      <h2 className="me-h2">{theme === "playful" ? "Your week" : "This week"}</h2>
      <div className="me-week" role="img" aria-label="Screen time, last seven days">
        {week.map((d) => (
          <div key={d.key} className="me-week-day" data-today={d.isToday}>
            <div className="me-week-bar">
              {limit !== null && (
                <span className="me-week-limit" style={{ bottom: `${(limit / max) * 100}%` }} />
              )}
              <span
                className="me-week-fill"
                data-over={limit !== null && d.used > limit}
                style={{ height: `${Math.max(4, (d.used / max) * 100)}%` }}
              />
            </div>
            <span className="me-week-min">{d.used > 0 ? fmt(d.used) : "·"}</span>
            <span className="me-week-label">{d.letter}</span>
          </div>
        ))}
      </div>
      {compare && <p className="me-week-compare">{compare}</p>}
      {devices.length > 0 && (
        <ul className="me-where">
          {devices.map((d) => (
            <li key={d.name} className="me-where-row">
              <span className="me-where-name">{d.name}</span>
              <span className="me-where-min">{fmtLong(d.used_minutes)}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function Schedule({ today, theme }: { today: MeToday; theme: Theme }) {
  if (!today.bedtime && today.windows.length === 0) return null;
  const w = today.windows;
  return (
    <section className="me-section me-when">
      {today.bedtime && (
        <div className="me-card me-card-night">
          <span className="me-moon" aria-hidden="true">
            <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor">
              <path d="M21 14.5A8.5 8.5 0 0 1 9.5 3a7 7 0 1 0 11.5 11.5Z" />
            </svg>
          </span>
          <div>
            <p className="me-card-title">{theme === "playful" ? "Bedtime" : "Screens off"}</p>
            <p className="me-card-value">{today.bedtime.start} – {today.bedtime.end}</p>
          </div>
        </div>
      )}
      {w.length > 0 && (
        <div className="me-card">
          <span className="me-sun" aria-hidden="true">
            <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor">
              <circle cx="12" cy="12" r="5" />
              <path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
            </svg>
          </span>
          <div>
            <p className="me-card-title">{theme === "playful" ? "Screen time is" : "Allowed hours"}</p>
            <p className="me-card-value">
              {w.map((x, i) => (
                <span key={i}>
                  {i > 0 && " · "}
                  {x.days.includes(1) && x.days.includes(5) ? "school days" : x.days.includes(0) || x.days.includes(6) ? "weekend" : "days"}{" "}
                  {x.start}–{x.end}
                </span>
              ))}
            </p>
          </div>
        </div>
      )}
    </section>
  );
}

function Devices({ today, theme }: { today: MeToday; theme: Theme }) {
  if (today.devices.length === 0) return null;
  return (
    <section className="me-section">
      <h2 className="me-h2">{theme === "playful" ? "Your computers" : "Devices"}</h2>
      <ul className="me-devices">
        {today.devices.map((d) => (
          <li key={d.name} className="me-device">
            <span className="me-device-dot" data-state={d.status} aria-hidden="true" />
            <span className="me-device-name">{d.name}</span>
            <span className="me-device-state">
              {d.locked ? "paused" : d.status === "online" ? "on" : d.status === "pending" ? "not set up" : "off"}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

// ---- the page ----------------------------------------------------------------------

export function Me() {
  const { me, logout } = useSession();
  const navigate = useNavigate();
  const { theme: mode } = useTheme();
  const [today, setToday] = useState<MeToday | null>(null);
  const [history, setHistory] = useState<MeHistory | null>(null);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function load() {
    try {
      setToday(await api.getMeToday());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't load your day");
    }
    // The week is decoration on top of the day — it failing is not an error.
    void api.getMeHistory().then(setHistory).catch(() => {});
  }
  useEffect(() => {
    void load();
    void api.getCatalog().then(setCatalog).catch(() => setCatalog(null));
    // Living data: the minutes move while the page is open.
    const t = setInterval(() => void load(), 30_000);
    return () => clearInterval(t);
  }, []);

  const member = me?.account?.role === "member";
  const name = me?.account?.display_name ?? "";
  const theme: Theme = today?.theme ?? me?.account?.effective_theme ?? "plain";

  // Headline numbers. With a limit: what's left. Without: what's used.
  const total = today && today.limit_minutes !== null ? today.limit_minutes + today.earned_minutes : null;
  const left = today?.left_minutes ?? null;
  const spent = today ? today.locked || (left !== null && left <= 0) : false;
  const headline = useCountUp(today ? (total !== null ? (left ?? 0) : today.used_minutes) : 0, 900);
  const pct = total && left !== null ? left / total : total === null ? 1 : 0;

  async function signOut() {
    await logout();
    navigate("/login", { replace: true });
  }

  const hour = new Date().getHours();
  const hi =
    theme === "playful"
      ? `${hour < 12 ? "Good morning" : hour < 18 ? "Hi" : "Good evening"}, ${name.split(" ")[0]}!`
      : theme === "calm"
        ? name
        : "My screen time";

  return (
    <div className={`me theme-${theme}`} data-mode={mode}>
      <header className="me-head">
        <h1 className="me-hi">{hi}</h1>
        {!member && <p className="me-sub">Your own day, private to you.</p>}
      </header>

      {error && (
        <p className="me-err">
          {error}{" "}
          <button className="me-link" onClick={() => void load()}>Try again</button>
        </p>
      )}

      {member && <FirstVisit theme={theme} />}

      {today && (
        <>
          {theme !== "plain" ? (
            <section className="me-hero">
              <Ring pct={pct} size={theme === "playful" ? 280 : 220} stroke={theme === "playful" ? 24 : 10} spent={spent}>
                {spent ? (
                  <>
                    <span className="me-big me-big-stop">Stop</span>
                    <span className="me-unit">{today.locked ? "paused by a parent" : "time's up for today"}</span>
                  </>
                ) : total !== null ? (
                  <>
                    <span className="me-big">{fmt(headline)}</span>
                    <span className="me-unit">{unitFor(headline)} left today</span>
                  </>
                ) : (
                  <>
                    <span className="me-big">{fmt(headline)}</span>
                    <span className="me-unit">{unitFor(headline)} today · no limit</span>
                  </>
                )}
              </Ring>
              {spent && (
                <p className="me-stopline">
                  {today.locked
                    ? "Your screen is paused. A parent can start it again."
                    : "That's all the screen time for today. It starts again tomorrow."}
                </p>
              )}
              {!spent && total !== null && (
                <p className="me-under">
                  {today.used_minutes} of {total} used
                  {today.earned_minutes > 0 && ` · ${today.earned_minutes} earned`}
                </p>
              )}
              {theme === "calm" && total !== null && (
                <dl className="me-stats">
                  <div><dt>used</dt><dd>{today.used_minutes} min</dd></div>
                  <div><dt>limit</dt><dd>{today.limit_minutes} min</dd></div>
                  <div><dt>earned</dt><dd>{today.earned_minutes} min</dd></div>
                </dl>
              )}
              <AskForTime today={today} theme={theme} onAsked={() => void load()} />
            </section>
          ) : (
            <section className="me-plain-top">
              <div className="me-plain-stat">
                <p className="me-plain-big" data-spent={spent}>{headline}</p>
                <p className="me-plain-unit">
                  {total !== null ? (spent ? "no time left today" : "minutes left today") : "minutes today · no limit"}
                </p>
              </div>
              {today.locked && <p className="me-stopline">Paused.</p>}
            </section>
          )}

          {history && history.days.length > 0 && (
            <Week history={history} today={today} theme={theme} />
          )}
          <WhereTheTime />
          <Schedule today={today} theme={theme} />
          <Blocked today={today} catalog={catalog} theme={theme} />
          <Devices today={today} theme={theme} />
        </>
      )}

      {!today && !error && <p className="me-wait">…</p>}

      <footer className="me-foot">
        {member ? (
          <>
            <Wordmark size={0.8} />
            <button className="me-link" onClick={() => void signOut()}>Sign out</button>
          </>
        ) : (
          <p className="me-foot-note">This is what {name || "you"} would see on their own computer.</p>
        )}
      </footer>
    </div>
  );
}
