// ============================================================================
// FAMILY — the home screen.
//
// This console used to open on a device list, which meant the first thing a
// parent saw was infrastructure: hostnames, enrollment states, LED strips. The
// product is not a fleet. It is three children and how their day is going.
//
// So: one card per child, avatar and name first, today's screen time under it,
// and nothing else competing. Devices, tokens, enrollment and server health are
// machinery — they belong in the background and should only ever interrupt when
// something is actually broken (see <Trouble/> at the bottom, which renders
// nothing at all on a healthy day).
// ============================================================================
import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import * as api from "../api";
import type { Device, DeviceUser, Profile } from "../types";

/** A child, assembled from whatever devices they have an account on. */
interface Child {
  key: string;
  name: string;
  usedMinutes: number;
  earnedMinutes: number;
  limitMinutes: number | null;
  profileName: string | null;
  devices: { id: string; name: string; status: Device["status"] }[];
}

/** Deterministic warm hue per child, so an avatar is recognisable at a glance. */
function hueFor(key: string): number {
  let h = 0;
  for (const ch of key) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return h;
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

function Avatar({ name, seed, size = 56 }: { name: string; seed: string; size?: number }) {
  const hue = hueFor(seed);
  return (
    <span
      className="fam-avatar"
      style={{
        width: size,
        height: size,
        fontSize: size * 0.34,
        background: `hsl(${hue} 45% 88%)`,
        color: `hsl(${hue} 55% 26%)`,
      }}
      aria-hidden="true"
    >
      {initials(name)}
    </span>
  );
}

/**
 * Today's time as a single bar. Deliberately not a ring or a gauge: a parent
 * reads "how much is left" in under a second, and a bar makes the remainder
 * legible at a glance in a way a donut does not.
 */
function TimeBar({ used, limit, earned }: { used: number; limit: number | null; earned: number }) {
  if (limit === null) {
    return <p className="fam-time-none">{used} min today · no limit set</p>;
  }
  const total = limit + earned;
  const pct = total > 0 ? Math.min(100, Math.round((used / total) * 100)) : 0;
  const left = Math.max(0, total - used);
  const spent = pct >= 100;
  return (
    <div className="fam-time">
      <div className="fam-bar" role="img" aria-label={`${used} of ${total} minutes used`}>
        <span className="fam-bar-fill" style={{ width: `${pct}%` }} data-spent={spent} />
      </div>
      <p className="fam-time-label">
        {spent ? (
          <span className="fam-spent">Time is up for today</span>
        ) : (
          <>
            <strong>{left} min</strong> left of {total}
            {earned > 0 && <span className="fam-earned"> · {earned} earned</span>}
          </>
        )}
      </p>
    </div>
  );
}

/**
 * The only thing allowed to interrupt. Renders nothing on a healthy day —
 * "you just notice when something dramatically fails" is a UI requirement, not
 * a wish, so this is a single line and never a dashboard.
 */
function since(iso: string | null | undefined): string {
  if (!iso) return "";
  const mins = Math.round((Date.now() - new Date(iso).getTime()) / 60000);
  if (mins < 60) return `${mins} min ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs} hour${hrs === 1 ? "" : "s"} ago`;
  const days = Math.round(hrs / 24);
  return `${days} day${days === 1 ? "" : "s"} ago`;
}

function Trouble({ devices }: { devices: Device[] }) {
  // "pending" means set up but never contacted — that is normal for minutes
  // after adding a child, so alarming a parent about it trains them to ignore
  // the one alert this app ever shows.
  const dark = devices.filter((d) => d.status === "offline");
  if (dark.length === 0) return null;
  const ago = since(dark[0].last_seen);
  return (
    <p className="fam-trouble">
      <span className="fam-trouble-dot" aria-hidden="true" />
      {dark.length === 1
        ? `${dark[0].name} was last seen ${ago || "a while ago"}. If it is switched off, that is normal.`
        : `${dark.length} computers haven't been seen recently. If they are switched off, that is normal.`}
    </p>
  );
}

export function Family() {
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [usersByDevice, setUsersByDevice] = useState<Record<string, DeviceUser[]>>({});
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const [ds, ps] = await Promise.all([api.listDevices(), api.listProfiles()]);
        if (!alive) return;
        setProfiles(ps);
        if (!alive) return;
        setDevices(ds);
        // Children live on devices, so the family view has to gather them.
        // A dedicated endpoint would be better; noted rather than faked.
        const entries = await Promise.all(
          ds.map(async (d) => {
            try {
              return [d.id, await api.listDeviceUsers(d.id)] as const;
            } catch {
              return [d.id, [] as DeviceUser[]] as const;
            }
          }),
        );
        if (!alive) return;
        setUsersByDevice(Object.fromEntries(entries));
      } catch (e) {
        if (alive) setError(e instanceof Error ? e.message : "Could not load the family");
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  const children = useMemo<Child[]>(() => {
    if (!devices) return [];
    const byKey = new Map<string, Child>();
    for (const d of devices) {
      for (const u of usersByDevice[d.id] ?? []) {
        const key = u.os_username;
        const existing = byKey.get(key);
        const name = u.display_name?.trim() || u.os_username;
        if (existing) {
          // Same person on a second machine: their day is the sum of both.
          existing.usedMinutes += u.used_minutes_today ?? 0;
          existing.earnedMinutes += u.earned_minutes_today ?? 0;
          existing.devices.push({ id: d.id, name: d.name, status: d.status });
        } else {
          byKey.set(key, {
            key,
            name,
            usedMinutes: u.used_minutes_today ?? 0,
            earnedMinutes: u.earned_minutes_today ?? 0,
            limitMinutes:
              profiles.find((p) => p.id === u.profile_id)?.policy.screen_time
                ?.daily_limit_minutes ?? null,
            profileName: u.profile_name ?? null,
            devices: [{ id: d.id, name: d.name, status: d.status }],
          });
        }
      }
    }
    return [...byKey.values()].sort((a, b) => a.name.localeCompare(b.name));
  }, [devices, usersByDevice, profiles]);

  if (error) {
    return (
      <div className="fam-wrap">
        <p className="fam-error">{error}</p>
      </div>
    );
  }

  if (!devices) {
    return (
      <div className="fam-wrap">
        <p className="fam-quiet">Loading…</p>
      </div>
    );
  }

  const hour = new Date().getHours();
  const greeting = hour < 11 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening";

  return (
    <div className="fam-wrap">
      <header className="fam-head">
        <h1 className="fam-greet">{greeting}</h1>
        <Link to="/add" className="fam-add">+ Add a child</Link>
        <p className="fam-sub">
          {children.length === 0
            ? "No children set up yet."
            : `${children.length} ${children.length === 1 ? "child" : "children"} today`}
        </p>
      </header>

      <Trouble devices={devices} />

      {children.length === 0 ? (
        <div className="fam-empty">
          <p>Once a device is set up, the people using it appear here.</p>
          <Link to="/add" className="fam-cta">
            Add a child
          </Link>
        </div>
      ) : (
        <ul className="fam-grid">
          {children.map((c) => (
            <li key={c.key}>
              <Link to={`/child/${encodeURIComponent(c.key)}`} className="fam-card">
                <Avatar name={c.name} seed={c.key} />
                <div className="fam-card-body">
                  <p className="fam-name">{c.name}</p>
                  <p className="fam-meta">
                    {c.profileName ?? "No profile"}
                    {c.devices.length > 1 && ` · ${c.devices.length} devices`}
                  </p>
                  <TimeBar used={c.usedMinutes} limit={c.limitMinutes} earned={c.earnedMinutes} />
                </div>
              </Link>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
