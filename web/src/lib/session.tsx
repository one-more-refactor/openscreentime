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
    void refresh();
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
