// ============================================================================
// CHILD — everything about one person, and nothing about infrastructure.
//
// This replaces the device detail page as the place you actually spend time.
// The ordering is deliberate and reflects what a parent came here to do:
//   1. how much time is left today          (the question they opened this for)
//   2. anything waiting on them             (a request they must answer)
//   3. protection level                     (the thing they came to change)
//   4. where they use it                    (machinery, smallest and last)
// ============================================================================
import { useCallback, useEffect, useMemo, useState, type CSSProperties } from "react";
import { Link, useParams } from "react-router-dom";
import * as api from "../api";
import type { Device, DeviceUser, EarnRequest, Event, Policy, Profile } from "../types";
import {
  LEVELS,
  SecuritySlider,
  levelForPolicy,
  policyForLevel,
} from "../components/SecuritySlider";
import { EventFeed } from "../components/EventFeed";
import { useStepUp, StepUpCancelled } from "../lib/stepup";
import { familyChanged } from "../lib/family";

function hueFor(key: string): number {
  let h = 0;
  for (const ch of key) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return h;
}
function initials(name: string): string {
  const p = name.trim().split(/\s+/).filter(Boolean);
  if (!p.length) return "?";
  return (p.length === 1 ? p[0].slice(0, 2) : p[0][0] + p[p.length - 1][0]).toUpperCase();
}

