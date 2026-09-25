// ============================================================================
// A person's rules (the second half of the person page). Every control maps
// to one field the computer really enforces, and every change is one save:
//
//   Daily limit     a slider; 0 is no limit. Given minutes come on top.
//   Allowed hours   school days and the weekend, each "any time" or a window.
//   Bedtime         screens off overnight, whatever the limit says.
//   Blocked         one list: categories, apps, and sites by name — plus safe
//                   search. Everything else works.
//   Earning time    tasks that turn into minutes, each one approved by you.
//   Remove          at the bottom, quietly, behind typing their name.
//
// Someone who keeps their own time (an adult) has no editor here: their rules
// are theirs, and the server wouldn't show them to you anyway.
// ============================================================================
import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import * as api from "../api";
import { EMPTY_BLOCKS, type AppBlocks, type Catalog, type Policy, type TimeWindow } from "../types";
import { useAsync } from "../lib/useAsync";
import { bedtimeProblem, describeWindow, windowProblem } from "../lib/schedule";
import { normalizeSite } from "../lib/myRules";
import { notBlockedSentence } from "../lib/degraded";
import { duration } from "../lib/format";
import { FluentSlider } from "../components/FluentSlider";
import { AppGlyph } from "../components/AppGlyph";
import { Button } from "../components/Button";
import { Icon } from "../components/Icon";
import { Modal } from "../components/Modal";
import { TextInput } from "../components/TextInput";
import { keepsOwnTime, type PersonCtx } from "./Person";

const WEEKDAYS = [1, 2, 3, 4, 5];
const WEEKEND = [0, 6];

/** screen_time.enabled must survive "no daily limit" while hours or a bedtime remain. */
function withScreenTime(pol: Policy, next: Partial<Policy["screen_time"]>): Policy {
  const st = { ...pol.screen_time, ...next };
  st.enabled = st.daily_limit_minutes > 0 || st.bedtime !== null || st.schedule.length > 0;
  return { ...pol, screen_time: st };
}

/** The one list of blocked sites: the hand-typed ones plus any the old
 * editor kept in dns.blocklist. Saving folds them into one place. */
function sitesOf(p: Policy): string[] {
  return [...new Set([...(p.blocks?.custom_domains ?? []), ...(p.dns.blocklist ?? [])])];
}
function withSites(p: Policy, sites: string[]): Policy {
  return {
    ...p,
    dns: { ...p.dns, blocklist: [] },
    blocks: { ...(p.blocks ?? EMPTY_BLOCKS), custom_domains: sites },
  };
}

function Section({ title, sub, children }: { title: string; sub?: string; children: React.ReactNode }) {
  return (
    <section className="section">
      <div className="section-head pp-section-head">
        <h2 className="h2">{title}</h2>
        {sub && <p className="meta">{sub}</p>}
      </div>
      {children}
    </section>
  );
}

