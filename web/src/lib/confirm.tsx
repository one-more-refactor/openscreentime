// ============================================================================
// Confirm — the one dialog that asks you to prove it's you (docs/AUTH.md).
//
// Signing in is the proof: pausing, granting time, changing rules just work.
// Only the keys — a computer's unlock code and recovery codes, your passkeys,
// pairing tokens — ask again. The server answers those with 428
// `step_up_required` while the session's confirm window is shut; `guard()`
// turns that into "confirm, then do it" instead of a dead end.
//
// Two ways to confirm, the same two you sign in with: your passkey, or a code
// shown on your own computer. An account with neither (SSO only) confirms by
// signing in again — the dialog always offers a way through.
// ============================================================================
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
import {
  ApiError,
  auth,
  confirmWithPasskey,
  getConfirmStatus,
  startConfirmCode,
  verifyConfirmCode,
} from "../api";
import { STEP_UP_REQUIRED } from "../types";
import type { ConfirmStatus } from "../types";
import { Modal } from "../components/Modal";
import { Button } from "../components/Button";
import { CodeRing } from "../components/CodeRing";
import { PasskeyButton } from "../components/PasskeyButton";
import { sentence } from "./format";

/** Thrown when the user dismisses the dialog — callers no-op on it. */
export class StepUpCancelled extends Error {
  constructor() {
    super("Confirm cancelled");
    this.name = "StepUpCancelled";
  }
}

export interface ConfirmApi {
  /** The confirm window is open right now. */
  armed: boolean;
  /** Open the window (asks unless it is already open). */
  enter: () => Promise<void>;
  /** Resolve once the window is open, asking if needed. */
  requireConfirm: () => Promise<void>;
  /** Run a call; if the server asks for proof, ask the person once and retry. */
  guard: <T>(fn: () => Promise<T>) => Promise<T>;
}

const Ctx = createContext<ConfirmApi | null>(null);

function live(until: string | null): boolean {
  return !!until && new Date(until).getTime() > Date.now();
}

export function ConfirmProvider({ children }: { children: ReactNode }) {
  // The window, mirrored for rendering; the ref is what async code reads so a
  // guard() that started before a re-render still sees the current truth.
  const [armedUntil, setArmedUntil] = useState<string | null>(null);
  const untilRef = useRef<string | null>(null);
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<ConfirmStatus | null>(null);
  // Everyone waiting on the dialog: one dialog answers them all, and
  // cancelling it tells them all.
  const waiters = useRef<{ resolve: () => void; reject: (e: Error) => void }[]>([]);

  const setWindow = useCallback((until: string | null) => {
    untilRef.current = until;
    setArmedUntil(until);
  }, []);

  // A fresh sign-in opens the window; a reload asks the server whether it
  // still is, so the Security room doesn't show shut while reads work.
  useEffect(() => {
    let alive = true;
    getConfirmStatus()
      .then((s) => {
        if (alive) setWindow(live(s.armed_until) ? s.armed_until : null);
      })
      .catch(() => {
        /* no session yet: stay shut */
      });
    return () => {
      alive = false;
    };
  }, [setWindow]);

  // Shut the moment it lapses, without waiting for a failed call.
  useEffect(() => {
    if (!armedUntil) return;
    const ms = new Date(armedUntil).getTime() - Date.now();
    if (ms <= 0) {
      setWindow(null);
      return;
    }
    const t = setTimeout(() => setWindow(null), ms);
    return () => clearTimeout(t);
  }, [armedUntil, setWindow]);

  const requireConfirm = useCallback(async () => {
    if (live(untilRef.current)) return;
    const first = waiters.current.length === 0;
    const wait = new Promise<void>((resolve, reject) => {
      waiters.current.push({ resolve, reject });
    });
    if (first) {
      // Which ways this account has right now, so the dialog offers those.
      try {
        const s = await getConfirmStatus();
        if (live(s.armed_until)) {
          setWindow(s.armed_until);
          const all = waiters.current;
          waiters.current = [];
          all.forEach((w) => w.resolve());
          return wait;
        }
        setStatus(s);
      } catch {
        setStatus({ armed_until: null, passkey: false, computer: false });
      }
      setOpen(true);
    }
    await wait;
  }, [setWindow]);

  // Optimistic: run the call; only if the server wants proof does anyone get
  // asked. Inside a live window the dialog never appears at all.
  const guard = useCallback(
    async <T,>(fn: () => Promise<T>): Promise<T> => {
      try {
        return await fn();
      } catch (e) {
        if (e instanceof ApiError && e.code === STEP_UP_REQUIRED) {
          setWindow(null);
          await requireConfirm();
          return await fn();
        }
        throw e;
      }
    },
    [requireConfirm, setWindow],
  );

  const enter = useCallback(async () => {
    try {
      await requireConfirm();
    } catch (e) {
      if (!(e instanceof StepUpCancelled)) throw e;
    }
  }, [requireConfirm]);

  const onConfirmed = useCallback(
    (until: string) => {
      setWindow(until);
      setOpen(false);
      const all = waiters.current;
      waiters.current = [];
      all.forEach((w) => w.resolve());
    },
    [setWindow],
  );

  const onCancel = useCallback(() => {
    setOpen(false);
    const all = waiters.current;
    waiters.current = [];
    all.forEach((w) => w.reject(new StepUpCancelled()));
  }, []);

  const armed = armedUntil !== null;
  const api = useMemo<ConfirmApi>(
    () => ({ armed, enter, requireConfirm, guard }),
    [armed, enter, requireConfirm, guard],
  );

  return (
    <Ctx.Provider value={api}>
      {children}
      <ConfirmModal open={open} status={status} onConfirmed={onConfirmed} onCancel={onCancel} />
    </Ctx.Provider>
  );
}