/** Today's time, as the one big number this page exists to answer. */
function Today({ used, limit, earned }: { used: number; limit: number; earned: number }) {
  const total = limit + earned;
  const left = Math.max(0, total - used);
  const pct = total > 0 ? Math.min(100, Math.round((used / total) * 100)) : 0;
  const spent = total > 0 && used >= total;
  return (
    <section className="ch-today">
      {total === 0 ? (
        <>
          <p className="ch-big">{used}</p>
          <p className="ch-big-unit">minutes used today · no limit set</p>
        </>
      ) : (
        <>
          <p className="ch-big" data-spent={spent}>
            {spent ? 0 : left}
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

export function ChildDetail() {
  const { key = "" } = useParams();
  const { guard } = useStepUp();
  const [devices, setDevices] = useState<Device[]>([]);
  const [users, setUsers] = useState<(DeviceUser & { deviceName: string })[]>([]);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [requests, setRequests] = useState<EarnRequest[]>([]);
  const [events, setEvents] = useState<Event[]>([]);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [ds, ps] = await Promise.all([api.listDevices(), api.listProfiles()]);
      setDevices(ds);
      setProfiles(ps);
      const found: (DeviceUser & { deviceName: string })[] = [];
      for (const d of ds) {
        try {
          for (const u of await api.listDeviceUsers(d.id)) {
            if (u.os_username === key) found.push({ ...u, deviceName: d.name });
          }
        } catch {
          /* a single unreachable device must not blank the whole page */
        }
      }
      setUsers(found);
      setLoading(false);
      try {
        setRequests((await api.listEarnRequests("pending")).filter((r) => r.os_username === key));
      } catch {
        setRequests([]);
      }
      // The audit trail for this child's devices. Without it, a tamper event the
      // server recorded is never seen — the "never silent" promise depends on
      // this being on screen.
      try {
        const deviceIds = [...new Set(found.map((u) => u.device_id))];
        const perDevice = await Promise.all(
          deviceIds.map((id) => api.listEvents({ device_id: id, limit: 50 }).catch(() => [])),
        );
        const merged = perDevice
          .flat()
          .sort((a, b) => b.created_at.localeCompare(a.created_at))
          .slice(0, 50);
        setEvents(merged);
      } catch {
        setEvents([]);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load this child");
      setLoading(false);
    }
  }, [key]);

  useEffect(() => {
    void load();
  }, [load]);

  const name = users[0]?.display_name?.trim() || key;
  const used = users.reduce((n, u) => n + (u.used_minutes_today ?? 0), 0);
  const earned = users.reduce((n, u) => n + (u.earned_minutes_today ?? 0), 0);

  const profile = useMemo(
    () => profiles.find((p) => p.id === users[0]?.profile_id) ?? null,
    [profiles, users],
  );
  // A disabled schedule means no limit, whatever number the policy carries.
  const limit = profile?.policy.screen_time?.enabled
    ? (profile.policy.screen_time.daily_limit_minutes ?? 0)
    : 0;
  const level = profile ? levelForPolicy(profile.policy) : 2;

  if (loading)
    return (
      <div className="ch-wrap">
        <Link to="/" className="ch-back">← Family</Link>
        <p className="fam-quiet">Loading…</p>
      </div>
    );


  async function setLevel(next: number) {
    if (!profile) return;
    setBusy(true);
    setNote(null);
    try {
      await guard(() => api.updateProfile(profile.id, policyForLevel(next, profile.policy)));
      setNote(`Protection set to ${LEVELS[next].name}. It reaches the device within a minute.`);
      await load();
      familyChanged();
    } catch (e) {
      if (e instanceof StepUpCancelled) return;
      setError(e instanceof Error ? e.message : "Could not change protection");
    } finally {
      setBusy(false);
    }
  }

  async function grant(minutes: number) {
    const u = users[0];
    if (!u) return;
    setBusy(true);
    try {
      await guard(() => api.creditTime(u.id, minutes));
      setNote(`Gave ${name} ${minutes} more minutes.`);
      await load();
      familyChanged();
    } catch (e) {
      if (e instanceof StepUpCancelled) return;
      setError(e instanceof Error ? e.message : "Could not grant time");
    } finally {
      setBusy(false);
    }
  }

  /** Every rule edit funnels through here: step-up, save, reload. */
  async function saveRules(next: Policy, doneNote: string) {
    if (!profile) return;
    setBusy(true);
    setNote(null);
    try {
      await guard(() => api.updateProfile(profile.id, next));
      setNote(doneNote);
      await load();
      familyChanged();
    } catch (e) {
      if (e instanceof StepUpCancelled) return;
      setError(e instanceof Error ? e.message : "Could not change the rules");
    } finally {
      setBusy(false);
    }
  }

  /** The hard stop: pause (lock) every device this child uses. */
  const childDevices = devices.filter((d) => users.some((u) => u.device_id === d.id));
  const allPaused = childDevices.length > 0 && childDevices.every((d) => d.status === "locked");

  async function pause(resume: boolean) {
    setBusy(true);
    setNote(null);
    try {
      await guard(async () => {
        for (const d of childDevices) {
          if (resume) await api.unlockDevice(d.id);
          else if (d.status !== "locked") await api.lockDevice(d.id);
        }
      });
      setNote(
        resume
          ? `${name} can use their devices again.`
          : `Paused. Every device ${name} uses stops within a minute.`,
      );
      await load();
      familyChanged();
    } catch (e) {
      if (e instanceof StepUpCancelled) return;
      setError(e instanceof Error ? e.message : "Could not change the devices");
    } finally {
      setBusy(false);
    }
  }

  async function answer(r: EarnRequest, approve: boolean) {
    setBusy(true);
    try {
      await guard(() =>
        approve ? api.approveEarnRequest(r.id) : api.denyEarnRequest(r.id),
      );
      await load();
      familyChanged();
    } catch (e) {
      if (e instanceof StepUpCancelled) return;
      setError(e instanceof Error ? e.message : "Could not answer the request");
    } finally {
      setBusy(false);
    }
  }

  if (error)
    return (
      <div className="ch-wrap">
        {/* An error must never be a dead end with no way back. */}
        <Link to="/" className="ch-back">← Family</Link>
        <p className="fam-error">{error}</p>
        <button className="ch-btn" onClick={() => { setError(null); void load(); }}>
          Try again
        </button>
      </div>
    );

  return (
    <div className="ch-wrap">
      <Link to="/" className="ch-back">← Family</Link>

      <header className="ch-head">
        <span
          className="fam-avatar"
          style={{
            width: 64, height: 64, fontSize: 22,
            background: `hsl(${hueFor(key)} 45% 88%)`,
            color: `hsl(${hueFor(key)} 55% 26%)`,
          }}
          aria-hidden="true"
        >
          {initials(name)}
        </span>
        <div>
          <h1 className="ch-name">{name}</h1>
          <p className="ch-meta">
            {profile?.name ?? "No profile"}
            {users.length > 1 && ` · ${users.length} devices`}
          </p>
        </div>
      </header>

      {note && <p className="ch-note">{note}</p>}

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
                  <button className="ch-btn ch-btn-yes" disabled={busy} onClick={() => void answer(r, true)}>
                    Allow
                  </button>
                  <button className="ch-btn" disabled={busy} onClick={() => void answer(r, false)}>
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
            <button className="ch-btn ch-btn-pause" data-paused="true" disabled={busy} onClick={() => void pause(true)}>
              Resume their devices
            </button>
          ) : (
            <button className="ch-btn ch-btn-pause" disabled={busy} onClick={() => void pause(false)}>
              Pause their devices
            </button>
          ))}
        <button className="ch-btn" disabled={busy || !users.length} onClick={() => void grant(15)}>
          +15 min today
        </button>
        <button className="ch-btn" disabled={busy || !users.length} onClick={() => void grant(30)}>
          +30 min today
        </button>
      </div>

      {profile && (
        <section className="ch-section">
          <h2 className="ch-h2">The rules</h2>
          <Rules profile={profile} busy={busy} onSave={(p, note) => void saveRules(p, note)} />
        </section>
      )}

      <section className="ch-section">
        {profile ? (
          <SecuritySlider value={level} busy={busy} onChange={(l) => void setLevel(l)} />
        ) : (
          <p className="fam-quiet">No profile assigned, so there is nothing to protect yet.</p>
        )}
      </section>

      <section className="ch-section">
        <h2 className="ch-h2">Where {name} uses it</h2>
        <ul className="ch-devices">
          {users.map((u) => (
            <li key={u.id} className="ch-device">
              <span>{u.deviceName}</span>
              <span className="ch-device-state" data-state={devices.find((d) => d.id === u.device_id)?.status}>
                {devices.find((d) => d.id === u.device_id)?.status ?? "unknown"}
              </span>
            </li>
          ))}
          {users.length === 0 && <li className="fam-quiet">No devices yet.</li>}
        </ul>
      </section>

      {users.length > 0 && (
        <section className="ch-section">
          <h2 className="ch-h2">Recent activity</h2>
          <EventFeed events={events} emptyLabel="NOTHING RECORDED YET" />
        </section>
      )}
    </div>
  );
}

