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
import { Rules } from "./ChildRules";
import { useCountUp } from "../lib/useCountUp";

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
        <button className="ch-btn no-code" onClick={() => { setError(null); void load(); }}>
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
