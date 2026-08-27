// ============================================================================
// CHILD — everything about one person, and nothing about infrastructure.
//
// The ordering is deliberate and reflects what a parent came here to do:
//   1. how much time is left today          (the question they opened this for)
//   2. anything waiting on them             (a request they must answer)
//   3. the controls: pause, more time        (the thing they came to change)
//   4. the rules, the protection level
//   5. where they use it                    (machinery, smallest and last)
//
// Since 0.4 a child is a member account (key = account id) and everything
// here comes from the family store — one fetch shared with the rail — plus
// the audit feed for their devices. Lock state is honest: `locked` is what
// the devices report, `lock_pending` a pause still on its way.
// ============================================================================
import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { Link, useParams } from "react-router-dom";
import * as api from "../api";
import {
  AGE_BRACKETS,
  THEMES,
  defaultThemeFor,
  type AgeBracket,
  type EarnRequest,
  type Event,
  type Policy,
  type Theme,
} from "../types";
import {
  LEVELS,
  SecuritySlider,
  levelForPolicy,
  policyForLevel,
} from "../components/SecuritySlider";
import { Moments } from "../components/Moments";
import { useConfirm, StepUpCancelled } from "../lib/confirm";
import { useFamily, familyChanged } from "../lib/family";
import { Avatar } from "./Family";
import { Rules } from "./ChildRules";
import { useCountUp } from "../lib/useCountUp";
import { PageHead } from "../layout/PageHead";

