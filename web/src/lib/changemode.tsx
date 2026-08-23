// ============================================================================
// Change mode — the console's explicit edit state.
//
// Reading is free. Changing anything needs a second factor — but once, not per
// change: a verified code turns change mode ON for fifteen minutes (the
// server's step-up grant, docs/AUTH.md), the whole console visibly unlocks,
// and it locks again when the parent says so, when the time runs out, or when
// a reloaded console finds the grant gone.
//
// The server is still the authority: a mutation without a live grant comes
// back 428 `step_up_required`, and `guard()` turns that into "turn on change
// mode, then do it" instead of a dead end. Entering and leaving both play a
// short full-screen veil (ChangeModeVeil) so the state change is felt, not
// inferred from a chip somewhere.
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
  extendChangeMode,
  getChangeMode,
  getTwoFactorStatus,
  lockChangeMode,
  startEmailStepUp,
  verifyStepUp,
} from "../api";
import { STEP_UP_REQUIRED } from "../types";
import type { SecondFactorMethod, StepUpGrant, TwoFactorStatus } from "../types";
import { Modal } from "../components/Modal";
import { Button } from "../components/Button";
import { CodeRing } from "../components/CodeRing";
import { ChangeModeVeil, type VeilKind } from "../components/ChangeModeVeil";

/** Thrown when the user dismisses the dialog — callers no-op on it. */
export class StepUpCancelled extends Error {
  constructor() {
    super("Step-up cancelled");
    this.name = "StepUpCancelled";
  }
}

export interface ChangeModeApi {
  /** Change mode is on right now. */
  armed: boolean;
  /** When it lapses (ISO), or null while locked. */
  armedUntil: string | null;
  /** The one allowed extension has been used. */
  extended: boolean;
  /** Turn change mode on (opens the code dialog unless already on). */
  enter: () => Promise<void>;
  /** Lock it down again, now. */
  lock: () => Promise<void>;
  /** Another fifteen minutes from now — once. */
  extend: () => Promise<void>;
  /** Resolve when change mode is on, prompting for a code if not. */
  requireStepUp: () => Promise<void>;
  /** Run a mutation behind change mode, retrying once if the server demands it. */
  guard: <T>(fn: () => Promise<T>) => Promise<T>;
}

const Ctx = createContext<ChangeModeApi | null>(null);

function live(until: string | null): boolean {
  return !!until && new Date(until).getTime() > Date.now();
}

/** A promise whose settling is in someone else's hands. */
function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve: () => void = () => {};
  const promise = new Promise<void>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

export function ChangeModeProvider({ children }: { children: ReactNode }) {
  // The grant, mirrored for rendering; the ref is what async code reads so a
  // guard() that started before a re-render still sees the current truth.
  const [armedUntil, setArmedUntil] = useState<string | null>(null);
  const [extended, setExtended] = useState(false);
  const untilRef = useRef<string | null>(null);
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<TwoFactorStatus | null>(null);
  const [veil, setVeil] = useState<VeilKind | null>(null);
  // Everyone waiting on the dialog. Two controls can ask at once (a panel
  // that loads a code and its recovery count together); one dialog answers
  // them all, and cancelling it tells them all.
  const waiters = useRef<{ resolve: () => void; reject: (e: Error) => void }[]>([]);
  // The first question to the server — "is change mode still on?" — settles
  // before any guard() decides to open the dialog, so a click on a reloaded
  // console (or a child that loads on mount — children's effects run before
  // ours) does not ask for a code the session already has.
  const ready = useRef(deferred());

  const setGrant = useCallback((until: string | null, ext: boolean) => {
    untilRef.current = until;
    setArmedUntil(until);
    setExtended(ext);
  }, []);

  // A reloaded console asks the server whether change mode is still on —
  // otherwise the controls would show locked while mutations quietly succeed.
  useEffect(() => {
    let alive = true;
    const gate = ready.current;
    getChangeMode()
      .then((s) => {
        if (!alive) return;
        setGrant(live(s.armed_until) ? s.armed_until : null, s.extended);
      })
      .catch(() => {
        /* no session yet, or an older server: stay locked */
      })
      .finally(() => gate.resolve());
    return () => {
      alive = false;
    };
  }, [setGrant]);

  // Lock the moment the grant lapses, without waiting for a failed call —
  // and say so with the same veil a manual lock shows.
  useEffect(() => {
    if (!armedUntil) return;
    const ms = new Date(armedUntil).getTime() - Date.now();
    if (ms <= 0) {
      setGrant(null, false);
      return;
    }
    const t = setTimeout(() => {
      setGrant(null, false);
      setVeil("lock");
    }, ms);
    return () => clearTimeout(t);
  }, [armedUntil, setGrant]);

  const requireStepUp = useCallback(async () => {
    await ready.current.promise;
    if (live(untilRef.current)) return;
    const first = waiters.current.length === 0;
    const wait = new Promise<void>((resolve, reject) => {
      waiters.current.push({ resolve, reject });
    });
    if (first) {
      // Load which factors this account has, so the dialog shows the right paths.
      try {
        setStatus(await getTwoFactorStatus());
      } catch {
        setStatus({ totp_enrolled: false, email_available: true });
      }
      setOpen(true);
    }
    await wait;
  }, []);

  const guard = useCallback(
    async <T,>(fn: () => Promise<T>): Promise<T> => {
      await requireStepUp();
      try {
        return await fn();
      } catch (e) {
        // The grant may have lapsed between check and call — once more.
        if (e instanceof ApiError && e.code === STEP_UP_REQUIRED) {
          setGrant(null, false);
          await requireStepUp();
          return await fn();
        }
        throw e;
      }
    },
    [requireStepUp, setGrant],
  );

  const enter = useCallback(async () => {
    try {
      await requireStepUp();
    } catch (e) {
      if (!(e instanceof StepUpCancelled)) throw e;
    }
  }, [requireStepUp]);

  const lock = useCallback(async () => {
    try {
      await lockChangeMode();
    } catch {
      // Even if the server could not be told, the console locks: the next
      // mutation simply asks again.
    }
    setGrant(null, false);
    setVeil("lock");
  }, [setGrant]);

  const extend = useCallback(async () => {
    const s = await extendChangeMode();
    setGrant(s.armed_until, s.extended);
  }, [setGrant]);

  const onVerified = useCallback(
    (grant: StepUpGrant) => {
      setGrant(grant.expires_at, grant.extended ?? false);
      setOpen(false);
      // The veil plays over an app that is already unlocked — the mutation
      // that asked for this proceeds underneath it.
      setVeil("enter");
      const all = waiters.current;
      waiters.current = [];
      all.forEach((w) => w.resolve());
    },
    [setGrant],
  );

  const onCancel = useCallback(() => {
    setOpen(false);
    const all = waiters.current;
    waiters.current = [];
    all.forEach((w) => w.reject(new StepUpCancelled()));
  }, []);

  const armed = armedUntil !== null;
  const api = useMemo<ChangeModeApi>(
    () => ({ armed, armedUntil, extended, enter, lock, extend, requireStepUp, guard }),
    [armed, armedUntil, extended, enter, lock, extend, requireStepUp, guard],
  );

  return (
    <Ctx.Provider value={api}>
      {/* display:contents — a pure CSS scope: off greys the code-gated
          controls, on releases them all at once. */}
      <div data-changemode={armed ? "on" : "off"} style={{ display: "contents" }}>
        {children}
      </div>
      <StepUpModal open={open} status={status} onVerified={onVerified} onCancel={onCancel} />
      {veil && <ChangeModeVeil kind={veil} onDone={() => setVeil(null)} />}
    </Ctx.Provider>
  );
}

