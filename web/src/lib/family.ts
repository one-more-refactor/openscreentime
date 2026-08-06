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
// ============================================================================
import { useEffect, useState } from "react";
import * as api from "../api";
import type { Device, EarnRequest, FamilyChild, Profile } from "../types";

export type { FamilyChild } from "../types";

export function minutesLeft(c: FamilyChild): number | null {
  if (c.limit_minutes === null) return null;
  return Math.max(0, c.limit_minutes + c.earned_minutes - c.used_minutes);
}

/** Total minutes available today: the limit plus anything earned on top. */
export function minutesTotal(c: FamilyChild): number | null {
  if (c.limit_minutes === null) return null;
  return c.limit_minutes + c.earned_minutes;
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

function emit(next: Partial<FamilyState>) {
  state = { ...state, ...next };
  for (const l of listeners) l(state);
}

async function load(): Promise<void> {
  // Coalesce: the rail and the page mount in the same tick.
  if (inflight) return inflight;
  emit(state.devices ? { refreshing: true } : { loading: true });
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
    }
  })();
  return inflight;
}

/** Mutations anywhere (a granted quarter-hour, a pause) announce themselves
 *  here, and every subscribed view updates from one refetch. */
export function familyChanged(): void {
  void load();
}

/** Drop everything on sign-out so the next account never sees a stale family. */
export function resetFamily(): void {
  state = EMPTY;
  inflight = null;
  for (const l of listeners) l(state);
}

/**
 * Optimistically patch a child in place, before the server round-trip lands.
 * Used by the pause control so the UI reacts on the tap rather than 300ms
 * later — the refetch that follows is what makes it true.
 */
export function patchChild(key: string, patch: Partial<FamilyChild>): void {
  emit({
    children: state.children.map((c) => (c.key === key ? { ...c, ...patch } : c)),
  });
}

export function useFamily(): FamilyState & { reload: () => Promise<void> } {
  const [local, setLocal] = useState<FamilyState>(state);

  useEffect(() => {
    listeners.add(setLocal);
    // Fetch on first subscriber, or when a previous load failed outright.
    if (!state.devices && !inflight) void load();
    else setLocal(state);
    return () => {
      listeners.delete(setLocal);
    };
  }, []);

  return { ...local, reload: load };
}
