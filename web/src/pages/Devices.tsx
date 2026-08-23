// ============================================================================
// DEVICES — the machinery, kept human.
//
// One card per device, answering only what a parent actually asks:
//   is it steady? · is it blocked? · when did it last call home? ·
//   is it allowed to be offline right now?
// No hostnames, no IPs, no agent versions, no tokens — those are back-of-house.
// The page's primary layer is a single verdict sentence; on a healthy day it
// says so and the red never appears.
// ============================================================================
import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import * as api from "../api";
import type { Device } from "../types";
import { useChangeMode, StepUpCancelled } from "../lib/changemode";
import { familyChanged } from "../lib/family";
import { StateRing, type RingTone } from "../components/StateRing";
import { PageHead } from "../layout/PageHead";

function minsSince(iso: string | null | undefined): number | null {
  if (!iso) return null;
  return Math.max(0, Math.round((Date.now() - new Date(iso).getTime()) / 60000));
}

function agoLabel(iso: string | null | undefined): string {
  const m = minsSince(iso);
  if (m === null) return "never";
  if (m < 1) return "just now";
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  return `${d} day${d === 1 ? "" : "s"} ago`;
}

function leftLabel(iso: string): string {
  const mins = Math.max(0, Math.round((new Date(iso).getTime() - Date.now()) / 60000));
  if (mins < 60) return `${mins} min`;
  return `${Math.floor(mins / 60)} h ${String(mins % 60).padStart(2, "0")} min`;
}

/** Minutes until 07:00 tomorrow — "allow offline until tomorrow morning". */
function untilTomorrowMinutes(): number {
  const t = new Date();
  t.setDate(t.getDate() + 1);
  t.setHours(7, 0, 0, 0);
  return Math.round((t.getTime() - Date.now()) / 60000);
}

function offlineAllowed(d: Device): boolean {
  return !!d.offline_allowed_until && new Date(d.offline_allowed_until).getTime() > Date.now();
}

/** The card's one-word state, in human words. */
function stateOf(d: Device): { word: string; tone: "ok" | "crit" | "warn" | "idle" } {
  // A pause is only "paused" once the agent has said so. Until then it is a
  // wish in flight, and the card says exactly that.
  if (d.lock_pending) return { word: d.locked ? "resuming…" : "pausing…", tone: "warn" };
  if (d.locked) return { word: "paused", tone: "crit" };
  if (d.status === "pending") return { word: "waiting to join", tone: "idle" };
  if (d.status === "offline")
    return offlineAllowed(d)
      ? { word: "away · allowed", tone: "idle" }
      : { word: "not calling home", tone: "warn" };
  return { word: "connected", tone: "ok" };
}

/** The ring: color = state, arc = connection freshness, dashed = allowed away. */
function ringOf(d: Device): { arc: number; tone: RingTone; dashed: boolean } {
  if (d.locked) return { arc: 1, tone: "crit", dashed: false };
  if (d.status === "pending") return { arc: 0.25, tone: "idle", dashed: false };
  if (d.status === "offline" && offlineAllowed(d)) return { arc: 1, tone: "idle", dashed: true };
  const m = minsSince(d.last_seen);
  const arc =
    m === null ? 0.1
    : m <= 2 ? 1
    : m <= 10 ? 0.85
    : m <= 60 ? 0.6
    : m <= 360 ? 0.4
    : m <= 1440 ? 0.25
    : 0.1;
  const tone: RingTone =
    d.status === "offline" ? "warn" : m !== null && m > 10 ? "warn" : "ok";
  return { arc, tone, dashed: false };
}

function initialsOf(name: string): string {
  const p = name.trim().split(/\s+/).filter(Boolean);
  if (!p.length) return "?";
  return (p.length === 1 ? p[0].slice(0, 2) : p[0][0] + p[p.length - 1][0]).toUpperCase();
}

/** Connection steadiness, derived from how recently the agent called home. */
function steadiness(d: Device): { label: string; tone?: "ok" | "warn" | "crit" } {
  if (d.status === "pending") return { label: "—" };
  const m = minsSince(d.last_seen);
  if (m === null) return { label: "silent" };
  if (m <= 5) return { label: "steady", tone: "ok" };
  if (m <= 60) return { label: "patchy", tone: "warn" };
  return { label: "silent", tone: offlineAllowed(d) ? undefined : "warn" };
}