export function useChangeMode(): ChangeModeApi {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useChangeMode must be used within a ChangeModeProvider");
  return ctx;
}

// ---- The dialog ------------------------------------------------------------

interface ModalProps {
  open: boolean;
  status: TwoFactorStatus | null;
  onVerified: (grant: StepUpGrant) => void;
  onCancel: () => void;
}

function StepUpModal({ open, status, onVerified, onCancel }: ModalProps) {
  const totp = status?.totp_enrolled ?? false;
  const [method, setMethod] = useState<SecondFactorMethod>("totp");
  const [code, setCode] = useState("");
  const [sent, setSent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // When the dialog opens, reset and default to the strongest available method.
  useEffect(() => {
    if (!open) return;
    setMethod(totp ? "totp" : "email");
    setCode("");
    setSent(false);
    setError(null);
    setBusy(false);
  }, [open, totp]);

  async function sendEmail() {
    setBusy(true);
    setError(null);
    try {
      await startEmailStepUp();
      setSent(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not send a code.");
    } finally {
      setBusy(false);
    }
  }

  async function verify(full: string) {
    setBusy(true);
    setError(null);
    try {
      onVerified(await verifyStepUp(method, full));
    } catch (e) {
      // The ring flashes red and empties; the message says why.
      setError(e instanceof Error ? e.message : "That code didn't match.");
      setCode("");
    } finally {
      setBusy(false);
    }
  }

  const emailMethod = (status?.email_available ?? false) && (
    <MethodTab
      active={method === "email"}
      label="Email a code"
      onClick={() => {
        setMethod("email");
        setError(null);
      }}
    />
  );

  return (
    <Modal
      open={open}
      onClose={onCancel}
      title="TURN ON CHANGE MODE"
      footer={
        <Button variant="ghost" onClick={onCancel} disabled={busy}>
          CANCEL
        </Button>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="text-sm" style={{ color: "var(--fg-dim)" }}>
          Enter a code once. Change mode stays on for 15 minutes — every change
          goes through without asking again — and you can lock it any time.
        </p>

        {(totp || (status?.email_available ?? false)) && (
          <div className="seg">
            {totp && (
              <MethodTab
                active={method === "totp"}
                label="Authenticator"
                onClick={() => {
                  setMethod("totp");
                  setError(null);
                }}
              />
            )}
            {emailMethod}
          </div>
        )}

        {method === "email" && !sent ? (
          <Button variant="ghost" onClick={() => void sendEmail()} disabled={busy}>
            {busy ? "SENDING…" : "SEND CODE TO MY EMAIL"}
          </Button>
        ) : (
          <div className="cr-wrap">
            <CodeRing
              value={code}
              disabled={busy}
              error={!!error}
              aria-label={method === "totp" ? "Code from your authenticator" : "Code from your email"}
              onChange={(v) => {
                setCode(v);
                if (error) setError(null);
              }}
              onComplete={(full) => void verify(full)}
            />
            <p className="cr-note" data-error={!!error} role={error ? "alert" : undefined}>
              {busy
                ? "Checking…"
                : (error ??
                  (method === "totp"
                    ? "The 6 digits from your authenticator"
                    : "The 6 digits we emailed you"))}
            </p>
          </div>
        )}
      </div>
    </Modal>
  );
}

function MethodTab({
  active,
  label,
  onClick,
}: {
  active: boolean;
  label: string;
  onClick: () => void;
}) {
  return (
    <button type="button" onClick={onClick} className="focusable seg-btn" data-on={active}>
      {label}
    </button>
  );
}
