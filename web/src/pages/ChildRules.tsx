// ============================================================================
// THE RULES — the full parenting suite, every control deliberate:
//
//   Daily limit     slider (0 = no limit). One drag = one step-up = one save.
//   Allowed hours   two windows (school days / weekend) — matches how real
//                   families think, without a 7×24 grid nobody fills in.
//   Bedtime         time range; separate from allowed hours because "screens
//                   off at night" survives even a generous day.
//   App limits      per-app slider on top of the daily limit; remove = chip ×.
//   Websites        allow/block chips — the actual links a parent cares about.
//   Safe search     one switch, because it's one decision.
//   Earning back    task editor: label + reward, add/remove, one toggle.
//   Time's up       what the hard stop feels like: wait, math, or parent PIN.
//
// Every commit goes through the caller's onSave → step-up → whole policy.
// ============================================================================
import { useEffect, useMemo, useState } from "react";
import { EMPTY_BLOCKS, type AppBlocks, type Catalog, type Policy, type Profile, type TimeWindow, type UnlockChallenge } from "../types";
import { FluentSlider } from "../components/FluentSlider";
import { getCatalog } from "../api";
import { useAsync } from "../lib/useAsync";

export function fmtMin(m: number): string {
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r === 0 ? `${h} h` : `${h} h ${r} min`;
}

/** screen_time.enabled must survive "no daily limit" while bedtime remains. */
function screenTimeEnabled(p: Policy): boolean {
  const st = p.screen_time;
  return st.daily_limit_minutes > 0 || st.bedtime !== null || st.schedule.length > 0;
}

const WEEKDAYS = [1, 2, 3, 4, 5];
const WEEKEND = [0, 6];

function findWindow(schedule: TimeWindow[], days: number[]): TimeWindow | null {
  return schedule.find((w) => days.some((d) => w.days.includes(d))) ?? null;
}

interface RulesProps {
  profile: Profile;
  busy: boolean;
  onSave: (next: Policy, doneNote: string) => void;
}

export function Rules({ profile, busy, onSave }: RulesProps) {
  const pol = profile.policy;
  const st = pol.screen_time;
  const limit = st.enabled ? st.daily_limit_minutes : 0;

  function withScreenTime(next: Partial<Policy["screen_time"]>): Policy {
    const merged = { ...pol, screen_time: { ...st, ...next } };
    merged.screen_time.enabled = screenTimeEnabled(merged);
    return merged;
  }

  return (
    <div className="rl">
      <AppsAndCategories pol={pol} busy={busy} onSave={onSave} />
      <DailyLimit limit={limit} busy={busy} onSave={(m) =>
        onSave(
          withScreenTime({ daily_limit_minutes: m }),
          m === 0 ? "Daily limit removed." : `Daily limit set to ${fmtMin(m)}.`,
        )
      } />
      <AllowedHours st={st} busy={busy} withScreenTime={withScreenTime} onSave={onSave} />
      <BedtimeRule st={st} busy={busy} withScreenTime={withScreenTime} onSave={onSave} />
      <Websites pol={pol} busy={busy} onSave={onSave} />
      <SafeSearch pol={pol} busy={busy} onSave={onSave} />
      <EarnTime pol={pol} busy={busy} onSave={onSave} />
      <TimesUp pol={pol} busy={busy} onSave={onSave} />
    </div>
  );
}

// ---- apps & categories -----------------------------------------------------
// The one-click blocks. A category is one decision ("no social media"); an
// app is the exception a parent actually argues about ("…but YouTube is
// fine"). Apps already covered by a blocked category are shown as covered and
// can't be toggled on their own — the category is the rule, the grid is the
// picture of it. Every tap is one save (one step-up), like the rest of the
// rules. Names come from the server's catalog; the device holds the domains.

