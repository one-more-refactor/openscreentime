// ============================================================================
// FAMILY — the home screen.
//
// This console used to open on a device list, which meant the first thing a
// parent saw was infrastructure. The product is not a fleet. It is a handful
// of children and how their day is going: one card per child, today's time
// under their name, and nothing else competing. Machinery interrupts only
// when actually broken (<Trouble/> renders nothing on a healthy day).
// ============================================================================
import { type CSSProperties } from "react";
import { Link } from "react-router-dom";
import type { Device } from "../types";
import { useFamily, type FamilyChild } from "../lib/family";

/** Deterministic warm hue per child, so an avatar is recognisable at a glance. */
export function hueFor(key: string): number {
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

export function Avatar({ name, seed, size = 56 }: { name: string; seed: string; size?: number }) {
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
 * Today's time as a segmented bar — one cell per 15 minutes, so the diagram
 * is a picture of the day rather than decoration.
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
      <div
        className="fam-bar"
        role="img"
        aria-label={`${used} of ${total} minutes used`}
        style={{ "--segs": Math.max(1, Math.round(total / 15)) } as CSSProperties}
      >
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

function since(iso: string | null | undefined): string {
  if (!iso) return "";
  const mins = Math.round((Date.now() - new Date(iso).getTime()) / 60000);
  if (mins < 60) return `${mins} min ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs} hour${hrs === 1 ? "" : "s"} ago`;
  const days = Math.round(hrs / 24);
  return `${days} day${days === 1 ? "" : "s"} ago`;
}

/**
 * The only thing allowed to interrupt. Renders nothing on a healthy day.
 * A device inside an allowed-offline window is not trouble — the parent said
 * it may be away — and "pending" is normal minutes after setup.
 */
function Trouble({ devices }: { devices: Device[] }) {
  const dark = devices.filter(
    (d) =>
      d.status === "offline" &&
      !(d.offline_allowed_until && new Date(d.offline_allowed_until).getTime() > Date.now()),
  );
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

function ChildCard({ child }: { child: FamilyChild }) {
  return (
    <Link to={`/child/${encodeURIComponent(child.key)}`} className="fam-card">
      <Avatar name={child.name} seed={child.key} />
      <div className="fam-card-body">
        <p className="fam-name">{child.name}</p>
        <p className="fam-meta">
          {child.profileName ?? "No profile"}
          {child.devices.length > 1 && ` · ${child.devices.length} devices`}
        </p>
        <TimeBar used={child.usedMinutes} limit={child.limitMinutes} earned={child.earnedMinutes} />
        {child.pendingRequests > 0 && (
          <p className="fam-waiting">
            {child.pendingRequests === 1
              ? "1 request waiting for you"
              : `${child.pendingRequests} requests waiting for you`}
          </p>
        )}
      </div>
    </Link>
  );
}

export function Family() {
  const { devices, children, error } = useFamily();

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
              <ChildCard child={c} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