function since(iso: string | null | undefined): string {
  if (!iso) return "never";
  const mins = Math.round((Date.now() - new Date(iso).getTime()) / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins} min ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs} h ago`;
  const days = Math.round(hrs / 24);
  return `${days} day${days === 1 ? "" : "s"} ago`;
}

/** Today's time, as the one big number this page exists to answer. */
function Today({ used, limit, earned }: { used: number; limit: number; earned: number }) {
  const total = limit + earned;
  const left = Math.max(0, total - used);
  const pct = total > 0 ? Math.min(100, Math.round((used / total) * 100)) : 0;
  const spent = total > 0 && used >= total;
  // The number counts to its new value — living data, honest ending.
  const shown = useCountUp(total === 0 ? used : spent ? 0 : left);
  return (
    <section className="ch-today">
      {total === 0 ? (
        <>
          <p className="ch-big">{shown}</p>
          <p className="ch-big-unit">minutes used today · no limit set</p>
        </>
      ) : (
        <>
          <p className="ch-big" data-spent={spent}>
            {shown}
          </p>
          <p className="ch-big-unit">
            {spent ? "no time left today" : "minutes left today"}
          </p>
          <div
            className="ch-bar"
            // One cell per 15 minutes, same grammar as the family cards.
            style={{ "--segs": Math.max(1, Math.round(total / 15)) } as CSSProperties}
          >
            <span className="ch-bar-fill" style={{ width: `${pct}%` }} data-spent={spent} />
          </div>
          <p className="ch-bar-note">
            {used} of {total} used{earned > 0 && ` · ${earned} earned`}
          </p>
        </>
      )}
    </section>
  );
}

/** The faces a parent can pick — stable, friendly, no uploads to moderate. */
const FACES = ["🦊", "🐼", "🦖", "🚀", "⚽", "🎨", "🐙", "🌟", "🦄", "🐸", "🎮", "🎧", "📚", "🌈", "🐳", "🐯"];

/** Age bracket + look + face, in the header. Each is one tap. */
function Identity({
  bracket,
  theme,
  avatar,
  busy,
  onBracket,
  onTheme,
  onAvatar,
}: {
  bracket: AgeBracket;
  theme: Theme | null;
  avatar: string | null | undefined;
  busy: boolean;
  onBracket: (b: AgeBracket) => void;
  onTheme: (t: Theme | null) => void;
  onAvatar: (a: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const auto = defaultThemeFor(bracket);
  const b = AGE_BRACKETS.find((x) => x.key === bracket);
  return (
    <div className="ch-ident">
      <button className="ch-ident-btn" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        {b ? `${b.label} · ${b.range}` : bracket}
        <span className="ch-ident-sep">·</span>
        {theme ? `${THEMES.find((t) => t.key === theme)?.label ?? theme} look` : `${THEMES.find((t) => t.key === auto)?.label ?? auto} look (auto)`}
        <span className="ch-ident-caret" aria-hidden="true">{open ? "▴" : "▾"}</span>
      </button>
      {open && (
        <div className="ch-ident-panel">
          <p className="rl-name">Age</p>
          <p className="rl-value">How much they decide for themselves, and how hard the stops are.</p>
          <div className="pills" style={{ marginTop: "0.5rem" }}>
            {AGE_BRACKETS.map((x) => (
              <button
                key={x.key}
                className="pill"
                data-on={x.key === bracket}
                disabled={busy}
                onClick={() => x.key !== bracket && onBracket(x.key)}
              >
                {x.label} <span className="pill-range">{x.range}</span>
              </button>
            ))}
          </div>
          <p className="rl-name" style={{ marginTop: "1rem" }}>Their face</p>
          <p className="rl-value">The icon that stands for them everywhere in the console.</p>
          <div className="pills faces" style={{ marginTop: "0.5rem" }}>
            <button className="pill" data-on={!avatar} disabled={busy} onClick={() => avatar && onAvatar("")}>
              Auto
            </button>
            {FACES.map((f) => (
              <button
                key={f}
                className="pill pill-face"
                data-on={avatar === f}
                disabled={busy}
                onClick={() => avatar !== f && onAvatar(f)}
                aria-label={`Use ${f} as their face`}
              >
                {f}
              </button>
            ))}
          </div>
          <p className="rl-name" style={{ marginTop: "1rem" }}>Their page</p>
          <p className="rl-value">How OpenScreenTime looks when they open it on their own computer.</p>
          <div className="pills" style={{ marginTop: "0.5rem" }}>
            <button className="pill" data-on={theme === null} disabled={busy} onClick={() => theme !== null && onTheme(null)}>
              Auto
            </button>
            {THEMES.map((t) => (
              <button
                key={t.key}
                className="pill"
                data-on={theme === t.key}
                disabled={busy}
                title={t.blurb}
                onClick={() => theme !== t.key && onTheme(t.key)}
              >
                {t.label}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

export function ChildDetail() {
  const { key = "" } = useParams();
  const { guard } = useConfirm();
  const fam = useFamily();
  const [events, setEvents] = useState<Event[]>([]);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const child = useMemo(() => fam.children.find((c) => c.key === key) ?? null, [fam.children, key]);
  const profile = useMemo(
    () => (child?.profile_id ? fam.profiles.find((p) => p.id === child.profile_id) ?? null : null),
    [fam.profiles, child],
  );
  const childDevices = useMemo(
    () =>
      (child?.devices ?? []).map((d) => ({
        ...d,
        full: fam.devices?.find((x) => x.id === d.id) ?? null,
      })),
    [child, fam.devices],
  );
  const deviceUserIds = useMemo(() => new Set((child?.devices ?? []).map((d) => d.device_user_id)), [child]);
  const requests = useMemo(
    () => fam.requests.filter((r) => deviceUserIds.has(r.device_user_id)),
    [fam.requests, deviceUserIds],
  );

  // The audit trail for this child's devices. Without it, a tamper event the
  // server recorded is never seen — the "never silent" promise depends on
  // this being on screen.
  const deviceIdsKey = (child?.devices ?? []).map((d) => d.id).sort().join(",");
  useEffect(() => {
    if (!deviceIdsKey) {
      setEvents([]);
      return;
    }
    let alive = true;
    void (async () => {
      const perDevice = await Promise.all(
        deviceIdsKey.split(",").map((id) => api.listEvents({ device_id: id, limit: 50 }).catch(() => [])),
      );
      if (!alive) return;
      setEvents(
        perDevice
          .flat()
          .sort((a, b) => b.created_at.localeCompare(a.created_at))
          .slice(0, 50),
      );
    })();
    return () => {
      alive = false;
    };
  }, [deviceIdsKey, fam.children]);

  if (fam.loading && !child)
    return (
      <div className="ch-wrap">
        <Link to="/" className="ch-back">← Family</Link>
        <p className="fam-quiet">Loading…</p>
      </div>
    );

  if (!child)
    return (
      <div className="ch-wrap">
        <Link to="/" className="ch-back">← Family</Link>
        <p className="fam-error">{fam.error ?? "There's no one with that name in your family."}</p>
        <button className="ch-btn" onClick={() => void fam.reload()}>
          Try again
        </button>
      </div>
    );

  const name = child.name;
  const used = child.used_minutes;
  const earned = child.earned_minutes;
  // A disabled schedule means no limit, whatever number the policy carries.
  const limit = child.limit_minutes ?? 0;
  const level = profile ? levelForPolicy(profile.policy) : 2;

  /** Every change funnels through here: step-up, do it, refetch the family. */
  async function change(doneNote: string, fn: () => Promise<unknown>, failNote: string) {
    setBusy(true);
    setNote(null);
    setError(null);
    try {
      await guard(fn);
      setNote(doneNote);
      await fam.reload();
    } catch (e) {
      if (e instanceof StepUpCancelled) return;
      setError(e instanceof Error ? e.message : failNote);
    } finally {
      setBusy(false);
      familyChanged();
    }
  }

  function setLevel(next: number) {
    if (!profile) return;
    void change(
      `Protection set to ${LEVELS[next].name}. It reaches the device within a minute.`,
      () => api.updateProfile(profile.id, policyForLevel(next, profile.policy)),
      "Could not change protection",
    );
  }

  function grant(minutes: number) {
    const du = child?.devices[0]?.device_user_id;
    if (!du) return;
    void change(`Gave ${name} ${minutes} more minutes.`, () => api.creditTime(du, minutes), "Could not grant time");
  }

  function saveRules(next: Policy, doneNote: string) {
    if (!profile) return;
    void change(doneNote, () => api.updateProfile(profile.id, next), "Could not change the rules");
  }

  function setBracket(b: AgeBracket) {
    const label = AGE_BRACKETS.find((x) => x.key === b)?.label ?? b;
    void change(
      `${name} is now in the ${label.toLowerCase()} bracket.`,
      () => api.updateMember(child!.account_id, { age_bracket: b }),
      "Could not change the age bracket",
    );
  }

  function setTheme(t: Theme | null) {
    void change(
      t ? `${name}'s page now uses the ${t} look.` : `${name}'s page follows their age bracket again.`,
      () => api.updateMember(child!.account_id, { theme: t }),
      "Could not change the look",
    );
  }

  function setAvatar(a: string) {
    void change(
      a ? `${name} is ${a} now.` : `${name}'s face is back to their initials.`,
      () => api.updateMember(child!.account_id, { avatar: a }),
      "Could not change the face",
    );
  }

  /** The hard stop: pause (lock) every device this child uses. */
  const allPaused = childDevices.length > 0 && childDevices.every((d) => d.locked);
  const anyPending = childDevices.some((d) => d.lock_pending);

  function pause(resume: boolean) {
    void change(
      resume
        ? `Resuming. ${name}'s devices confirm in a moment.`
        : `Pausing. It shows as paused once each device confirms.`,
      async () => {
        for (const d of childDevices) {
          if (resume) await api.unlockDevice(d.id);
          else if (!d.locked) await api.lockDevice(d.id);
        }
      },
      "Could not change the devices",
    );
  }

  function answer(r: EarnRequest, approve: boolean) {
    void change(
      approve ? `Allowed ${r.minutes} more minutes.` : "Said no.",
      () => (approve ? api.approveEarnRequest(r.id) : api.denyEarnRequest(r.id)),
      "Could not answer the request",
    );
  }

  return (
    <div className="ch-wrap">
      <PageHead
        back={{ to: "/", label: "Family" }}
        lead={<Avatar name={name} seed={key} avatar={child.avatar} size={64} />}
        title={name}
        sub={
          <>
            {profile?.name ?? "No rules yet"}
            {childDevices.length > 1 && ` · ${childDevices.length} devices`}
            {allPaused && " · paused"}
          </>
        }
      >
        <Identity
          bracket={child.age_bracket}
          theme={child.theme}
          avatar={child.avatar}
          busy={busy}
          onBracket={setBracket}
          onTheme={setTheme}
          onAvatar={setAvatar}
        />
      </PageHead>

      {note && <p className="ch-note">{note}</p>}
      {error && <p className="fam-error" style={{ marginBottom: "1rem" }}>{error}</p>}

      <Today used={used} limit={limit} earned={earned} />

      {requests.length > 0 && (
        <section className="ch-section">
          <h2 className="ch-h2">Waiting for you</h2>
          <ul className="ch-reqs">
            {requests.map((r) => (
              <li key={r.id} className="ch-req">
                <span>
                  Asked for <strong>{r.minutes} more minutes</strong>
                  {r.task_label && ` for ${r.task_label}`}
                </span>
                <span className="ch-req-btns">
                  <button className="ch-btn ch-btn-yes" disabled={busy} onClick={() => answer(r, true)}>
                    Allow
                  </button>
                  <button className="ch-btn" disabled={busy} onClick={() => answer(r, false)}>
                    No
                  </button>
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* The controls a parent came for, first: the hard stop, then more time. */}
      <div className="ch-actions">
        {childDevices.length > 0 &&
          (allPaused ? (
            <button
              className="ch-btn ch-btn-pause"
              data-paused="true"
              data-pending={anyPending}
              disabled={busy || anyPending}
              onClick={() => pause(true)}
            >
              {anyPending ? "Resuming…" : "Resume their devices"}
            </button>
          ) : (
            <button
              className="ch-btn ch-btn-pause"
              data-pending={anyPending}
              disabled={busy || anyPending}
              onClick={() => pause(false)}
            >
              {anyPending ? "Pausing…" : "Pause their devices"}
            </button>
          ))}
        <button className="ch-btn" disabled={busy || !childDevices.length} onClick={() => grant(15)}>
          +15 min today
        </button>
        <button className="ch-btn" disabled={busy || !childDevices.length} onClick={() => grant(30)}>
          +30 min today
        </button>
      </div>

      {profile && (
        <section className="ch-section">
          <h2 className="ch-h2">The rules</h2>
          <Rules profile={profile} busy={busy} onSave={saveRules} />
        </section>
      )}

      <section className="ch-section">
        {profile ? (
          <SecuritySlider value={level} busy={busy} onChange={setLevel} />
        ) : (
          <p className="fam-quiet">No rules assigned, so there is nothing to protect yet.</p>
        )}
      </section>

      <section className="ch-section">
        <h2 className="ch-h2">Where {name} uses it</h2>
        <ul className="ch-devices">
          {childDevices.map((d) => (
            <li key={d.id} className="ch-device">
              <span className="ch-device-name">
                <span className="ch-device-dot" data-state={d.status} aria-hidden="true" />
                {d.name}
              </span>
              <span className="ch-device-right">
                {d.lock_pending ? (
                  <span className="ch-device-state" data-state="pending">{d.locked ? "resuming…" : "pausing…"}</span>
                ) : d.locked ? (
                  <span className="ch-device-state" data-state="locked">paused</span>
                ) : null}
                <span className="ch-device-state" data-state={d.status}>
                  {d.status === "online"
                    ? "online"
                    : d.status === "pending"
                      ? "not set up yet"
                      : `offline · ${since(d.full?.last_seen)}`}
                </span>
              </span>
            </li>
          ))}
          {childDevices.length === 0 && (
            <li className="fam-quiet">
              No devices yet. <Link to="/add" style={{ color: "var(--fg)" }}>Set one up</Link>.
            </li>
          )}
        </ul>
      </section>

      {/* The day's story, not a log: only moments that mattered, and nothing
          at all on a healthy day. The raw feed lives with the machinery. */}
      <Moments events={events} />
    </div>
  );
}