/** A recognisable tile without shipping a logo pack: two letters, a hue. */
function Monogram({ id, name }: { id: string; name: string }) {
  let h = 0;
  for (const ch of id) h = (h * 31 + ch.charCodeAt(0)) % 360;
  const parts = name.replace(/[()]/g, "").split(/[\s/]+/).filter(Boolean);
  const mono = (parts.length >= 2 ? parts[0][0] + parts[1][0] : name.slice(0, 2)).toUpperCase();
  return (
    <span
      className="apps-mono"
      style={{ background: `hsl(${h} 45% 88%)`, color: `hsl(${h} 55% 26%)` }}
      aria-hidden="true"
    >
      {mono}
    </span>
  );
}

function AppsAndCategories({ pol, busy, onSave }: { pol: Policy; busy: boolean; onSave: (p: Policy, note: string) => void }) {
  const catalog = useAsync<Catalog>(getCatalog, []);
  const blocks: AppBlocks = pol.blocks ?? EMPTY_BLOCKS;
  const [showAll, setShowAll] = useState(false);

  const cats = catalog.data?.categories ?? [];
  const apps = catalog.data?.apps ?? [];
  const blockedCats = useMemo(() => new Set(blocks.categories), [blocks.categories]);
  const blockedApps = useMemo(() => new Set(blocks.apps), [blocks.apps]);

  function save(next: Partial<AppBlocks>, note: string) {
    onSave({ ...pol, blocks: { ...blocks, ...next } }, note);
  }

  function toggleCategory(id: string, name: string) {
    const on = blockedCats.has(id);
    save(
      { categories: on ? blocks.categories.filter((c) => c !== id) : [...blocks.categories, id] },
      on ? `${name} is allowed again.` : `${name} is blocked.`,
    );
  }
  function toggleApp(id: string, name: string) {
    const on = blockedApps.has(id);
    save(
      { apps: on ? blocks.apps.filter((a) => a !== id) : [...blocks.apps, id] },
      on ? `${name} is allowed again.` : `${name} is blocked.`,
    );
  }

  // The grid: blocked and covered apps first, then the rest; beyond twelve
  // the list folds unless something inside is blocked or the parent opens it.
  const covered = (a: { id: string; category: string }) => blockedCats.has(a.category);
  const sorted = [...apps].sort((a, b) => {
    const ra = blockedApps.has(a.id) || covered(a) ? 0 : 1;
    const rb = blockedApps.has(b.id) || covered(b) ? 0 : 1;
    return ra - rb;
  });
  const FOLD = 12;
  const visible = showAll ? sorted : sorted.slice(0, FOLD);
  const blockedCount = blockedCats.size + blocks.apps.filter((a) => !covered({ id: a, category: apps.find((x) => x.id === a)?.category ?? "" })).length;

  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what">
        <p className="rl-name">Apps &amp; categories</p>
        <p className="rl-value">
          {catalog.loading
            ? "Loading the list…"
            : blockedCount === 0
              ? "Nothing blocked — tap a category or an app to block it, one tap to allow it again"
              : `${blockedCats.size} ${blockedCats.size === 1 ? "category" : "categories"} and ${blocks.apps.length} ${blocks.apps.length === 1 ? "app" : "apps"} blocked on every device they use`}
        </p>
        {catalog.error && <p className="fam-error">Couldn't load the list: {catalog.error}</p>}
      </div>

      {cats.length > 0 && (
        <div className="apps-cats" role="group" aria-label="Categories">
          {cats.map((c) => {
            const on = blockedCats.has(c.id);
            return (
              <button
                key={c.id}
                className="apps-cat"
                data-on={on}
                disabled={busy}
                title={c.blurb}
                aria-pressed={on}
                onClick={() => toggleCategory(c.id, c.name)}
              >
                <span className="apps-cat-name">{c.name}</span>
                <span className="apps-cat-n">
                  {on ? "blocked" : c.app_ids.length > 0 ? `${c.app_ids.length} apps` : "sites"}
                </span>
              </button>
            );
          })}
        </div>
      )}

      {apps.length > 0 && (
        <>
          <div className="apps-grid" role="group" aria-label="Apps">
            {visible.map((a) => {
              const viaCat = covered(a);
              const on = viaCat || blockedApps.has(a.id);
              const catName = cats.find((c) => c.id === a.category)?.name ?? a.category;
              return (
                <button
                  key={a.id}
                  className="apps-tile"
                  data-on={on}
                  data-covered={viaCat}
                  disabled={busy || viaCat}
                  aria-pressed={on}
                  title={viaCat ? `Blocked with ${catName}` : on ? `Allow ${a.name}` : `Block ${a.name}`}
                  onClick={() => !viaCat && toggleApp(a.id, a.name)}
                >
                  <Monogram id={a.id} name={a.name} />
                  <span className="apps-tile-name">{a.name}</span>
                  <span className="apps-tile-state">{viaCat ? `via ${catName}` : on ? "blocked" : "allowed"}</span>
                </button>
              );
            })}
          </div>
          {sorted.length > FOLD && (
            <button className="ch-btn" style={{ justifySelf: "start" }} onClick={() => setShowAll((s) => !s)}>
              {showAll ? "Show fewer" : `All ${sorted.length} apps`}
            </button>
          )}
        </>
      )}

      <details className="apps-more">
        <summary>More — block a site by name</summary>
        <div className="rl-app">
          <ChipList
            items={blocks.custom_domains}
            busy={busy}
            placeholder="+ site, e.g. example.com"
            onAdd={(v) =>
              blocks.custom_domains.includes(v)
                ? undefined
                : save({ custom_domains: [...blocks.custom_domains, v] }, `${v} is blocked.`)
            }
            onRemove={(v) =>
              save({ custom_domains: blocks.custom_domains.filter((s) => s !== v) }, `${v} is allowed again.`)
            }
          />
        </div>
      </details>
    </div>
  );
}

