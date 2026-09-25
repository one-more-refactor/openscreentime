import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useNavigate } from "react-router-dom";
import type { Me } from "../types";
import { auth, getMe, pkcePair, usingMock } from "../api";
import { resetFamily } from "./family";
import { captureFragmentTokens, takeToken } from "./fragment";

interface SessionState {
  me: Me | null;
  loading: boolean;
  mock: boolean;
  refresh: () => Promise<void>;
  /** First run: your name, then a passkey. */
  createHousehold: (name: string, setupToken?: string) => Promise<void>;
  /** Door two: a passkey, no name first. */
  signInWithPasskey: () => Promise<void>;
  /** Door one: send a code to the computer of whoever is called `name`.
   * Resolves to how many seconds the code works for. */
  sendCode: (name: string) => Promise<number>;
  /** …then type it in. Throws `wrong_code` (type it again) or
   * `code_expired` (ask for a new one). */
  enterCode: (code: string) => Promise<void>;
  logout: () => Promise<void>;
}

const Ctx = createContext<SessionState | null>(null);

export function SessionProvider({ children }: { children: ReactNode }) {
  // The first thing the app renders: one-time tokens leave the address bar
  // here, before any route below can redirect and take the fragment with it
  // (`/#setup=…` with no session is sent on to /login).
  useState(captureFragmentTokens);
  const [me, setMe] = useState<Me | null>(null);
  const [loading, setLoading] = useState(true);
  const [mock, setMock] = useState(false);
  const navigate = useNavigate();
  // The code request in flight: its id, and the PKCE verifier that only this
  // tab holds. In memory only — a code typed into any other browser is useless.
  const pending = useRef<{ id: string; verifier: string } | null>(null);
  // The one-time link's redemption, started once (below): was it a recovery?
  const redemption = useRef<Promise<boolean> | null>(null);

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
    // One-time tokens in the fragment are redeemed BEFORE the first /api/me,
    // or the console flashes the sign-in page on a visit entitled to skip it.
    // A stale or spent one simply fails, and the sign-in page appears. Once
    // per page load: a later run of this effect (React's development
    // double-run, a navigation) waits for that redemption instead of racing
    // a token-less /api/me against it.
    const first = redemption.current === null;
    if (first) {
      redemption.current = (async () => {
        const voucher = takeToken("v");
        const link = takeToken("signin");
        try {
          if (voucher) await auth.voucher(voucher);
          if (link) {
            await auth.link(link);
            return true;
          }
        } catch {
          /* sign in the ordinary way */
        }
        return false;
      })();
    }
    void redemption.current?.then(async (recovered) => {
      await refresh();
      // A recovery link's whole point: add a new passkey, now.
      if (first && recovered) navigate("/settings", { replace: true, state: { recovered: true } });
    });
  }, [refresh, navigate]);

  const createHousehold = useCallback(
    async (name: string, setupToken?: string) => {
      await auth.register(name, setupToken);
      await refresh();
    },
    [refresh],
  );

  const signInWithPasskey = useCallback(async () => {
    await auth.passkey();
    await refresh();
  }, [refresh]);

  const sendCode = useCallback(async (name: string) => {
    const { verifier, challenge } = await pkcePair();
    const started = await auth.codeStart(name, challenge);
    pending.current = { id: started.request_id, verifier };
    return started.expires_in_secs;
  }, []);

  const enterCode = useCallback(
    async (code: string) => {
      const p = pending.current;
      if (!p) throw new Error("Ask for a code first.");
      await auth.codeVerify(p.id, p.verifier, code);
      pending.current = null;
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
    () => ({
      me,
      loading,
      mock,
      refresh,
      createHousehold,
      signInWithPasskey,
      sendCode,
      enterCode,
      logout,
    }),
    [me, loading, mock, refresh, createHousehold, signInWithPasskey, sendCode, enterCode, logout],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useSession(): SessionState {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useSession must be used within <SessionProvider>");
  return ctx;
}