export function useConfirm(): ConfirmApi {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useConfirm must be used within a ConfirmProvider");
  return ctx;
}

// ---- The dialog ------------------------------------------------------------

interface ModalProps {
  open: boolean;
  status: ConfirmStatus | null;
  onConfirmed: (until: string) => void;
  onCancel: () => void;
}

function ConfirmModal({ open, status, onConfirmed, onCancel }: ModalProps) {
  const [requestId, setRequestId] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setRequestId(null);
    setCode("");
    setError(null);
    setBusy(false);
  }, [open]);

  const passkey = status?.passkey ?? false;
  const computer = status?.computer ?? false;

  async function withPasskey() {
    setError(null);
    try {
      onConfirmed((await confirmWithPasskey()).armed_until);
    } catch (e) {
      if (e instanceof Error && (e.name === "NotAllowedError" || e.name === "AbortError")) return;
      setError(e instanceof Error ? sentence(e.message) : "That passkey didn't work.");
    }
  }

  async function sendCode() {
    setBusy(true);
    setError(null);
    try {
      setRequestId((await startConfirmCode()).request_id);
      setCode("");
    } catch (e) {
      setError(e instanceof Error ? sentence(e.message) : "Couldn't reach your computer.");
    } finally {
      setBusy(false);
    }
  }

  async function verify(full: string) {
    if (!requestId) return;
    setBusy(true);
    setError(null);
    try {
      onConfirmed((await verifyConfirmCode(requestId, full)).armed_until);
    } catch (e) {
      setCode("");
      if (e instanceof ApiError && e.code === "code_expired") {
        setRequestId(null);
        setError("That code ran out. Send a new one.");
      } else {
        setError("That code didn't match. Try again.");
      }
    } finally {
      setBusy(false);
    }
  }

  // A fresh sign-in opens the window. A full page load, so nothing of this
  // session lingers in memory.
  async function signInAgain() {
    try {
      await auth.logout();
    } finally {
      window.location.assign("/login");
    }
  }

  return (
    <Modal
      open={open}
      onClose={onCancel}
      title="Confirm it's you"
      footer={
        <Button variant="ghost" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      }
    >
      <div className="flex flex-col gap-4">
        {requestId ? (
          <div className="cr-wrap">
            <p className="text-sm" style={{ color: "var(--fg-dim)" }}>
              Type the code from the OpenScreenTime window on your computer.
            </p>
            <CodeRing
              value={code}
              disabled={busy}
              error={!!error}
              aria-label="The code from your computer"
              onChange={(v) => {
                setCode(v);
                if (error) setError(null);
              }}
              onComplete={(full) => void verify(full)}
            />
            <p className="cr-note" data-error={!!error} role={error ? "alert" : undefined}>
              {busy ? "Checking…" : (error ?? "It works for 5 minutes.")}
            </p>
          </div>
        ) : (
          <>
            <p className="text-sm" style={{ color: "var(--fg-dim)" }}>
              You're about to see the keys to your household. It stays confirmed for 15 minutes.
            </p>
            {passkey && <PasskeyButton label="Use your passkey" onActivate={withPasskey} />}
            {computer && (
              <Button
                variant={passkey ? "ghost" : "primary"}
                className="w-full"
                disabled={busy}
                onClick={() => void sendCode()}
              >
                {busy ? "Sending…" : "Get a code on your computer"}
              </Button>
            )}
            {!passkey && !computer && (
              <>
                <p className="text-sm" role="note">
                  Sign in again to confirm — a fresh sign-in counts for 15 minutes.
                </p>
                <Button className="w-full" onClick={() => void signInAgain()}>
                  Sign in again
                </Button>
              </>
            )}
            {error && (
              <p className="text-sm" role="alert" style={{ color: "var(--accent)" }}>
                {error}
              </p>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
