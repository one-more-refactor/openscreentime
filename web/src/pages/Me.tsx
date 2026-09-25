// ============================================================================
// ME — a person's own page, in one calm look for everyone.
//
// Two shapes of the same page:
//
//   My day       a child or teen on their own computer (a member session can
//                reach nothing else): the ring — time USED today, filling
//                like every ring in the product — and the time left; asking
//                for more; their rules in one glance; the week; where the
//                time went; their computers.
//
//   My computer  someone who keeps their own time — an adult, or the parent
//                for themselves (brand board § f): the same ring, then their
//                own daily limit, focus hours and the sites they block for
//                themselves, each edited in place; the week; where the time
//                went; this computer. No "parent" anywhere: the limits are
//                the ones they set.
// ============================================================================
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { Link, useNavigate } from "react-router-dom";
import * as api from "../api";
import type { Catalog, MeHistory, MeToday, MyRules, TimeWindow } from "../types";
import { useSession } from "../lib/session";
import { useCountUp } from "../lib/useCountUp";
import { duration, durationShort, sentence } from "../lib/format";
import { describeWindow } from "../lib/schedule";
import { daysBefore, deviceNow, stopSentence, type WallTime } from "../lib/day";
import { parentSeesSentence } from "../lib/parentSees";
import { cantFilter, notBlockedSentence } from "../lib/degraded";
import {
  DAY_LETTERS,
  DAY_SHORT,
  WEEK_ORDER,
  describeDays,
  focusEndsAt,
  focusProblem,
  nextFocusStart,
  normalizeSite,
} from "../lib/myRules";
import { Ring, RingNumber } from "../components/Ring";
import { Button } from "../components/Button";
import { Icon, type IconName } from "../components/Icon";
import { Wordmark } from "../components/Wordmark";
import { WhereTheTime } from "../components/WhereTheTime";
import { FluentSlider } from "../components/FluentSlider";
import { TextInput } from "../components/TextInput";
import { PageHead } from "../layout/PageHead";

// ---- the day, as a ring ---------------------------------------------------------

/** Time left — what the computer shows: minutes until the screen stops, a
 * snooze or an unlock included. Null = no limit. */
function timeLeft(today: MeToday): number | null {
  if (today.limit_minutes === null) return null;
  if (typeof today.left_minutes === "number") return Math.max(0, today.left_minutes);
  return Math.max(0, today.limit_minutes + today.earned_minutes - today.used_minutes);
}

/** Fraction of the day used — what the ring draws: used of used + left, so the
 * ring agrees with the number. Null = no limit. */
export function usedFraction(today: MeToday): number | null {
  const left = timeLeft(today);
  if (left === null) return null;
  const total = today.used_minutes + left;
  return total > 0 ? today.used_minutes / total : 1;
}

function DayRing({ today, size }: { today: MeToday; size: number }) {
  const total = today.limit_minutes === null ? null : today.limit_minutes + today.earned_minutes;
  const left = timeLeft(today);
  const shown = useCountUp(left ?? today.used_minutes, 700);
  const label =
    total === null
      ? `${duration(today.used_minutes)} used today, no limit`
      : `${duration(today.used_minutes)} used of ${duration(total)}, ${duration(left ?? 0)} left`;
  return (
    <Ring size={size} used={usedFraction(today)} minutesLeft={left} paused={today.locked} on="card" label={label}>
      <RingNumber
        size={size}
        value={durationShort(shown)}
        unit={
          total === null
            ? shown < 60
              ? "min today"
              : "today"
            : shown >= 60 && shown % 60 === 0
              ? "left"
              : "min left"
        }
      />
    </Ring>
  );
}

// ---- the week -------------------------------------------------------------------

/** The last seven days on the COMPUTER's calendar — the days its ledger is
 * filed under — ending with its today (not this browser's). */
export function lastSevenDays(history: MeHistory | null, today: MeToday, now = new Date()) {
  const byDay = new Map((history?.days ?? []).map((d) => [d.day, d]));
  const base = deviceNow(today.utc_offset_secs, now).date;
  const days = Array.from({ length: 7 }, (_, i) => {
    const d = daysBefore(base, 6 - i);
    const row = byDay.get(d.date);
    return { key: d.date, day: d.day, used: row?.used_minutes ?? 0, earned: row?.earned_minutes ?? 0, today: i === 6 };
  });
  // The live numbers beat a history row that may lag behind them.
  days[6].used = Math.max(days[6].used, today.used_minutes);
  days[6].earned = Math.max(days[6].earned, today.earned_minutes);
  return days;
}

