// ============================================================================
// The family, assembled. Children live on devices (a person = the same OS
// username across machines), so anything that wants to show people has to
// gather devices → users → profiles. That logic used to live inside the home
// page; the sidebar needs it too, so it lives here once.
// A dedicated /api/family endpoint would replace most of this — noted, not faked.
// ============================================================================
import { useCallback, useEffect, useState } from "react";
import * as api from "../api";
import type { Device, DeviceUser, EarnRequest, Profile } from "../types";

export interface FamilyChild {
  key: string;
  name: string;
  usedMinutes: number;
  earnedMinutes: number;
  /** null = no limit configured (disabled or zero — never "0 left of 0") */
  limitMinutes: number | null;
  profileId: string | null;
  profileName: string | null;
  devices: { id: string; name: string; status: Device["status"] }[];
  /** earn requests waiting on a parent */
  pendingRequests: number;
}

export function minutesLeft(c: FamilyChild): number | null {
  if (c.limitMinutes === null) return null;
  return Math.max(0, c.limitMinutes + c.earnedMinutes - c.usedMinutes);
}

interface FamilyState {
  devices: Device[] | null;
  children: FamilyChild[];
  profiles: Profile[];
  requests: EarnRequest[];
  error: string | null;
}

function assemble(
  devices: Device[],
  usersByDevice: Record<string, DeviceUser[]>,
  profiles: Profile[],
  requests: EarnRequest[],
): FamilyChild[] {
  const byKey = new Map<string, FamilyChild>();
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
        const profile = profiles.find((p) => p.id === u.profile_id) ?? null;
        const st = profile?.policy.screen_time;
        byKey.set(key, {
          key,
          name,
          usedMinutes: u.used_minutes_today ?? 0,
          earnedMinutes: u.earned_minutes_today ?? 0,
          // A disabled or zero limit is "no limit", never "0 left of 0".
          limitMinutes:
            st?.enabled && (st.daily_limit_minutes ?? 0) > 0
              ? st.daily_limit_minutes
              : null,
          profileId: profile?.id ?? null,
          profileName: u.profile_name ?? profile?.name ?? null,
          devices: [{ id: d.id, name: d.name, status: d.status }],
          pendingRequests: requests.filter((r) => r.os_username === key).length,
        });
      }
    }
  }
  return [...byKey.values()].sort((a, b) => a.name.localeCompare(b.name));
}

// Mutations anywhere (a granted quarter-hour, a paused device) announce
// themselves here so every mounted family view — the rail included — refreshes.
const bus = new EventTarget();
export function familyChanged() {
  bus.dispatchEvent(new Event("change"));
}

export function useFamily(): FamilyState & { reload: () => Promise<void> } {
  const [state, setState] = useState<FamilyState>({
    devices: null,
    children: [],
    profiles: [],
    requests: [],
    error: null,
  });

  const reload = useCallback(async () => {
    try {
      const [devices, profiles] = await Promise.all([
        api.listDevices(),
        api.listProfiles(),
      ]);
      const entries = await Promise.all(
        devices.map(async (d) => {
          try {
            return [d.id, await api.listDeviceUsers(d.id)] as const;
          } catch {
            // One unreachable device must not blank the whole family.
            return [d.id, [] as DeviceUser[]] as const;
          }
        }),
      );
      const usersByDevice = Object.fromEntries(entries);
      let requests: EarnRequest[] = [];
      try {
        requests = await api.listEarnRequests("pending");
      } catch {
        requests = [];
      }
      setState({
        devices: [...devices],
        children: assemble(devices, usersByDevice, profiles, requests),
        profiles,
        requests,
        error: null,
      });
    } catch (e) {
      setState((s) => ({
        ...s,
        error: e instanceof Error ? e.message : "Could not load the family",
      }));
    }
  }, []);

  useEffect(() => {
    void reload();
    const onChange = () => void reload();
    bus.addEventListener("change", onChange);
    return () => bus.removeEventListener("change", onChange);
  }, [reload]);

  return { ...state, reload };
}
