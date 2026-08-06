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
import { useEffect, useState } from "react";
import type { Policy, Profile, TimeWindow, UnlockChallenge } from "../types";
import { FluentSlider } from "../components/FluentSlider";

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
      <DailyLimit limit={limit} busy={busy} onSave={(m) =>
        onSave(
          withScreenTime({ daily_limit_minutes: m }),
          m === 0 ? "Daily limit removed." : `Daily limit set to ${fmtMin(m)}.`,
        )
      } />
      <AllowedHours st={st} busy={busy} withScreenTime={withScreenTime} onSave={onSave} />
      <BedtimeRule st={st} busy={busy} withScreenTime={withScreenTime} onSave={onSave} />
      <AppLimits pol={pol} busy={busy} onSave={onSave} />
      <Websites pol={pol} busy={busy} onSave={onSave} />
      <SafeSearch pol={pol} busy={busy} onSave={onSave} />
      <EarnTime pol={pol} busy={busy} onSave={onSave} />
      <TimesUp pol={pol} busy={busy} onSave={onSave} />
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

// ---- app limits ------------------------------------------------------------

function AppLimits({ pol, busy, onSave }: { pol: Policy; busy: boolean; onSave: (p: Policy, note: string) => void }) {
  const [newApp, setNewApp] = useState("");

  function setApp(match: string, minutes: number | null) {
    const app_limits =
      minutes === null
        ? pol.app_limits.filter((a) => a.match !== match)
        : pol.app_limits.some((a) => a.match === match)
          ? pol.app_limits.map((a) => (a.match === match ? { ...a, daily_limit_minutes: minutes } : a))
          : [...pol.app_limits, { match, daily_limit_minutes: minutes }];
    onSave(
      { ...pol, app_limits },
      minutes === null ? `Limit for ${match} removed.` : `${match} limited to ${fmtMin(minutes)} a day.`,
    );
  }

  function add() {
    const name = newApp.trim().toLowerCase();
    if (!name) return;
    setApp(name, 30);
    setNewApp("");
  }

  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what">
        <p className="rl-name">App limits</p>
        <p className="rl-value">
          {pol.app_limits.length === 0
            ? "No app has its own cap — the daily limit covers everything"
            : "Caps inside the daily limit — the app stops, the day continues"}
        </p>
      </div>
      {pol.app_limits.map((a) => (
        <div className="rl-app" key={a.match}>
          <span className="rl-app-name">{a.match}</span>
          <FluentSlider
            min={15}
            max={240}
            step={15}
            value={a.daily_limit_minutes}
            disabled={busy}
            aria-label={`Daily limit for ${a.match}`}
            format={(v) => `${fmtMin(v)} / day`}
            onCommit={(v) => setApp(a.match, v)}
          />
          <button className="chip-x" disabled={busy} aria-label={`Remove limit for ${a.match}`}
            onClick={() => setApp(a.match, null)}>
            ✕
          </button>
        </div>
      ))}
      <div className="rl-app">
        <input
          className="chip-input"
          placeholder="+ app, e.g. steam"
          value={newApp}
          disabled={busy}
          onChange={(e) => setNewApp(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && add()}
          aria-label="Add an app limit"
        />
        {newApp.trim() && (
          <button className="ch-btn" disabled={busy} onClick={add}>
            Limit to 30 min
          </button>
        )}
      </div>
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
  const openWeb = pol.dns.allowlist.includes("*");
  const allow = pol.dns.allowlist.filter((s) => s !== "*");

  function saveDns(next: Partial<Policy["dns"]>, note: string) {
    onSave({ ...pol, dns: { ...pol.dns, ...next } }, note);
  }

  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what">
        <p className="rl-name">Websites</p>
        <p className="rl-value">
          {openWeb
            ? "The web is open — blocked sites are the exception"
            : "Only approved sites work — everything else is off"}
        </p>
      </div>
      {!openWeb && (
        <div className="rl-app">
          <span className="rl-app-name">Approved</span>
          <ChipList
            items={allow}
            busy={busy}
            placeholder="+ site, e.g. wikipedia.org"
            onAdd={(v) =>
              allow.includes(v)
                ? undefined
                : saveDns({ allowlist: [...allow, v] }, `${v} is now approved.`)
            }
            onRemove={(v) =>
              saveDns({ allowlist: allow.filter((s) => s !== v) }, `${v} is no longer approved.`)
            }
          />
        </div>
      )}
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
  { key: "parent_pin", label: "Parent PIN", hint: "only you can reopen the screen" },
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