function Week({ history, today, title }: { history: MeHistory | null; today: MeToday; title: string }) {
  const days = lastSevenDays(history, today);
  const limit = today.limit_minutes;
  // A little headroom above the tallest bar (or the limit line).
  const max = Math.max(...days.map((d) => d.used), limit ?? 0, 60) * 1.12;
  const earlier = days.slice(0, 6).filter((d) => d.used > 0);
  const avg = earlier.length ? Math.round(earlier.reduce((s, d) => s + d.used, 0) / earlier.length) : null;
  const short = (m: number) => (m < 60 ? `${m} min` : durationShort(m));
  return (
    <section className="section">
      <div className="card card-pad me-week">
        <div className="me-week-h">
          <h2 className="h2">{title}</h2>
          <span className="meta num">
            {[avg !== null ? `average ${duration(avg)}` : null, limit ? `limit ${duration(limit)}` : null]
              .filter(Boolean)
              .join(" · ")}
          </span>
        </div>
        <div
          className="me-bars"
          role="img"
          aria-label={`Screen time, the last seven days${avg !== null ? `, average ${duration(avg)}` : ""}`}
        >
          <div className="me-bars-plot">
            {limit ? (
              <div className="me-bars-limit" style={{ bottom: `${(limit / max) * 100}%` }}>
                <em className="num">{duration(limit)}</em>
              </div>
            ) : null}
            {days.map((d) => (
              <i
                key={d.key}
                className="me-bar"
                data-today={d.today}
                // Over is over the day's whole budget — time given on top isn't "over".
                data-over={limit ? d.used > limit + d.earned : false}
                style={{ height: `${Math.max(1.5, (d.used / max) * 100)}%` }}
              />
            ))}
          </div>
          <div className="me-bars-labels">
            {days.map((d) => (
              <span key={d.key} className="num" data-today={d.today}>
                <b>{d.used > 0 ? short(d.used) : "—"}</b>
                {d.today ? "Today" : DAY_SHORT[d.day]}
              </span>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}

// ---- their computers ------------------------------------------------------------

function ComputerRows({ today, hub, who }: { today: MeToday; hub: boolean; who: "my" | "your" }) {
  if (today.devices.length === 0) {
    return (
      <div className="card me-devrow">
        <Icon name="laptop" size={20} />
        <span>No computer yet.</span>
        <span className="sp" />
        {hub && (
          <Link to="/computers?add=mine" className="btn btn-secondary btn-sm">
            Add my computer
          </Link>
        )}
      </div>
    );
  }
  return (
    <div className="card me-devices">
      {today.devices.map((d) => (
        <div key={d.name} className="me-devrow">
          <Icon name={d.status === "offline" ? "offline" : "laptop"} size={20} />
          <span>
            <b>{d.name}</b>
            <span className="meta">
              {" · "}
              {d.locked ? "paused" : d.status === "online" ? "online" : d.status === "pending" ? "not set up yet" : "offline — it keeps today's rules"}
            </span>
          </span>
          <span className="sp" />
          {hub && (
            <Link to="/settings" className="me-devrow-keys">
              {who === "my" ? "Unlock code and recovery codes are in Settings" : "Keys are in Settings"}
            </Link>
          )}
        </div>
      ))}
    </div>
  );
}

// ---- a child's or teen's own day ------------------------------------------------

/** "What can they see?" — said once on a first visit, and always one tap away.
 * The first sentence is the server's own answer for this person. */
function seeSentences(today: MeToday): [string, string] {
  return [
    parentSeesSentence(today.parent_sees),
    "They can't see your screen, read your messages or see what you type. Nothing in OpenScreenTime can.",
  ];
}

function FirstVisit({ see }: { see: [string, string] }) {
  const KEY = "ost-intro-seen";
  const [seen, setSeen] = useState(() => {
    try {
      return localStorage.getItem(KEY) === "1";
    } catch {
      return true;
    }
  });
  if (seen) return null;
  return (
    <div className="banner me-intro" role="note">
      <Icon name="info" size={20} />
      <div className="banner-main">
        <p>
          <b>Before anything else.</b> {see[0]} {see[1]}
        </p>
      </div>
      <Button
        size="sm"
        variant="quiet"
        onClick={() => {
          try {
            localStorage.setItem(KEY, "1");
          } catch {
            /* private mode: show again next time */
          }
          setSeen(true);
        }}
      >
        Got it
      </Button>
    </div>
  );
}

function AskForTime({ today, onAsked }: { today: MeToday; onAsked: () => void }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  if (today.bracket === "little" || today.bracket === "adult") return null;
  if (today.pending_request) {
    return (
      <p className="me-asked">
        <Icon name="check" size={18} /> You asked. A parent will answer soon.
      </p>
    );
  }
  async function ask(minutes: number) {
    setBusy(true);
    setErr(null);
    try {
      await api.askForTime(minutes);
      onAsked();
    } catch (e) {
      setErr(sentence(e instanceof Error ? e.message : "That didn't go through. Try again."));
    } finally {
      setBusy(false);
    }
  }
  const kid = today.bracket === "kid";
  return (
    <div className="me-ask">
      <Button icon="ask" disabled={busy} onClick={() => void ask(15)}>
        {busy ? "Asking…" : kid ? "Ask for 15 more minutes" : "Ask for more time"}
      </Button>
      {!kid && (
        <span className="me-ask-more">
          <Button size="sm" variant="quiet" disabled={busy} onClick={() => void ask(30)}>
            30 min
          </Button>
          <Button size="sm" variant="quiet" disabled={busy} onClick={() => void ask(60)}>
            1 h
          </Button>
        </span>
      )}
      {err && (
        <p className="hint" data-error="true" role="alert">
          {err}
        </p>
      )}
    </div>
  );
}

function RuleLine({ icon, label, value }: { icon: IconName; label: string; value: ReactNode }) {
  return (
    <li className="me-rule">
      <Icon name={icon} size={20} />
      <span className="me-rule-label">{label}</span>
      <b className="me-rule-value num">{value}</b>
    </li>
  );
}

/** Their rules in one glance — the ones a parent set. */
function YourRules({ today, catalog }: { today: MeToday; catalog: Catalog | null }) {
  const blocked = useMemo(() => {
    const cats = new Set(today.blocks.categories);
    const viaCat = new Set((catalog?.apps ?? []).filter((a) => cats.has(a.category)).map((a) => a.id));
    return [
      ...(catalog?.categories ?? []).filter((c) => cats.has(c.id)).map((c) => c.name),
      ...(catalog?.apps ?? []).filter((a) => today.blocks.apps.includes(a.id) && !viaCat.has(a.id)).map((a) => a.name),
      ...today.blocks.custom_domains,
    ];
  }, [today, catalog]);
  const school = today.windows.find((w) => w.days.includes(1));
  const weekend = today.windows.find((w) => w.days.includes(0) || w.days.includes(6));
  const hours = [
    school ? `School days ${describeWindow(school.start, school.end)}` : null,
    weekend ? `Weekend ${describeWindow(weekend.start, weekend.end)}` : null,
  ].filter(Boolean);

  return (
    <section className="section">
      <div className="section-head">
        <h2 className="h2">Your rules</h2>
      </div>
      <ul className="card me-rules">
        <RuleLine icon="clock" label="Every day" value={today.limit_minutes ? duration(today.limit_minutes) : "No limit"} />
        {hours.length > 0 && <RuleLine icon="allowed-hours" label="Screens on" value={hours.join(" · ")} />}
        {today.bedtime && (
          <RuleLine
            icon="moon"
            label="Bedtime"
            value={`${today.bedtime.start} – ${today.bedtime.end === "00:00" ? "midnight" : today.bedtime.end}`}
          />
        )}
        {blocked.length > 0 && (
          <RuleLine
            icon="block"
            label="Blocked"
            value={blocked.length > 6 ? `${blocked.slice(0, 5).join(", ")} and ${blocked.length - 5} more` : blocked.join(", ")}
          />
        )}
      </ul>
    </section>
  );
}

function MyDay({
  today,
  history,
  catalog,
  reload,
}: {
  today: MeToday;
  history: MeHistory | null;
  catalog: Catalog | null;
  reload: () => void;
}) {
  const { me } = useSession();
  const first = (me?.account?.display_name ?? "").split(/\s+/)[0];
  const total = today.limit_minutes === null ? null : today.limit_minutes + today.earned_minutes;
  const spent = total !== null && (today.left_minutes ?? 1) <= 0;
  const next = today.locked
    ? "A parent paused your computer. Nothing is broken — talk to them, and it comes back."
    : stopSentence(today.rules, "you");
  const see = seeSentences(today);

  return (
    <>
      <PageHead title={first ? `Hi, ${first}` : "Your day"} sub="Here's your day so far." />
      <FirstVisit see={see} />
      <section className="card me-hero">
        <DayRing today={today} size={176} />
        <div className="me-hero-main">
          <p className="me-big num">
            {duration(today.used_minutes)} used today{" "}
            <small>
              {total !== null
                ? `of ${duration(total)}${today.earned_minutes > 0 ? ` · ${today.earned_minutes} min extra` : ""}`
                : "· no limit"}
            </small>
          </p>
          {spent && !today.locked ? (
            <p className="me-next">That's all the screen time for today. It starts again tomorrow.</p>
          ) : (
            next && <p className="me-next">{next}</p>
          )}
          <AskForTime today={today} onAsked={reload} />
        </div>
      </section>
      <YourRules today={today} catalog={catalog} />
      <Week history={history} today={today} title="Your week" />
      <WhereTheTime who="you" offsetSecs={today.utc_offset_secs} />
      <section className="section">
        <div className="section-head">
          <h2 className="h2">Your computers</h2>
        </div>
        <ComputerRows today={today} hub={false} who="your" />
      </section>
      <details className="disclosure me-see">
        <summary>
          <Icon name="chevron-right" size={16} />
          What can a parent see?
        </summary>
        <p className="lede">{see[0]}</p>
        <p className="lede">{see[1]}</p>
      </details>
    </>
  );
}

// ---- my computer: someone keeping their own time --------------------------------

function RuleCard({
  icon,
  title,
  editing,
  onEdit,
  wide,
  children,
}: {
  icon: IconName;
  title: string;
  editing: boolean;
  onEdit?: () => void;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="card me-mrule" data-wide={wide} data-editing={editing}>
      <div className="me-mrule-h">
        <Icon name={icon} size={18} />
        <h3>{title}</h3>
        {onEdit && !editing && (
          <button type="button" className="me-edit" onClick={onEdit}>
            Edit
          </button>
        )}
      </div>
      {children}
    </div>
  );
}

function LimitCard({ rules, busy, save }: { rules: MyRules; busy: boolean; save: (r: MyRules) => Promise<boolean> }) {
  const [editing, setEditing] = useState(false);
  const limit = rules.daily_limit_minutes;
  return (
    <RuleCard icon="clock" title="My daily limit" editing={editing} onEdit={() => setEditing(true)}>
      <p className="me-mrule-v num">
        {limit > 0 ? duration(limit) : "No limit"}{" "}
        <small>{limit > 0 ? "a day · a hard stop, with warnings at 15, 5 and 1 minute" : "· your minutes are only counted"}</small>
      </p>
      {editing && (
        <div className="me-mrule-edit">
          <FluentSlider
            min={0}
            max={600}
            step={15}
            value={limit}
            disabled={busy}
            aria-label="My daily limit"
            format={(v) => (v === 0 ? "No limit" : `${duration(v)} a day`)}
            onCommit={(v) => void save({ ...rules, daily_limit_minutes: v }).then((ok) => ok && setEditing(false))}
          />
          <Button size="sm" variant="quiet" onClick={() => setEditing(false)}>
            Done
          </Button>
        </div>
      )}
    </RuleCard>
  );
}

function FocusCard({
  rules,
  busy,
  save,
  sitesBlocked,
}: {
  rules: MyRules;
  busy: boolean;
  save: (r: MyRules) => Promise<boolean>;
  /** false: no computer of theirs can filter websites right now */
  sitesBlocked: boolean;
}) {
  const h = rules.focus_hours;
  const [editing, setEditing] = useState(false);
  const [days, setDays] = useState<number[]>(h?.days ?? [1, 2, 3, 4, 5]);
  const [start, setStart] = useState(h?.start ?? "09:00");
  const [end, setEnd] = useState(h?.end ?? "12:00");
  function open() {
    setDays(h?.days ?? [1, 2, 3, 4, 5]);
    setStart(h?.start ?? "09:00");
    setEnd(h?.end ?? "12:00");
    setEditing(true);
  }
  const draft: TimeWindow = { days: [...days].sort(), start, end };
  const problem = focusProblem(draft);

  return (
    <RuleCard icon="allowed-hours" title="My focus hours" editing={editing} onEdit={open}>
      <p className="me-mrule-v num">
        {h ? describeWindow(h.start, h.end) : "All day"}{" "}
        <small>
          {!sitesBlocked
            ? h
              ? describeDays(h.days)
              : ""
            : h
              ? `${describeDays(h.days)} · the sites below are blocked`
              : "· the sites below are blocked all the time"}
        </small>
      </p>
      {editing && (
        <form
          className="me-mrule-edit me-focus-edit"
          onSubmit={(e) => {
            e.preventDefault();
            if (!problem) void save({ ...rules, focus_hours: draft }).then((ok) => ok && setEditing(false));
          }}
        >
          <div className="me-days" role="group" aria-label="Days">
            {WEEK_ORDER.map((d) => (
              <button
                key={d}
                type="button"
                className="me-day"
                aria-pressed={days.includes(d)}
                aria-label={DAY_SHORT[d]}
                onClick={() => setDays((xs) => (xs.includes(d) ? xs.filter((x) => x !== d) : [...xs, d]))}
              >
                {DAY_LETTERS[d]}
              </button>
            ))}
          </div>
          <div className="me-times">
            <input className="field pp-time num" inputMode="numeric" maxLength={5} aria-label="Focus from" value={start} onChange={(e) => setStart(e.target.value)} />
            <span className="pp-dash">–</span>
            <input className="field pp-time num" inputMode="numeric" maxLength={5} aria-label="Focus until" value={end} onChange={(e) => setEnd(e.target.value)} />
          </div>
          <p className="hint" data-error={problem !== null} role={problem ? "alert" : undefined}>
            {problem ?? `${describeDays(draft.days)}, ${describeWindow(start, end)}. 00:00 is midnight; hours can run past it.`}
          </p>
          <div className="me-mrule-actions">
            <Button size="sm" type="submit" disabled={busy || problem !== null}>
              Save
            </Button>
            {h && (
              <Button
                size="sm"
                variant="quiet"
                disabled={busy}
                onClick={() => void save({ ...rules, focus_hours: null }).then((ok) => ok && setEditing(false))}
              >
                Block all day instead
              </Button>
            )}
            <Button size="sm" variant="quiet" onClick={() => setEditing(false)}>
              Cancel
            </Button>
          </div>
        </form>
      )}
    </RuleCard>
  );
}

function SitesCard({
  rules,
  busy,
  save,
  notBlocked,
}: {
  rules: MyRules;
  busy: boolean;
  save: (r: MyRules) => Promise<boolean>;
  /** A computer of theirs can't filter websites: said right at the list. */
  notBlocked: string | null;
}) {
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  function add(e: React.FormEvent) {
    e.preventDefault();
    const n = normalizeSite(draft);
    if ("problem" in n) {
      setProblem(n.problem);
      return;
    }
    if (rules.sites.includes(n.site)) {
      setDraft("");
      return;
    }
    void save({ ...rules, sites: [...rules.sites, n.site] }).then((ok) => {
      if (ok) setDraft("");
    });
  }
  return (
    <RuleCard icon="block" title="Sites I block for myself" editing={adding} wide>
      {notBlocked && (
        <p className="banner banner-warn me-noblock" role="status">
          <Icon name="warning" size={18} />
          <span className="banner-main">{notBlocked}</span>
        </p>
      )}
      <ul className="me-chips" aria-label="Sites I block for myself">
        {rules.sites.map((s) => (
          <li key={s} className="chip">
            <Icon name="globe" size={14} />
            {s}
            <button
              type="button"
              className="chip-x"
              disabled={busy}
              aria-label={`Stop blocking ${s}`}
              onClick={() => void save({ ...rules, sites: rules.sites.filter((x) => x !== s) })}
            >
              <Icon name="close" size={14} />
            </button>
          </li>
        ))}
        {!adding && (
          <li>
            <button type="button" className="chip me-chip-add" onClick={() => setAdding(true)}>
              <Icon name="add" size={14} />
              Add a site
            </button>
          </li>
        )}
      </ul>
      {rules.sites.length === 0 && !adding && <p className="meta">Nothing yet. Add the ones that pull you in.</p>}
      {adding && (
        <form className="me-add-site" onSubmit={add}>
          <TextInput
            aria-label="A site to block"
            placeholder="e.g. reddit.com"
            value={draft}
            autoFocus
            error={problem}
            onChange={(e) => {
              setDraft(e.target.value);
              setProblem(null);
            }}
          />
          <Button type="submit" size="sm" disabled={busy || !draft.trim()}>
            Block it
          </Button>
          <Button size="sm" variant="quiet" onClick={() => setAdding(false)}>
            Done
          </Button>
        </form>
      )}
    </RuleCard>
  );
}

/** One sentence about the focus hours: what holds now, what happens next —
 * on the computer's clock (`now`), where the sites are really blocked.
 * `sitesBlocked` false: no computer of theirs can filter websites right now,
 * so the hours are only hours — nothing is said to be blocked or open. */
export function focusLine(rules: MyRules, now: WallTime, sitesBlocked = true): string | null {
  const f = { sites: rules.sites, hours: rules.focus_hours };
  if (rules.sites.length === 0) return null;
  const ends = focusEndsAt(f, now);
  if (ends) return sitesBlocked ? `Focus hours until ${ends} — the sites below open again then.` : `Focus hours until ${ends}.`;
  if (!rules.focus_hours) return sitesBlocked ? "The sites below are blocked all day." : null;
  const next = nextFocusStart(f, now);
  if (!next) return null;
  if (!sitesBlocked) {
    return next.today ? `Focus hours start at ${next.start} today.` : `Next focus hours: ${DAY_SHORT[next.day]} at ${next.start}.`;
  }
  return next.today
    ? `Focus hours start at ${next.start} today — the sites below are blocked then.`
    : `Next focus hours: ${DAY_SHORT[next.day]} at ${next.start}. Until then the sites below are open.`;
}

function MyComputer({
  today,
  history,
  reload,
}: {
  today: MeToday;
  history: MeHistory | null;
  reload: () => void;
}) {
  const { me } = useSession();
  const hub = me?.account?.role !== "member";
  const [rules, setRules] = useState<MyRules | null>(null);
  const [rulesError, setRulesError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    api
      .getMyRules()
      .then((r) => alive && setRules(r))
      .catch((e) => alive && setRulesError(sentence(e instanceof Error ? e.message : "Couldn't load your rules.")));
    return () => {
      alive = false;
    };
  }, []);

  const save = useCallback(
    async (next: MyRules): Promise<boolean> => {
      setBusy(true);
      setSaveError(null);
      setSaved(false);
      try {
        setRules(await api.setMyRules(next));
        setSaved(true);
        reload();
        return true;
      } catch (e) {
        setSaveError(sentence(e instanceof Error ? e.message : "That didn't save. Try again."));
        return false;
      } finally {
        setBusy(false);
      }
    },
    [reload],
  );

  const online = today.devices.some((d) => d.status === "online");
  const title =
    today.devices.length === 1 ? today.devices[0].name : today.devices.length > 1 ? "My computers" : "My computer";
  const total = today.limit_minutes === null ? null : today.limit_minutes + today.earned_minutes;
  const next = today.locked ? "This computer is paused." : stopSentence(today.rules, "you");
  // What the computers really report: a block promised only where it holds.
  const notBlocked = notBlockedSentence(today.devices);
  const onlineNow = today.devices.filter((d) => d.status === "online");
  const sitesBlocked = !(onlineNow.length > 0 && cantFilter(onlineNow).length === onlineNow.length);
  const focus = rules ? focusLine(rules, deviceNow(today.utc_offset_secs), sitesBlocked) : null;

  return (
    <>
      <PageHead
        eyebrow="My computer"
        title={title}
        sub="Only you can see this. The limits here are the ones you set — nobody else's."
        actions={
          online ? (
            <span className="tag tag-ok me-online">
              <Icon name="check" size={14} />
              Online
            </span>
          ) : undefined
        }
      />

      <section className="card me-hero">
        <DayRing today={today} size={152} />
        <div className="me-hero-main">
          <p className="me-big num">
            {duration(today.used_minutes)} used today{" "}
            <small>{total !== null ? `of the ${duration(today.limit_minutes ?? 0)} you set` : "· no limit set"}</small>
          </p>
          {focus && <p className="me-next">{focus}</p>}
          {focus && notBlocked && <p className="me-next">{notBlocked}</p>}
          {next && <p className="me-next">{next}</p>}
          {total !== null && (
            <p className="me-hatch">
              <Icon name="info" size={16} />
              When you hit your limit you can wait a minute and take 15 more — it's your call.
            </p>
          )}
        </div>
      </section>

      <section className="section" aria-label="My rules">
        {rulesError ? (
          <p className="banner banner-warn">
            <Icon name="warning" size={20} />
            <span className="banner-main">{rulesError}</span>
          </p>
        ) : !rules ? (
          <div className="me-mrules" aria-busy="true">
            <span className="wait" style={{ height: 96 }} />
            <span className="wait" style={{ height: 96 }} />
          </div>
        ) : (
          <div className="me-mrules">
            <LimitCard rules={rules} busy={busy} save={save} />
            <FocusCard rules={rules} busy={busy} save={save} sitesBlocked={sitesBlocked} />
            <SitesCard rules={rules} busy={busy} save={save} notBlocked={notBlocked} />
          </div>
        )}
        {(saveError || saved) && (
          <p className="hint me-saved" data-error={!!saveError} role={saveError ? "alert" : "status"}>
            {saveError ?? "Saved. Your computer picks it up within a minute."}
          </p>
        )}
      </section>

      <Week history={history} today={today} title="My week" />
      <WhereTheTime who="you" offsetSecs={today.utc_offset_secs} />
      <section className="section">
        <div className="section-head">
          <h2 className="h2">{today.devices.length > 1 ? "These computers" : "This computer"}</h2>
        </div>
        <ComputerRows today={today} hub={hub} who="my" />
      </section>
    </>
  );
}

// ---- the page -------------------------------------------------------------------

export function Me() {
  const { me, logout } = useSession();
  const navigate = useNavigate();
  const [today, setToday] = useState<MeToday | null>(null);
  const [history, setHistory] = useState<MeHistory | null>(null);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setToday(await api.getMeToday());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't load your day.");
    }
    // The week is decoration on the day — it failing is not an error.
    void api
      .getMeHistory()
      .then(setHistory)
      .catch(() => {});
  }, []);

  useEffect(() => {
    void load();
    void api
      .getCatalog()
      .then(setCatalog)
      .catch(() => setCatalog(null));
    // Living data: the minutes move while the page is open.
    const t = setInterval(() => void load(), 30_000);
    return () => clearInterval(t);
  }, [load]);

  const account = me?.account;
  const member = account?.role === "member";
  const own =
    today?.self_managed ?? (!!account && (account.role !== "member" || account.age_bracket === "adult" || account.self_managed));

  return (
    <div className="page me" data-shape={own ? "own" : "day"}>
      {member && (
        <div className="me-top">
          <Wordmark />
        </div>
      )}

      {error && (
        <div className="banner banner-stop" role="alert">
          <Icon name="warning" size={20} />
          <p className="banner-main">{today ? "Couldn't refresh — this is your day a moment ago." : "Couldn't load your day."}</p>
          <Button size="sm" variant="quiet" icon="refresh" onClick={() => void load()}>
            Try again
          </Button>
        </div>
      )}

      {!today && !error && (
        <div className="me-waiting" aria-busy="true" aria-label="Loading your day">
          <span className="wait" style={{ height: 40, width: "40%" }} />
          <span className="wait" style={{ height: 200 }} />
        </div>
      )}

      {today &&
        (own ? (
          <MyComputer today={today} history={history} reload={() => void load()} />
        ) : (
          <MyDay today={today} history={history} catalog={catalog} reload={() => void load()} />
        ))}

      {member && (
        <footer className="me-foot">
          <Button
            size="sm"
            variant="quiet"
            icon="sign-out"
            onClick={() => void logout().then(() => navigate("/login", { replace: true }))}
          >
            Sign out
          </Button>
        </footer>
      )}
    </div>
  );
}