// ---- daily limit -----------------------------------------------------------

function DailyLimit({ limit, busy, onSave }: { limit: number; busy: boolean; onSave: (m: number) => void }) {
  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what">
        <p className="rl-name">Daily limit</p>
        <p className="rl-value">
          {limit > 0
            ? `${fmtMin(limit)} of screen time a day — earned minutes come on top`
            : "No limit — the day is only shaped by hours and bedtime"}
        </p>
      </div>
      <FluentSlider
        min={0}
        max={480}
        step={15}
        value={limit}
        disabled={busy}
        aria-label="Daily screen-time limit"
        format={(v) => (v === 0 ? "No limit" : `${fmtMin(v)} / day`)}
        onCommit={onSave}
      />
    </div>
  );
}

// ---- allowed hours ---------------------------------------------------------

function HoursWindow({
  label,
  win,
  days,
  busy,
  onSet,
  onClear,
}: {
  label: string;
  win: TimeWindow | null;
  days: number[];
  busy: boolean;
  onSet: (start: string, end: string) => void;
  onClear: () => void;
}) {
  const [start, setStart] = useState(win?.start ?? "15:00");
  const [end, setEnd] = useState(win?.end ?? "19:00");
  useEffect(() => {
    if (win) {
      setStart(win.start);
      setEnd(win.end);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [win?.start, win?.end]);
  const dirty = win !== null && (start !== win.start || end !== win.end);
  void days;

  return (
    <div className="rl-app">
      <span className="rl-app-name">{label}</span>
      {win ? (
        <span className="rl-controls">
          <input type="time" className="rl-time" value={start} disabled={busy}
            onChange={(e) => setStart(e.target.value)} aria-label={`${label} start`} />
          <span className="rl-dash">–</span>
          <input type="time" className="rl-time" value={end} disabled={busy}
            onChange={(e) => setEnd(e.target.value)} aria-label={`${label} end`} />
          {dirty && (
            <button className="ch-btn ch-btn-yes" disabled={busy} onClick={() => onSet(start, end)}>
              Save
            </button>
          )}
          <button className="ch-btn" disabled={busy} onClick={onClear}>
            Any time
          </button>
        </span>
      ) : (
        <span className="rl-controls">
          <span className="rl-app-mins">any time</span>
          <button className="ch-btn" disabled={busy} onClick={() => onSet(start, end)}>
            Set {start} – {end}
          </button>
        </span>
      )}
    </div>
  );
}

function AllowedHours({
  st,
  busy,
  withScreenTime,
  onSave,
}: {
  st: Policy["screen_time"];
  busy: boolean;
  withScreenTime: (next: Partial<Policy["screen_time"]>) => Policy;
  onSave: (p: Policy, note: string) => void;
}) {
  const weekday = findWindow(st.schedule, WEEKDAYS);
  const weekend = findWindow(st.schedule, WEEKEND);

  function setWindow(days: number[], start: string, end: string, label: string) {
    const rest = st.schedule.filter((w) => !days.some((d) => w.days.includes(d)));
    onSave(
      withScreenTime({ schedule: [...rest, { days, start, end }] }),
      `${label}: screens allowed ${start} – ${end}.`,
    );
  }
  function clearWindow(days: number[], label: string) {
    onSave(
      withScreenTime({ schedule: st.schedule.filter((w) => !days.some((d) => w.days.includes(d))) }),
      `${label}: screens allowed any time.`,
    );
  }

  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what">
        <p className="rl-name">Allowed hours</p>
        <p className="rl-value">
          When screens work at all — outside these hours the device is a brick, limit or not
        </p>
      </div>
      <HoursWindow label="School days" win={weekday} days={WEEKDAYS} busy={busy}
        onSet={(s, e) => setWindow(WEEKDAYS, s, e, "School days")}
        onClear={() => clearWindow(WEEKDAYS, "School days")} />
      <HoursWindow label="Weekend" win={weekend} days={WEEKEND} busy={busy}
        onSet={(s, e) => setWindow(WEEKEND, s, e, "Weekend")}
        onClear={() => clearWindow(WEEKEND, "Weekend")} />
    </div>
  );
}

// ---- bedtime ---------------------------------------------------------------

function BedtimeRule({
  st,
  busy,
  withScreenTime,
  onSave,
}: {
  st: Policy["screen_time"];
  busy: boolean;
  withScreenTime: (next: Partial<Policy["screen_time"]>) => Policy;
  onSave: (p: Policy, note: string) => void;
}) {
  const [start, setStart] = useState(st.bedtime?.start ?? "20:00");
  const [end, setEnd] = useState(st.bedtime?.end ?? "07:00");
  useEffect(() => {
    if (st.bedtime) {
      setStart(st.bedtime.start);
      setEnd(st.bedtime.end);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [st.bedtime?.start, st.bedtime?.end]);
  const dirty = st.bedtime !== null && (start !== st.bedtime.start || end !== st.bedtime.end);

  return (
    <div className="rl-row">
      <div className="rl-what">
        <p className="rl-name">Bedtime</p>
        <p className="rl-value">
          {st.bedtime ? `Screens off ${st.bedtime.start} – ${st.bedtime.end}` : "No bedtime"}
        </p>
      </div>
      <span className="rl-controls">
        {st.bedtime ? (
          <>
            <input type="time" className="rl-time" value={start} disabled={busy}
              onChange={(e) => setStart(e.target.value)} aria-label="Bedtime start" />
            <span className="rl-dash">–</span>
            <input type="time" className="rl-time" value={end} disabled={busy}
              onChange={(e) => setEnd(e.target.value)} aria-label="Bedtime end" />
            {dirty && (
              <button className="ch-btn ch-btn-yes" disabled={busy}
                onClick={() => onSave(withScreenTime({ bedtime: { start, end } }), `Bedtime set: ${start} – ${end}.`)}>
                Save
              </button>
            )}
            <button className="ch-btn" disabled={busy}
              onClick={() => onSave(withScreenTime({ bedtime: null }), "Bedtime removed.")}>
              Remove
            </button>
          </>
        ) : (
          <button className="ch-btn" disabled={busy}
            onClick={() => onSave(withScreenTime({ bedtime: { start, end } }), `Bedtime set: ${start} – ${end}.`)}>
            Set {start} – {end}
          </button>
        )}
      </span>
    </div>
  );
}

// ---- websites --------------------------------------------------------------

function ChipList({
  items,
  busy,
  placeholder,
  onAdd,
  onRemove,
}: {
  items: string[];
  busy: boolean;
  placeholder: string;
  onAdd: (v: string) => void;
  onRemove: (v: string) => void;
}) {
  const [draft, setDraft] = useState("");
  function add() {
    const v = draft.trim().toLowerCase().replace(/^https?:\/\//, "").replace(/\/.*$/, "");
    if (!v) return;
    onAdd(v);
    setDraft("");
  }
  return (
    <div className="chips">
      {items.map((s) => (
        <span className="chip" key={s}>
          {s}
          <button className="chip-x" disabled={busy} aria-label={`Remove ${s}`} onClick={() => onRemove(s)}>
            ✕
          </button>
        </span>
      ))}
      <input
        className="chip-input"
        placeholder={placeholder}
        value={draft}
        disabled={busy}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && add()}
        onBlur={() => draft.trim() && add()}
        aria-label={placeholder}
      />
    </div>
  );
}

function Websites({ pol, busy, onSave }: { pol: Policy; busy: boolean; onSave: (p: Policy, note: string) => void }) {
  function saveDns(next: Partial<Policy["dns"]>, note: string) {
    // CONTRACT-0.6: the web is open by default; the blocklist is the whole
    // story. Saving from here also clears any legacy allowlist mode so an old
    // hand-edited profile relaxes into the current posture.
    onSave({ ...pol, dns: { ...pol.dns, mode: "allow_all", ...next } }, note);
  }

  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what">
        <p className="rl-name">Websites</p>
        <p className="rl-value">
          Everything works unless you block it — and what you block is really
          blocked, on every device they use
        </p>
      </div>
      <div className="rl-app">
        <span className="rl-app-name">Blocked</span>
        <ChipList
          items={pol.dns.blocklist}
          busy={busy}
          placeholder="+ site to block"
          onAdd={(v) =>
            pol.dns.blocklist.includes(v)
              ? undefined
              : saveDns({ blocklist: [...pol.dns.blocklist, v] }, `${v} is blocked.`)
          }
          onRemove={(v) =>
            saveDns({ blocklist: pol.dns.blocklist.filter((s) => s !== v) }, `${v} is unblocked.`)
          }
        />
      </div>
    </div>
  );
}

function SafeSearch({ pol, busy, onSave }: { pol: Policy; busy: boolean; onSave: (p: Policy, note: string) => void }) {
  return (
    <div className="rl-row">
      <div className="rl-what">
        <p className="rl-name">Safe search</p>
        <p className="rl-value">
          {pol.dns.safe_search
            ? "Search engines filter adult results — enforced at the network, not the browser"
            : "Search results are unfiltered"}
        </p>
      </div>
      <span className="rl-controls">
        <button className="ch-btn" disabled={busy}
          onClick={() =>
            onSave(
              { ...pol, dns: { ...pol.dns, safe_search: !pol.dns.safe_search } },
              pol.dns.safe_search ? "Safe search is off." : "Safe search is on.",
            )
          }>
          {pol.dns.safe_search ? "Turn off" : "Turn on"}
        </button>
      </span>
    </div>
  );
}

// ---- earning time back -----------------------------------------------------

function EarnTime({ pol, busy, onSave }: { pol: Policy; busy: boolean; onSave: (p: Policy, note: string) => void }) {
  const et = pol.gamification.earn_time;
  const [newTask, setNewTask] = useState("");

  function save(next: Partial<Policy["gamification"]["earn_time"]>, note: string) {
    onSave(
      { ...pol, gamification: { ...pol.gamification, earn_time: { ...et, ...next } } },
      note,
    );
  }

  function addTask() {
    const label = newTask.trim();
    if (!label) return;
    const id = label.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "task";
    save(
      { enabled: true, tasks: [...et.tasks, { id, label, reward_minutes: 15 }] },
      `"${label}" earns 15 min now.`,
    );
    setNewTask("");
  }

  function setReward(id: string, minutes: number) {
    const m = Math.min(60, Math.max(5, minutes));
    save(
      { tasks: et.tasks.map((t) => (t.id === id ? { ...t, reward_minutes: m } : t)) },
      `Reward set to ${m} min.`,
    );
  }

  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what" style={{ display: "flex", justifyContent: "space-between", gap: "1rem", alignItems: "flex-start" }}>
        <div>
          <p className="rl-name">Earning time back</p>
          <p className="rl-value">
            {et.enabled
              ? "Finished tasks turn into minutes — you approve each one"
              : "Off — extra time only when you give it"}
          </p>
        </div>
        <button className="ch-btn" disabled={busy}
          onClick={() => save({ enabled: !et.enabled }, et.enabled ? "Earning time is off." : "Earning time is on.")}>
          {et.enabled ? "Turn off" : "Turn on"}
        </button>
      </div>
      {et.enabled && (
        <>
          {et.tasks.map((t) => (
            <div className="rl-app" key={t.id}>
              <span className="rl-app-name">{t.label}</span>
              <span className="rl-app-mins">+{t.reward_minutes} min</span>
              <span className="rl-controls">
                <button className="ch-btn" disabled={busy || t.reward_minutes <= 5}
                  onClick={() => setReward(t.id, t.reward_minutes - 5)} aria-label={`Smaller reward for ${t.label}`}>
                  −5
                </button>
                <button className="ch-btn" disabled={busy || t.reward_minutes >= 60}
                  onClick={() => setReward(t.id, t.reward_minutes + 5)} aria-label={`Bigger reward for ${t.label}`}>
                  +5
                </button>
                <button className="chip-x" disabled={busy} aria-label={`Remove ${t.label}`}
                  onClick={() =>
                    save({ tasks: et.tasks.filter((x) => x.id !== t.id) }, `"${t.label}" removed.`)
                  }>
                  ✕
                </button>
              </span>
            </div>
          ))}
          <div className="rl-app">
            <input
              className="chip-input"
              style={{ width: "14rem" }}
              placeholder="+ task, e.g. Read for 20 min"
              value={newTask}
              disabled={busy}
              onChange={(e) => setNewTask(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && addTask()}
              aria-label="Add an earnable task"
            />
            {newTask.trim() && (
              <button className="ch-btn" disabled={busy} onClick={addTask}>
                Earns 15 min
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}

// ---- time's up -------------------------------------------------------------

const CHALLENGES: { key: UnlockChallenge; label: string; hint: string }[] = [
  { key: "wait", label: "Just wait", hint: "the screen stays off until tomorrow" },
  { key: "math", label: "Math problem", hint: "solving one earns a short extension" },
  { key: "parent_pin", label: "Parent code", hint: "only you can reopen the screen, with the code from your authenticator app" },
];

function TimesUp({ pol, busy, onSave }: { pol: Policy; busy: boolean; onSave: (p: Policy, note: string) => void }) {
  const current = pol.gamification.lockout.unlock_challenge;
  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what">
        <p className="rl-name">When time runs out</p>
        <p className="rl-value">
          The stop is always a hard stop — this only decides what, if anything, reopens it:{" "}
          {CHALLENGES.find((c) => c.key === current)?.hint}
        </p>
      </div>
      <div className="pills">
        {CHALLENGES.map((c) => (
          <button
            key={c.key}
            className="pill"
            data-on={current === c.key}
            disabled={busy}
            onClick={() =>
              current !== c.key &&
              onSave(
                {
                  ...pol,
                  gamification: {
                    ...pol.gamification,
                    lockout: { ...pol.gamification.lockout, enabled: true, unlock_challenge: c.key },
                  },
                },
                `When time runs out: ${c.label.toLowerCase()} — ${c.hint}.`,
              )
            }
          >
            {c.label}
          </button>
        ))}
      </div>
    </div>
  );
}