// ---- The rules -------------------------------------------------------------
// The actual parenting options, as quiet rows that commit on touch: daily
// limit, bedtime, per-app limits, earning time back. Every change goes
// through step-up and lands on the server as a whole policy.

function fmtMin(m: number): string {
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

interface RulesProps {
  profile: Profile;
  busy: boolean;
  onSave: (next: Policy, doneNote: string) => void;
}

function Rules({ profile, busy, onSave }: RulesProps) {
  const pol = profile.policy;
  const st = pol.screen_time;
  const limit = st.enabled ? st.daily_limit_minutes : 0;

  const [bedStart, setBedStart] = useState(st.bedtime?.start ?? "20:00");
  const [bedEnd, setBedEnd] = useState(st.bedtime?.end ?? "07:00");
  const [newApp, setNewApp] = useState("");
  // Keep the bedtime inputs in sync when a save comes back from the server.
  useEffect(() => {
    setBedStart(st.bedtime?.start ?? bedStart);
    setBedEnd(st.bedtime?.end ?? bedEnd);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [st.bedtime?.start, st.bedtime?.end]);
  const bedDirty =
    st.bedtime !== null && (bedStart !== st.bedtime.start || bedEnd !== st.bedtime.end);

  function withScreenTime(next: Partial<Policy["screen_time"]>): Policy {
    const merged = { ...pol, screen_time: { ...st, ...next } };
    merged.screen_time.enabled = screenTimeEnabled(merged);
    return merged;
  }

  function setLimit(minutes: number) {
    const m = Math.max(0, minutes);
    onSave(
      withScreenTime({ daily_limit_minutes: m }),
      m === 0 ? "Daily limit removed." : `Daily limit set to ${fmtMin(m)}.`,
    );
  }

  function setAppLimit(match: string, minutes: number | null) {
    const app_limits =
      minutes === null
        ? pol.app_limits.filter((a) => a.match !== match)
        : pol.app_limits.some((a) => a.match === match)
          ? pol.app_limits.map((a) =>
              a.match === match ? { ...a, daily_limit_minutes: Math.max(15, minutes) } : a,
            )
          : [...pol.app_limits, { match, daily_limit_minutes: Math.max(15, minutes) }];
    onSave(
      { ...pol, app_limits },
      minutes === null ? `Limit for ${match} removed.` : `${match} limited to ${fmtMin(Math.max(15, minutes))} a day.`,
    );
  }

  return (
    <div className="rl">
      {/* Daily limit */}
      <div className="rl-row">
        <div className="rl-what">
          <p className="rl-name">Daily limit</p>
          <p className="rl-value">{limit > 0 ? `${fmtMin(limit)} a day` : "No limit"}</p>
        </div>
        <span className="rl-controls">
          {limit > 0 ? (
            <>
              <button className="ch-btn" disabled={busy} onClick={() => setLimit(limit - 15)} aria-label="15 minutes less">
                −15
              </button>
              <button className="ch-btn" disabled={busy} onClick={() => setLimit(limit + 15)} aria-label="15 minutes more">
                +15
              </button>
            </>
          ) : (
            <button className="ch-btn" disabled={busy} onClick={() => setLimit(60)}>
              Set 1 h a day
            </button>
          )}
        </span>
      </div>

      {/* Bedtime */}
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
              <input
                type="time"
                className="rl-time"
                value={bedStart}
                disabled={busy}
                onChange={(e) => setBedStart(e.target.value)}
                aria-label="Bedtime start"
              />
              <span className="rl-dash">–</span>
              <input
                type="time"
                className="rl-time"
                value={bedEnd}
                disabled={busy}
                onChange={(e) => setBedEnd(e.target.value)}
                aria-label="Bedtime end"
              />
              {bedDirty && (
                <button
                  className="ch-btn ch-btn-yes"
                  disabled={busy}
                  onClick={() =>
                    onSave(
                      withScreenTime({ bedtime: { start: bedStart, end: bedEnd } }),
                      `Bedtime set: ${bedStart} – ${bedEnd}.`,
                    )
                  }
                >
                  Save
                </button>
              )}
              <button
                className="ch-btn"
                disabled={busy}
                onClick={() => onSave(withScreenTime({ bedtime: null }), "Bedtime removed.")}
              >
                Remove
              </button>
            </>
          ) : (
            <button
              className="ch-btn"
              disabled={busy}
              onClick={() =>
                onSave(
                  withScreenTime({ bedtime: { start: bedStart, end: bedEnd } }),
                  `Bedtime set: ${bedStart} – ${bedEnd}.`,
                )
              }
            >
              Set {bedStart} – {bedEnd}
            </button>
          )}
        </span>
      </div>

      {/* Per-app limits */}
      <div className="rl-row rl-row-stack">
        <div className="rl-what">
          <p className="rl-name">App limits</p>
          <p className="rl-value">
            {pol.app_limits.length === 0 ? "No app has its own limit" : "On top of the daily limit"}
          </p>
        </div>
        {pol.app_limits.map((a) => (
          <div className="rl-app" key={a.match}>
            <span className="rl-app-name">{a.match}</span>
            <span className="rl-app-mins">{fmtMin(a.daily_limit_minutes)}</span>
            <span className="rl-controls">
              <button
                className="ch-btn"
                disabled={busy || a.daily_limit_minutes <= 15}
                onClick={() => setAppLimit(a.match, a.daily_limit_minutes - 15)}
                aria-label={`15 minutes less for ${a.match}`}
              >
                −15
              </button>
              <button
                className="ch-btn"
                disabled={busy}
                onClick={() => setAppLimit(a.match, a.daily_limit_minutes + 15)}
                aria-label={`15 minutes more for ${a.match}`}
              >
                +15
              </button>
              <button className="ch-btn" disabled={busy} onClick={() => setAppLimit(a.match, null)}>
                Remove
              </button>
            </span>
          </div>
        ))}
        <div className="rl-app">
          <input
            className="add-input rl-app-input"
            placeholder="App name, e.g. steam"
            value={newApp}
            disabled={busy}
            onChange={(e) => setNewApp(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && newApp.trim()) {
                setAppLimit(newApp.trim().toLowerCase(), 30);
                setNewApp("");
              }
            }}
          />
          <button
            className="ch-btn"
            disabled={busy || !newApp.trim()}
            onClick={() => {
              setAppLimit(newApp.trim().toLowerCase(), 30);
              setNewApp("");
            }}
          >
            Limit to 30 min
          </button>
        </div>
      </div>

      {/* Earning time back */}
      <div className="rl-row">
        <div className="rl-what">
          <p className="rl-name">Earning time back</p>
          <p className="rl-value">
            {pol.gamification.earn_time.enabled
              ? pol.gamification.earn_time.tasks
                  .map((t) => `${t.label} · +${t.reward_minutes} min`)
                  .join("  ·  ") || "On, but no tasks set"
              : "Off — extra time only when you give it"}
          </p>
        </div>
        <span className="rl-controls">
          <button
            className="ch-btn"
            disabled={busy}
            onClick={() =>
              onSave(
                {
                  ...pol,
                  gamification: {
                    ...pol.gamification,
                    earn_time: {
                      ...pol.gamification.earn_time,
                      enabled: !pol.gamification.earn_time.enabled,
                    },
                  },
                },
                pol.gamification.earn_time.enabled
                  ? "Earning time is off."
                  : "Earning time is on.",
              )
            }
          >
            {pol.gamification.earn_time.enabled ? "Turn off" : "Turn on"}
          </button>
        </span>
      </div>
    </div>
  );
}