function DeviceCard({ device, onChanged }: { device: Device; onChanged: () => void }) {
  const { guard } = useChangeMode();
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [statusTone, setStatusTone] = useState<"crit" | undefined>();
  const [pickingDuration, setPickingDuration] = useState(false);

  const d = device;
  const state = stateOf(d);
  const steady = steadiness(d);
  const blocked = d.locked;
  const away = offlineAllowed(d);
  const who = (d.users ?? [])
    .map((u) => u.display_name?.trim() || u.os_username)
    .join(" · ");

  async function run(label: string, fn: () => Promise<unknown>) {
    setBusy(true);
    setStatus(null);
    setStatusTone(undefined);
    try {
      await guard(fn);
      setStatus(label);
      setPickingDuration(false);
      onChanged();
      familyChanged();
    } catch (e) {
      if (e instanceof StepUpCancelled) return;
      setStatus(e instanceof Error ? e.message : "That didn't work");
      setStatusTone("crit");
    } finally {
      setBusy(false);
    }
  }

  return (
    <li className="dev-card" data-blocked={blocked}>
      <div className="dev-card-head">
        <div className="dev-card-id">
          <StateRing {...ringOf(d)} label={initialsOf(d.name)} />
          <div>
            <h2 className="dev-name">{d.name}</h2>
            <p className="dev-users">{who || "nobody yet"}</p>
          </div>
        </div>
        <span className="dev-state" data-tone={state.tone}>
          {state.word}
        </span>
      </div>

      {d.status !== "pending" && (
        <dl className="dev-facts">
          <div className="dev-fact">
            <dt>last heard</dt>
            <dd>{agoLabel(d.last_seen)}</dd>
          </div>
          <div className="dev-fact">
            <dt>connection</dt>
            <dd data-tone={steady.tone}>{steady.label}</dd>
          </div>
          {(d.pending_commands?.length ?? 0) > 0 && (
            <div className="dev-fact">
              <dt>changes</dt>
              <dd>on their way</dd>
            </div>
          )}
          {d.recovery_codes_unused !== undefined && (
            <div className="dev-fact">
              <dt>recovery codes</dt>
              <dd data-tone={d.recovery_codes_unused === 0 ? "warn" : undefined}>
                {d.recovery_codes_unused === 0
                  ? "none made"
                  : `${d.recovery_codes_unused} left`}
              </dd>
            </div>
          )}
        </dl>
      )}

      {d.status === "pending" ? (
        <p className="dev-offline-note">
          Set up, but it hasn't joined yet. <Link to="/add" style={{ color: "var(--fg)" }}>Finish setting it up</Link>.
        </p>
      ) : (
        <>
          {away && d.offline_allowed_until && (
            <div className="dev-offline">
              <p className="dev-offline-note">
                Allowed to be offline · <strong>{leftLabel(d.offline_allowed_until)}</strong> left
              </p>
              <button
                className="ch-btn"
                disabled={busy}
                onClick={() => void run("Offline window ended.", () => api.setOfflineWindow(d.id, null))}
              >
                End early
              </button>
            </div>
          )}

          <div className="dev-actions">
            {blocked ? (
              <button
                className="ch-btn"
                disabled={busy || d.lock_pending}
                onClick={() => void run("Resuming — the device confirms in a moment.", () => api.unlockDevice(d.id))}
              >
                {d.lock_pending ? "Resuming…" : "Resume"}
              </button>
            ) : (
              <button
                className="ch-btn"
                disabled={busy || d.lock_pending}
                onClick={() => void run("Pausing — it shows as paused once the device confirms.", () => api.lockDevice(d.id))}
              >
                {d.lock_pending ? "Pausing…" : "Pause now"}
              </button>
            )}

            {!away &&
              (pickingDuration ? (
                <span className="dev-durations">
                  <span className="label">allow offline for</span>
                  <button className="ch-btn" disabled={busy} onClick={() => void run("Offline allowed for 1 hour.", () => api.setOfflineWindow(d.id, 60))}>
                    1 h
                  </button>
                  <button className="ch-btn" disabled={busy} onClick={() => void run("Offline allowed for 4 hours.", () => api.setOfflineWindow(d.id, 240))}>
                    4 h
                  </button>
                  <button
                    className="ch-btn"
                    disabled={busy}
                    onClick={() => void run("Offline allowed until tomorrow morning.", () => api.setOfflineWindow(d.id, untilTomorrowMinutes()))}
                  >
                    Until tomorrow
                  </button>
                  <button className="ch-btn no-code" disabled={busy} onClick={() => setPickingDuration(false)}>
                    Cancel
                  </button>
                </span>
              ) : (
                <button className="ch-btn no-code" disabled={busy} onClick={() => setPickingDuration(true)}>
                  Allow offline…
                </button>
              ))}
          </div>
        </>
      )}

      {status && (
        <p className="dev-inline-status" data-tone={statusTone} role="status">
          {status}
        </p>
      )}
    </li>
  );
}

export function Devices() {
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      // Clone: the mock returns a stable array reference, and an identical
      // reference makes React skip the re-render that updates the verdict.
      setDevices([...(await api.listDevices())]);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load the devices");
    }
  }, []);

  useEffect(() => {
    void load();
    // Countdown labels and "last heard" drift by the minute — keep them honest.
    const t = setInterval(() => void load(), 30_000);
    return () => clearInterval(t);
  }, [load]);

  if (error)
    return (
      <div className="dev-wrap">
        <PageHead eyebrow="Devices" title="Couldn't load the devices." />
        <p className="fam-error">{error}</p>
      </div>
    );
  if (!devices)
    return (
      <div className="dev-wrap">
        <PageHead eyebrow="Devices" title={<span className="ph-wait">Checking on every device…</span>} />
      </div>
    );

  const blocked = devices.filter((d) => d.locked);
  const dark = devices.filter((d) => d.status === "offline" && !offlineAllowed(d));
  const verdict =
    blocked.length > 0
      ? `${blocked[0].name}${blocked.length > 1 ? ` and ${blocked.length - 1} more` : ""} is paused.`
      : dark.length > 0
        ? `${dark[0].name} hasn't called home in a while.`
        : "Every device is doing what it should.";

  return (
    <div className="dev-wrap">
      <PageHead eyebrow="Devices" title={verdict} />

      {devices.length === 0 ? (
        <div className="dev-empty">
          <p>No devices yet. A device joins when you set up a child on it.</p>
          <Link to="/add" className="fam-cta">
            Set one up
          </Link>
        </div>
      ) : (
        <ul className="dev-grid">
          {devices.map((d) => (
            <DeviceCard key={d.id} device={d} onChanged={() => void load()} />
          ))}
        </ul>
      )}
    </div>
  );
}
