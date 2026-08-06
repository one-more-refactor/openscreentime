import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import type { Me } from "../types";
import { auth, getMe, usingMock } from "../api";
import { resetFamily } from "./family";

interface SessionState {
  me: Me | null;
  loading: boolean;
  mock: boolean;
  refresh: () => Promise<void>;
  login: (email: string) => Promise<void>;
  register: (email: string, displayName: string) => Promise<void>;
  logout: () => Promise<void>;
}

const Ctx = createContext<SessionState | null>(null);

/**
 * Device-voucher autologin: `ost login` opens the console with a one-time
 * voucher in the URL **fragment**, which is never sent to a server — so the
 * credential cannot land in an access log on the way in.
 *
 * Read it, remove it from the address bar before anything else can happen
 * (history.replaceState, so it also leaves no history entry to go Back to),
 * then redeem it. A voucher is single-use and lives two minutes, so a stale
 * one simply fails and the normal sign-in screen appears.
 */
async function redeemVoucherFromUrl(): Promise<boolean> {
  const hash = window.location.hash;
  const match = /[#&]v=([A-Za-z0-9_-]+)/.exec(hash);
  if (!match) return false;

  const voucher = match[1];
  const cleanHash = hash.replace(/[#&]v=[A-Za-z0-9_-]+/, "").replace(/^#$/, "");
  window.history.replaceState(
    null,
    "",
    window.location.pathname + window.location.search + cleanHash,
  );

  try {
    await auth.voucher(voucher);
    return true;
  } catch {
    // An expired or already-spent voucher is not an error worth shouting
    // about — it just means signing in the ordinary way.
    return false;
  }
}

export function SessionProvider({ children }: { children: ReactNode }) {
  const [me, setMe] = useState<Me | null>(null);
  const [loading, setLoading] = useState(true);
  const [mock, setMock] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const value = await getMe();
      setMe(value);
      setMock(usingMock);
    } catch {
      setMe(null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // The voucher has to be redeemed BEFORE the first /api/me, or the console
    // flashes the login screen on a machine that was entitled to skip it.
    void (async () => {
      await redeemVoucherFromUrl();
      await refresh();
    })();
  }, [refresh]);

  const login = useCallback(
    async (email: string) => {
      await auth.login(email);
      await refresh();
    },
    [refresh],
  );

  const register = useCallback(
    async (email: string, displayName: string) => {
      await auth.register(email, displayName);
      await refresh();
    },
    [refresh],
  );

  const logout = useCallback(async () => {
    try {
      await auth.logout();
    } catch {
      /* ignore transport errors on logout */
    }
    setMe(null);
    // Drop the cached family too: the next person to sign in on this machine
    // must not see the previous account's children on first paint.
    resetFamily();
  }, []);

  const value = useMemo<SessionState>(
    () => ({ me, loading, mock, refresh, login, register, logout }),
    [me, loading, mock, refresh, login, register, logout],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useSession(): SessionState {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useSession must be used within <SessionProvider>");
  return ctx;
}
