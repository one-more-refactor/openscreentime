// ============================================================================
// FAMILY — the home screen.
//
// This console used to open on a device list, which meant the first thing a
// parent saw was infrastructure. The product is not a fleet. It is a handful
// of people and how their day is going: one card per person, today's time
// under their name, and nothing else competing. Machinery interrupts only
// when actually broken (<Trouble/> renders nothing on a healthy day).
//
// The one control that outranks the cards is Pause — the brief's "one tap
// freezes every screen in the house". It sits above them, and when it fires
// the freeze visibly sweeps across the family rather than the page silently
// re-rendering.
// ============================================================================
import { useState, type CSSProperties } from "react";
import { Link } from "react-router-dom";
import type { Device } from "../types";
import { useFamily, minutesLeft, minutesTotal, type FamilyChild } from "../lib/family";
import { PauseEverything } from "../components/PauseEverything";
import { useCountUp } from "../lib/useCountUp";

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
 * is a picture of the day rather than decoration. The fill animates from zero
 * on mount and the minutes count up to meet it.
 */
function TimeBar({ child }: { child: FamilyChild }) {
  const total = minutesTotal(child);
  const left = minutesLeft(child);
  // Unconditional: hooks cannot sit behind the no-limit early return below.
  const shown = useCountUp(left ?? 0);

  if (total === null || left === null) {
    return <p className="fam-time-none">{child.used_minutes} min today · no limit set</p>;
  }

  const pct = total > 0 ? Math.min(100, Math.round((child.used_minutes / total) * 100)) : 0;
  const spent = left === 0;

  return (
    <div className="fam-time">
      <div
        className="fam-bar"
        role="img"
        aria-label={`${child.used_minutes} of ${total} minutes used`}
        style={{ "--segs": Math.max(1, Math.round(total / 15)) } as CSSProperties}
      >
        <span className="fam-bar-fill" style={{ width: `${pct}%` }} data-spent={spent} />
      </div>
      <p className="fam-time-label">
        {spent ? (
          <span className="fam-spent">Time is up for today</span>
        ) : (
          <>
            <strong>{shown} min</strong> left of {total}
            {child.earned_minutes > 0 && (
              <span className="fam-earned"> · {child.earned_minutes} earned</span>
            )}
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
      <Link to="/devices" className="fam-trouble-go">
        Devices →
      </Link>
    </p>
  );
}

const BRACKET_LABEL: Record<FamilyChild["age_bracket"], string> = {
  little: "Little",
  kid: "Kid",
  younger_teen: "Younger teen",
  older_teen: "Older teen",
  adult: "Adult",
};

function ChildCard({ child, index }: { child: FamilyChild; index: number }) {
  // `locked` is what the devices actually report; a pause in flight is shown
  // as exactly that, never as already done.
  const paused = child.locked && child.devices.length > 0;
  const pending = child.devices.some((d) => d.lock_pending);
  return (
    <Link
      to={`/child/${encodeURIComponent(child.key)}`}
      className="fam-card"
      data-paused={paused}
      data-pending={pending}
      // Staggered so the freeze sweeps across the family left-to-right
      // instead of every card blinking at once.
      style={{ "--i": index } as CSSProperties}
    >
      <Avatar name={child.name} seed={child.key} />
      <div className="fam-card-body">
        <p className="fam-name">{child.name}</p>
        <p className="fam-meta">
          {BRACKET_LABEL[child.age_bracket] ?? child.age_bracket}
          {child.profile_name && ` · ${child.profile_name}`}
          {child.devices.length > 1 && ` · ${child.devices.length} devices`}
        </p>
        <TimeBar child={child} />
        {child.pending_requests > 0 && (
          <p className="fam-waiting">
            {child.pending_requests === 1
              ? "1 request waiting for you"
              : `${child.pending_requests} requests waiting for you`}
          </p>
        )}
      </div>
      {paused ? (
        <span className="fam-card-paused" aria-label="Paused">
          Paused
        </span>
      ) : pending ? (
        <span className="fam-card-paused fam-card-pending" aria-label="Pausing">
          Pausing…
        </span>
      ) : null}
    </Link>
  );
}

/**
 * The waiting state. Not a shimmer: the real layout, drawn in outline, so the
 * page does not jump when the data lands. One card per person we don't know
 * about yet — two is the honest guess.
 */
function FamilyWaiting() {
  return (
    <ul className="fam-grid" aria-busy="true" aria-label="Loading the family">
      {[0, 1].map((i) => (
        <li key={i}>
          <div className="fam-card fam-card-wait" style={{ "--i": i } as CSSProperties}>
            <span className="fam-avatar fam-wait-block" style={{ width: 56, height: 56 }} />
            <div className="fam-card-body">
              <span className="fam-wait-line" style={{ width: "38%", height: "1.125rem" }} />
              <span className="fam-wait-line" style={{ width: "26%", height: "0.8rem" }} />
              <span className="fam-wait-line" style={{ width: "100%", height: "10px", marginTop: "0.7rem" }} />
              <span className="fam-wait-line" style={{ width: "45%", height: "0.9rem" }} />
            </div>
          </div>
        </li>
      ))}
    </ul>
  );
}

export function Family() {
  const { devices, children, error, loading, refreshing, reload } = useFamily();
  const [sweeping, setSweeping] = useState(false);

  const hour = new Date().getHours();
  const greeting = hour < 11 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening";

  // Every device that could be paused. Pending devices have no agent yet.
  const pausable = (devices ?? []).filter((d) => d.status !== "pending");
  const allPaused = pausable.length > 0 && pausable.every((d) => d.locked);

  return (
    <div className="fam-wrap" data-sweeping={sweeping} data-refreshing={refreshing}>
      <header className="fam-head">
        <h1 className="fam-greet">{greeting}</h1>
        <Link to="/add" className="fam-add">
          + Add a child
        </Link>
        <p className="fam-sub">
          {loading && children.length === 0
            ? " "
            : children.length === 0
              ? "No children set up yet."
              : `${children.length} ${children.length === 1 ? "child" : "children"} today`}
        </p>
      </header>

      {error && <p className="fam-error">{error}</p>}

      {pausable.length > 0 && (
        <PauseEverything
          devices={pausable}
          allPaused={allPaused}
          onSweep={setSweeping}
          onDone={reload}
        />
      )}

      {devices && <Trouble devices={devices} />}

      {loading && children.length === 0 ? (
        <FamilyWaiting />
      ) : children.length === 0 ? (
        <div className="fam-empty">
          <p>Once a device is set up, the people using it appear here.</p>
          <Link to="/add" className="fam-cta">
            Add a child
          </Link>
        </div>
      ) : (
        <ul className="fam-grid">
          {children.map((c, i) => (
            <li key={c.key}>
              <ChildCard child={c} index={i} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
