// The store's whole reason to exist is that N mounting views cause ONE fetch.
// These tests hold that line, plus the two things a dashboard must never do:
// blank itself on a failed refresh, and leak one account's family into the next.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { renderHook, waitFor, act } from "@testing-library/react";

// Registers the shared module mocks; must be imported before the store.
import { apiCalls, apiImpl, resetApiMock } from "../test/mockApi";
import { useFamily, familyChanged, resetFamily, minutesLeft, minutesTotal } from "./family";
import type { FamilyChild, FamilyResponse } from "../types";

function child(over: Partial<FamilyChild> = {}): FamilyChild {
  return {
    key: "mia",
    name: "Mia",
    used_minutes: 30,
    earned_minutes: 0,
    limit_minutes: 60,
    profile_id: "p-kids",
    profile_name: "Kids",
    devices: [],
    pending_requests: 0,
    ...over,
  };
}

function response(over: Partial<FamilyResponse> = {}): FamilyResponse {
  return {
    children: [child()],
    devices: [],
    profiles: [],
    requests: [],
    server_time: new Date().toISOString(),
    ...over,
  };
}

afterEach(() => {
  resetFamily();
  resetApiMock();
  apiImpl.getFamily = () => Promise.resolve(response());
});

beforeEach(() => {
  apiImpl.getFamily = () => Promise.resolve(response());
});

describe("minutes maths", () => {
  test("a null limit means no limit, not zero left", () => {
    const c = child({ limit_minutes: null, used_minutes: 500 });
    expect(minutesLeft(c)).toBeNull();
    expect(minutesTotal(c)).toBeNull();
  });

  test("earned time extends the day and never goes negative", () => {
    expect(minutesLeft(child({ used_minutes: 30, earned_minutes: 15 }))).toBe(45);
    expect(minutesTotal(child({ earned_minutes: 15 }))).toBe(75);
    // Over budget clamps at zero rather than showing "-20 min left".
    expect(minutesLeft(child({ used_minutes: 80 }))).toBe(0);
  });
});

describe("the shared store", () => {
  test("two mounted views cause exactly one fetch", async () => {
    apiImpl.getFamily = () => Promise.resolve(response());

    const a = renderHook(() => useFamily());
    const b = renderHook(() => useFamily());

    await waitFor(() => expect(a.result.current.loading).toBe(false));
    await waitFor(() => expect(b.result.current.loading).toBe(false));

    // This is the regression the endpoint was built to kill: the rail and the
    // page each used to run the whole fan-out independently.
    expect(apiCalls.family).toBe(1);
    expect(a.result.current.children[0].name).toBe("Mia");
    expect(b.result.current.children[0].name).toBe("Mia");
  });

  test("every subscriber sees a mutation's refetch", async () => {
    let nth = 0;
    apiImpl.getFamily = () => {
      nth += 1;
      return Promise.resolve(
        response({ children: [child({ used_minutes: nth === 1 ? 30 : 45 })] }),
      );
    };

    const rail = renderHook(() => useFamily());
    const page = renderHook(() => useFamily());
    await waitFor(() => expect(rail.result.current.loading).toBe(false));

    await act(async () => {
      familyChanged();
    });

    await waitFor(() => expect(rail.result.current.children[0].used_minutes).toBe(45));
    expect(page.result.current.children[0].used_minutes).toBe(45);
  });

  test("a failed refresh keeps the last good data on screen", async () => {
    let first = true;
    apiImpl.getFamily = () => {
      if (first) {
        first = false;
        return Promise.resolve(response());
      }
      return Promise.reject(new Error("network is down"));
    };

    const { result } = renderHook(() => useFamily());
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.children).toHaveLength(1);

    await act(async () => {
      familyChanged();
    });

    await waitFor(() => expect(result.current.error).toBe("network is down"));
    // The parent's dashboard must not go blank because one poll blipped.
    expect(result.current.children).toHaveLength(1);
    expect(result.current.children[0].name).toBe("Mia");
  });

  test("signing out drops the family so the next account starts clean", async () => {
    const { result } = renderHook(() => useFamily());
    await waitFor(() => expect(result.current.children).toHaveLength(1));

    act(() => {
      resetFamily();
    });

    expect(result.current.children).toHaveLength(0);
    expect(result.current.devices).toBeNull();
  });
});