/** HH:MM, typed — 24-hour everywhere, the way every time here reads. */
export function TimeField({
  value,
  onChange,
  label,
  disabled,
}: {
  value: string;
  onChange: (v: string) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <input
      className="field pp-time num"
      inputMode="numeric"
      placeholder="HH:MM"
      maxLength={5}
      value={value}
      disabled={disabled}
      aria-label={label}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}

/** A window that can be "off" (any time / no bedtime) or set, edited in place. */
function WindowRow({
  title,
  win,
  offLabel,
  clearLabel,
  setLabel,
  defaults,
  problemOf,
  busy,
  overnight = false,
  onSet,
  onClear,
}: {
  title: string;
  win: { start: string; end: string } | null;
  offLabel: string;
  clearLabel: string;
  setLabel: string;
  defaults: [string, string];
  problemOf: (start: string, end: string) => string | null;
  busy: boolean;
  /** A bedtime runs overnight by nature — no "(next day)" to say. */
  overnight?: boolean;
  onSet: (start: string, end: string) => void;
  onClear: () => void;
}) {
  const reads = (s: string, e: string) => (overnight ? `${s} – ${e === "00:00" ? "midnight" : e}` : describeWindow(s, e));
  const [editing, setEditing] = useState(false);
  const [start, setStart] = useState(win?.start ?? defaults[0]);
  const [end, setEnd] = useState(win?.end ?? defaults[1]);
  useEffect(() => {
    if (!editing) {
      setStart(win?.start ?? defaults[0]);
      setEnd(win?.end ?? defaults[1]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [win?.start, win?.end, editing]);
  const problem = problemOf(start, end);

  return (
    <div className="row row-wrap pp-window" data-editing={editing}>
      <div className="row-main">
        <p className="row-title">{title}</p>
        <p className="row-sub num">{win ? reads(win.start, win.end) : offLabel}</p>
      </div>
      {!editing && (
        <div className="row-end">
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => setEditing(true)}>
            {win ? "Change" : setLabel}
          </Button>
          {win && (
            <Button size="sm" variant="quiet" disabled={busy} onClick={onClear}>
              {clearLabel}
            </Button>
          )}
        </div>
      )}
      {editing && (
        <form
          className="pp-edit"
          onSubmit={(e) => {
            e.preventDefault();
            if (problem) return;
            onSet(start, end);
            setEditing(false);
          }}
        >
          <TimeField label={`${title} from`} value={start} onChange={setStart} disabled={busy} />
          <span className="pp-dash">–</span>
          <TimeField label={`${title} until`} value={end} onChange={setEnd} disabled={busy} />
          <Button size="sm" type="submit" disabled={busy || problem !== null}>
            Save
          </Button>
          <Button size="sm" variant="quiet" onClick={() => setEditing(false)}>
            Cancel
          </Button>
          <p className="hint pp-edit-note" data-error={problem !== null} role={problem ? "alert" : undefined}>
            {problem ??
              (overnight
                ? `Screens off ${reads(start, end)}, every night.`
                : `Screens on ${describeWindow(start, end)}. 00:00 is midnight; hours can run past it.`)}
          </p>
        </form>
      )}
    </div>
  );
}

function DailyLimit({ pol, busy, save }: { pol: Policy; busy: boolean; save: (p: Policy, done: string) => void }) {
  const limit = pol.screen_time.enabled ? pol.screen_time.daily_limit_minutes : 0;
  return (
    <Section title="Daily limit" sub="Screen time a day, across all their computers. Minutes you give come on top.">
      <div className="card card-pad pp-limit">
        <FluentSlider
          min={0}
          max={480}
          step={15}
          value={limit}
          disabled={busy}
          aria-label="Daily limit"
          format={(v) => (v === 0 ? "No limit" : `${duration(v)} a day`)}
          onCommit={(m) =>
            save(
              withScreenTime(pol, { daily_limit_minutes: m }),
              m === 0 ? "No daily limit now." : `Daily limit set to ${duration(m)}.`,
            )
          }
        />
      </div>
    </Section>
  );
}

function Hours({ pol, busy, save }: { pol: Policy; busy: boolean; save: (p: Policy, done: string) => void }) {
  const st = pol.screen_time;
  const find = (days: number[]) => st.schedule.find((w) => days.some((d) => w.days.includes(d))) ?? null;
  const rest = (days: number[]) => st.schedule.filter((w) => !days.some((d) => w.days.includes(d)));
  const set = (days: number[], label: string) => (start: string, end: string) =>
    save(
      withScreenTime(pol, { schedule: [...rest(days), { days, start, end } satisfies TimeWindow] }),
      `${label}: screens on ${describeWindow(start, end)}.`,
    );
  const clear = (days: number[], label: string) => () =>
    save(withScreenTime(pol, { schedule: rest(days) }), `${label}: any time.`);

  return (
    <Section
      title="When screens can be on"
      sub="Any time means no restriction. Bedtime turns screens off every night, whatever time is left."
    >
      <div className="card">
          <WindowRow
            title="School days"
            win={find(WEEKDAYS)}
            offLabel="Any time"
            clearLabel="Any time"
            setLabel="Set hours"
            defaults={["15:00", "19:00"]}
            problemOf={windowProblem}
            busy={busy}
            onSet={set(WEEKDAYS, "School days")}
            onClear={clear(WEEKDAYS, "School days")}
          />
          <WindowRow
            title="Weekend"
            win={find(WEEKEND)}
            offLabel="Any time"
            clearLabel="Any time"
            setLabel="Set hours"
            defaults={["09:00", "19:00"]}
            problemOf={windowProblem}
            busy={busy}
            onSet={set(WEEKEND, "Weekend")}
            onClear={clear(WEEKEND, "Weekend")}
          />
          <WindowRow
            title="Bedtime"
            win={st.bedtime}
            offLabel="No bedtime"
            clearLabel="No bedtime"
            setLabel="Set a bedtime"
            defaults={["20:00", "07:00"]}
            problemOf={bedtimeProblem}
            busy={busy}
            overnight
            onSet={(start, end) =>
              save(withScreenTime(pol, { bedtime: { start, end } }), `Bedtime ${start} – ${end}.`)
            }
            onClear={() => save(withScreenTime(pol, { bedtime: null }), "No bedtime now.")}
          />
      </div>
    </Section>
  );
}

/** One blocklist: categories, apps, sites by name. Everything else works.
 * `notBlocked`: a computer of theirs can't filter websites, said right here
 * instead of promising a block it can't keep. */
function Blocked({
  pol,
  busy,
  save,
  notBlocked,
}: {
  pol: Policy;
  busy: boolean;
  save: (p: Policy, done: string) => void;
  notBlocked: string | null;
}) {
  const catalog = useAsync<Catalog>(api.getCatalog, []);
  const blocks: AppBlocks = pol.blocks ?? EMPTY_BLOCKS;
  const [showAll, setShowAll] = useState(false);
  const [draft, setDraft] = useState("");
  const [siteProblem, setSiteProblem] = useState<string | null>(null);
  const cats = catalog.data?.categories ?? [];
  const apps = catalog.data?.apps ?? [];
  const blockedCats = useMemo(() => new Set(blocks.categories), [blocks.categories]);
  const blockedApps = useMemo(() => new Set(blocks.apps), [blocks.apps]);
  const sites = sitesOf(pol);

  const saveBlocks = (next: Partial<AppBlocks>, done: string) => save({ ...pol, blocks: { ...blocks, ...next } }, done);
  const toggleCat = (id: string, name: string) =>
    saveBlocks(
      { categories: blockedCats.has(id) ? blocks.categories.filter((c) => c !== id) : [...blocks.categories, id] },
      blockedCats.has(id) ? `${name}: allowed again.` : `${name}: blocked.`,
    );
  const toggleApp = (id: string, name: string) =>
    saveBlocks(
      { apps: blockedApps.has(id) ? blocks.apps.filter((a) => a !== id) : [...blocks.apps, id] },
      blockedApps.has(id) ? `${name} is allowed again.` : `${name} is blocked.`,
    );

  function addSite(e: React.FormEvent) {
    e.preventDefault();
    const n = normalizeSite(draft);
    if ("problem" in n) {
      setSiteProblem(n.problem);
      return;
    }
    setSiteProblem(null);
    setDraft("");
    if (sites.includes(n.site)) return;
    save(withSites(pol, [...sites, n.site]), `${n.site} is blocked.`);
  }

  // Blocked first, then the rest; beyond twelve the grid folds unless opened.
  const covered = (a: { category: string }) => blockedCats.has(a.category);
  const sorted = [...apps].sort(
    (a, b) => Number(!(blockedApps.has(a.id) || covered(a))) - Number(!(blockedApps.has(b.id) || covered(b))),
  );
  const FOLD = 12;
  const visible = showAll ? sorted : sorted.slice(0, FOLD);

  return (
    <Section
      title="Blocked"
      sub={
        notBlocked
          ? "Everything works unless it's here."
          : "Everything works unless it's here — and what's here is really blocked, on every computer they use."
      }
    >
      <div className="card card-pad pp-blocked">
        {notBlocked && (
          <p className="banner banner-warn pp-noblock" role="status">
            <Icon name="warning" size={18} />
            <span className="banner-main">{notBlocked}</span>
          </p>
        )}
        {catalog.error && <p className="hint" data-error="true">Couldn't load the list of apps: {catalog.error}</p>}

        {cats.length > 0 && (
          <div className="pp-block-group">
            <h3 className="wt-h">Categories</h3>
            <div className="pp-chips" role="group" aria-label="Categories">
              {cats.map((c) => {
                const on = blockedCats.has(c.id);
                return (
                  <button
                    key={c.id}
                    type="button"
                    className="blk"
                    aria-pressed={on}
                    disabled={busy}
                    title={c.blurb}
                    onClick={() => toggleCat(c.id, c.name)}
                  >
                    {on && <Icon name="block" size={14} />}
                    {c.name}
                  </button>
                );
              })}
            </div>
          </div>
        )}

        {apps.length > 0 && (
          <div className="pp-block-group">
            <h3 className="wt-h">Apps</h3>
            <div className="pp-apps" role="group" aria-label="Apps">
              {visible.map((a) => {
                const via = covered(a);
                const on = via || blockedApps.has(a.id);
                const catName = cats.find((c) => c.id === a.category)?.name ?? a.category;
                return (
                  <button
                    key={a.id}
                    type="button"
                    className="pp-app"
                    aria-pressed={on}
                    data-covered={via}
                    disabled={busy || via}
                    title={via ? `Blocked with ${catName}` : on ? `Allow ${a.name}` : `Block ${a.name}`}
                    onClick={() => !via && toggleApp(a.id, a.name)}
                  >
                    <AppGlyph id={a.id} name={a.name} />
                    <span className="pp-app-name">{a.name}</span>
                    <span className="pp-app-state">{via ? `With ${catName.toLowerCase()}` : on ? "Blocked" : "Allowed"}</span>
                  </button>
                );
              })}
            </div>
            {sorted.length > FOLD && (
              <Button size="sm" variant="quiet" icon={showAll ? "chevron-down" : "chevron-right"} onClick={() => setShowAll((s) => !s)}>
                {showAll ? "Fewer apps" : `All ${sorted.length} apps`}
              </Button>
            )}
          </div>
        )}

        <div className="pp-block-group">
          <h3 className="wt-h">Sites</h3>
          {sites.length > 0 && (
            <ul className="pp-chips" aria-label="Blocked sites">
              {sites.map((s) => (
                <li key={s} className="chip">
                  <Icon name="globe" size={14} />
                  {s}
                  <button
                    type="button"
                    className="chip-x"
                    disabled={busy}
                    aria-label={`Allow ${s} again`}
                    onClick={() => save(withSites(pol, sites.filter((x) => x !== s)), `${s} is allowed again.`)}
                  >
                    <Icon name="close" size={14} />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <form className="pp-add-site" onSubmit={addSite}>
            <TextInput
              aria-label="Block a site by name"
              placeholder="Block a site by name, e.g. example.com"
              value={draft}
              disabled={busy}
              error={siteProblem}
              onChange={(e) => {
                setDraft(e.target.value);
                setSiteProblem(null);
              }}
            />
            <Button type="submit" variant="secondary" icon="block" disabled={busy || !draft.trim()}>
              Block
            </Button>
          </form>
        </div>

        <div className="row pp-safe">
          <div className="row-main">
            <p className="row-title">Safe search</p>
            <p className="row-sub">
              {pol.dns.safe_search
                ? "On — Google, Bing and YouTube hide explicit results."
                : "Off — search results aren't filtered."}
            </p>
          </div>
          <div className="seg" role="group" aria-label="Safe search">
            {[true, false].map((v) => (
              <button
                key={String(v)}
                type="button"
                className="seg-btn"
                aria-pressed={pol.dns.safe_search === v}
                disabled={busy}
                onClick={() =>
                  pol.dns.safe_search !== v &&
                  save({ ...pol, dns: { ...pol.dns, safe_search: v } }, v ? "Safe search is on." : "Safe search is off.")
                }
              >
                {v ? "On" : "Off"}
              </button>
            ))}
          </div>
        </div>
      </div>
    </Section>
  );
}

function Earning({ pol, busy, save }: { pol: Policy; busy: boolean; save: (p: Policy, done: string) => void }) {
  const et = pol.gamification.earn_time;
  const [task, setTask] = useState("");
  const put = (next: Partial<Policy["gamification"]["earn_time"]>, done: string) =>
    save({ ...pol, gamification: { ...pol.gamification, earn_time: { ...et, ...next } } }, done);

  function add(e: React.FormEvent) {
    e.preventDefault();
    const label = task.trim();
    if (!label) return;
    const base = label.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "task";
    let id = base;
    for (let i = 2; et.tasks.some((t) => t.id === id); i++) id = `${base}-${i}`;
    put({ enabled: true, tasks: [...et.tasks, { id, label, reward_minutes: 15 }] }, `“${label}” earns 15 min.`);
    setTask("");
  }
  const reward = (id: string, m: number) => {
    const v = Math.min(60, Math.max(5, m));
    put({ tasks: et.tasks.map((t) => (t.id === id ? { ...t, reward_minutes: v } : t)) }, `Reward set to ${v} min.`);
  };

  return (
    <Section title="Earning time" sub="Things they can do for more minutes. You say yes to each one.">
      <div className="card">
        <div className="row">
          <div className="row-main">
            <p className="row-title">Tasks for more minutes</p>
            <p className="row-sub">{et.enabled ? "On — a finished task turns into minutes once you say yes." : "Off — more time only when you give it."}</p>
          </div>
          <div className="seg" role="group" aria-label="Earning time">
            {[true, false].map((v) => (
              <button
                key={String(v)}
                type="button"
                className="seg-btn"
                aria-pressed={et.enabled === v}
                disabled={busy}
                onClick={() => et.enabled !== v && put({ enabled: v }, v ? "Earning time is on." : "Earning time is off.")}
              >
                {v ? "On" : "Off"}
              </button>
            ))}
          </div>
        </div>
        {et.enabled && (
          <>
            {et.tasks.map((t) => (
              <div key={t.id} className="row">
                <div className="row-main">
                  <p className="row-title">{t.label}</p>
                  <p className="row-sub num">{t.reward_minutes} more minutes</p>
                </div>
                <div className="row-end">
                  <Button size="sm" variant="secondary" aria-label={`Smaller reward for ${t.label}`} disabled={busy || t.reward_minutes <= 5} onClick={() => reward(t.id, t.reward_minutes - 5)}>
                    −5
                  </Button>
                  <Button size="sm" variant="secondary" aria-label={`Bigger reward for ${t.label}`} disabled={busy || t.reward_minutes >= 60} onClick={() => reward(t.id, t.reward_minutes + 5)}>
                    +5
                  </Button>
                  <button
                    type="button"
                    className="btn-icon btn-icon-sm"
                    aria-label={`Remove ${t.label}`}
                    disabled={busy}
                    onClick={() => put({ tasks: et.tasks.filter((x) => x.id !== t.id) }, `“${t.label}” removed.`)}
                  >
                    <Icon name="remove" size={18} />
                  </button>
                </div>
              </div>
            ))}
            <form className="row pp-add-task" onSubmit={add}>
              <TextInput
                aria-label="A task they can do"
                placeholder="A task, e.g. Read for 20 minutes"
                value={task}
                disabled={busy}
                onChange={(e) => setTask(e.target.value)}
                maxLength={60}
              />
              <Button type="submit" variant="secondary" icon="add" disabled={busy || !task.trim()}>
                Add
              </Button>
            </form>
          </>
        )}
      </div>
    </Section>
  );
}

/** The one irreversible thing on the page — quiet until reached for. */
function Remove({ ctx }: { ctx: PersonCtx }) {
  const { child, busy, change } = ctx;
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState("");
  return (
    <section className="section pp-danger">
      <div className="card">
        <div className="row row-wrap">
          <div className="row-main">
            <p className="row-title">Remove {child.name}</p>
            <p className="row-sub">
              Deletes their account, their rules and everything you can see about their day. Their logins stay on the
              computers, unmanaged.
            </p>
          </div>
          <div className="row-end">
            <Button
              size="sm"
              variant="danger"
              icon="remove"
              disabled={busy}
              onClick={() => {
                setTyped("");
                setOpen(true);
              }}
            >
              Remove {child.name}
            </Button>
          </div>
        </div>
      </div>
      <Modal
        open={open}
        onClose={() => setOpen(false)}
        title={`Remove ${child.name}?`}
        danger
        footer={
          <>
            <Button variant="quiet" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button
              variant="danger-solid"
              disabled={busy || typed.trim() !== child.name}
              onClick={() => {
                setOpen(false);
                void change(`${child.name} was removed.`, async () => {
                  await api.deleteMember(child.account_id);
                  navigate("/");
                });
              }}
            >
              Remove {child.name}
            </Button>
          </>
        }
      >
        <div className="stack">
          <p className="dialog-lede">
            This deletes {child.name}'s account, their rules and everything you can see about their day. It can't be
            undone. Their logins stay on the computers, unmanaged.
          </p>
          <TextInput
            label={`Type ${child.name} to confirm`}
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            autoComplete="off"
          />
        </div>
      </Modal>
    </section>
  );
}

export function PersonRules({ ctx }: { ctx: PersonCtx }) {
  const { child, profile, busy, change } = ctx;

  if (keepsOwnTime(child)) {
    return (
      <>
        <section className="section">
          <div className="card card-pad pp-own">
            <Icon name="person" size={24} />
            <div>
              <h2 className="h2">{child.name} sets their own rules</h2>
              <p className="lede">
                Their daily limit, focus hours and the sites they block are theirs, on their own page — you don't see
                them and can't change them. You can still pause their computers from Today.
              </p>
            </div>
          </div>
        </section>
        <Remove ctx={ctx} />
      </>
    );
  }

  if (!profile) {
    return (
      <>
        <section className="section">
          <p className="card card-pad meta">{child.name} has no rules yet — they get their age's once their computer is set up.</p>
        </section>
        <Remove ctx={ctx} />
      </>
    );
  }

  const pol = profile.policy;
  const save = (next: Policy, done: string) =>
    void change(`${done} It reaches their computer within a minute.`, () => api.updateProfile(profile.id, next));
  // What their computers really report, not what the rules ask for.
  const notBlocked = notBlockedSentence(
    ctx.devices.map((d) => ({ name: d.name, status: d.full?.status ?? d.status, last_state: d.full?.last_state })),
  );

  return (
    <>
      <DailyLimit pol={pol} busy={busy} save={save} />
      <Hours pol={pol} busy={busy} save={save} />
      <Blocked pol={pol} busy={busy} save={save} notBlocked={notBlocked} />
      <Earning pol={pol} busy={busy} save={save} />
      <Remove ctx={ctx} />
    </>
  );
}
