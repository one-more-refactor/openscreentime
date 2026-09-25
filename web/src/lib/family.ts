// ============================================================================
// The family, fetched once.
//
// This used to assemble itself in the browser: list devices, list profiles,
// then one request per device for its users, then the pending asks. Worse, the
// hook had no shared state — the navigation rail and the page each mounted it,
// so a three-device family paid a dozen round trips *twice* on every single
// navigation, and the two copies could briefly disagree about who was over
// their limit.
//
// Now the server answers all of it at `GET /api/family` in a fixed number of
// queries, and this module is a single store every view subscribes to: one
// fetch, one truth, no matter how many components are watching.
//
// While anyone is watching, it refreshes itself quietly — every 20 seconds,
// every 4 while a pause is still on its way to a computer, every 5 while
// someone's screen stops within five minutes — so a new request for time
// appears, "Pausing…" becomes "Paused" and "1 min left" becomes "Time's up"
// without navigating (the computer reports its minutes every 30 s). A
// hidden tab doesn't poll; coming back to it refreshes at once. A failed
// refresh keeps the last good snapshot on screen.
// ============================================================================
import { useEffect, useState } from "react";
import * as api from "../api";
import type { Device, EarnRequest, FamilyChild, Profile } from "../types";
import { stopIsNear } from "./day";

export type { FamilyChild } from "../types";

/** Time left — the server's number: minutes until their screen stops, the
 *  same verdict their computer shows (an unlock code's or a grant's time
 *  included). Without one (an old server), limit + earned − used. */
export function minutesLeft(c: FamilyChild): number | null {
  if (c.limit_minutes === null) return null;
  if (typeof c.left_minutes === "number") return Math.max(0, c.left_minutes);
  return Math.max(0, c.limit_minutes + c.earned_minutes - c.used_minutes);
}

/** Why someone's screen is stopped right now. */
export type StopKind = "limit" | "bedtime" | "outside_hours" | "paused";

/** The verdict's own reason for a stop — so a bedtime never reads as "Time's
 * up" — or null while they may use the screen. */
export function stoppedBy(c: FamilyChild): StopKind | null {
  const r = c.rules;
  if (r && !r.allowed) return r.reason ?? "limit";
  // An older server without a verdict: out of minutes is all it can say.
  if (!r && minutesLeft(c) === 0) return "limit";
  return null;
}

/** Total minutes available today: the limit plus anything earned on top. */
export function minutesTotal(c: FamilyChild): number | null {
  if (c.limit_minutes === null) return null;
  return c.limit_minutes + c.earned_minutes;
}

/** What the ring fills toward: time used plus time left — so the ring and the
 *  number agree, and an override running past the limit is time, not a full
 *  red ring. Null = no limit (the empty track). */
export function ringTarget(used: number, left: number | null): number | null {
  return left === null ? null : used + left;
}

export interface FamilyState {
  devices: Device[] | null;
  children: FamilyChild[];
  profiles: Profile[];
  requests: EarnRequest[];
  error: string | null;
  /** True until the first successful load — drives skeletons, not spinners. */
  loading: boolean;
  /** A refresh is in flight over data already on screen. Show a hairline, not
   *  a blank page: replacing good content with a spinner reads as slower. */
  refreshing: boolean;
}

const EMPTY: FamilyState = {
  devices: null,
  children: [],
  profiles: [],
  requests: [],
  error: null,
  loading: true,
  refreshing: false,
};

// ---- the store -------------------------------------------------------------

let state: FamilyState = EMPTY;
const listeners = new Set<(s: FamilyState) => void>();
/** In-flight request, so N mounting components cause exactly one fetch. */
let inflight: Promise<void> | null = null;

const POLL_MS = 20_000;
/** While a pause or resume is still on its way to a computer. */
const PENDING_POLL_MS = 4_000;
/** While someone's screen stops within five minutes: each minute the computer
 * reports shows here within seconds, not up to twenty. */
const NEAR_STOP_POLL_MS = 5_000;

/** How long until the next quiet refresh of what's on screen. */
export function pollDelay(s: Pick<FamilyState, "devices" | "children">, now = Date.now()): number {
  if (s.devices?.some((d) => d.lock_pending)) return PENDING_POLL_MS;
  if (s.children.some((c) => !c.locked && stopIsNear(c.rules, now))) return NEAR_STOP_POLL_MS;
  return POLL_MS;
}

let timer: ReturnType<typeof setTimeout> | null = null;

function emit(next: Partial<FamilyState>) {
  state = { ...state, ...next };
  for (const l of listeners) l(state);
}

/** Fetch the family. `quiet` refreshes (the ambient poll) show no hairline. */
async function load(quiet = false): Promise<void> {
  // Coalesce: the rail and the page mount in the same tick.
  if (inflight) return inflight;
  if (!quiet) emit(state.devices ? { refreshing: true } : { loading: true });
  inflight = (async () => {
    try {
      const f = await api.getFamily();
      emit({
        devices: f.devices,
        children: f.children,
        profiles: f.profiles,
        requests: f.requests,
        error: null,
        loading: false,
        refreshing: false,
      });
    } catch (e) {
      // A failed refresh keeps the last good snapshot on screen — a transient
      // network blip must not blank out a parent's dashboard.
      emit({
        error: e instanceof Error ? e.message : "Could not load the family",
        loading: false,
        refreshing: false,
      });
    } finally {
      inflight = null;
      schedule();
    }
  })();
  return inflight;
}

function hidden(): boolean {
  return typeof document !== "undefined" && document.visibilityState === "hidden";
}

/** The next ambient refresh — only while someone is watching. */
function schedule() {
  if (timer) clearTimeout(timer);
  timer = null;
  if (listeners.size === 0) return;
  timer = setTimeout(() => {
    timer = null;
    if (hidden()) return; // picked up again on visibilitychange
    void load(true);
  }, pollDelay(state));
}

if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (!hidden() && listeners.size > 0 && state.devices && !inflight) void load(true);
  });
}

/** Mutations anywhere (a granted quarter-hour, a pause) announce themselves
 *  here, and every subscribed view updates from one refetch. */
export function familyChanged(): void {
  void load();
}

/** Drop everything on sign-out so the next account never sees a stale family. */
export function resetFamily(): void {
  if (timer) clearTimeout(timer);
  timer = null;
  state = EMPTY;
  inflight = null;
  for (const l of listeners) l(state);
}

export function useFamily(): FamilyState & { reload: () => Promise<void> } {
  const [local, setLocal] = useState<FamilyState>(state);

  useEffect(() => {
    listeners.add(setLocal);
    // Fetch on first subscriber, or when a previous load failed outright.
    if (!state.devices && !inflight) void load();
    else {
      setLocal(state);
      if (!timer && !inflight) schedule();
    }
    return () => {
      listeners.delete(setLocal);
      if (listeners.size === 0 && timer) {
        clearTimeout(timer);
        timer = null;
      }
    };
  }, []);

  return { ...local, reload: () => load() };
}
