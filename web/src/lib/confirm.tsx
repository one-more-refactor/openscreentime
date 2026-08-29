// ============================================================================
// Confirm — the one dialog that asks you to prove it's you.
//
// Trust lives at login now: a session born from a passkey, SSO, or the
// installed client on your own computer mutates freely — pausing, granting
// time, changing rules just work, with nothing to arm first. The server
// remains the authority; the rare route that still wants proof answers
// 428 `step_up_required`, and `guard()` turns that into "confirm, then do it"
// instead of a dead end.
//
// Two things still ask:
//   - the sensitive corner (unlock codes, recovery codes, passkeys, pairing
//     tokens) — one factor opens a fifteen-minute confirm window,
//   - a session that predates trust-at-login — one factor repairs it for good.
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
  startTelegramStepUp,
  verifyStepUp,
} from "../api";
import { STEP_UP_REQUIRED } from "../types";
import type { SecondFactorMethod, StepUpGrant, TwoFactorStatus } from "../types";
import { Modal } from "../components/Modal";
import { Button } from "../components/Button";
import { CodeRing } from "../components/CodeRing";

/** Thrown when the user dismisses the dialog — callers no-op on it. */
export class StepUpCancelled extends Error {
  constructor() {
    super("Confirm cancelled");
    this.name = "StepUpCancelled";
  }
}

export interface ConfirmApi {
  /** The sensitive-corner confirm window is open right now. */
  armed: boolean;
  /** When it lapses (ISO), or null while shut. */
  armedUntil: string | null;
  /** The one allowed extension has been used. */
  extended: boolean;
  /** Open the window (asks for a factor unless it is already open). */
  enter: () => Promise<void>;
  /** Shut it now. */
  lock: () => Promise<void>;
  /** Another fifteen minutes from now — once. */
  extend: () => Promise<void>;
  /** Resolve once the window is open, prompting for a factor if needed. */
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
  const [extended, setExtended] = useState(false);
  const untilRef = useRef<string | null>(null);
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<TwoFactorStatus | null>(null);
  // Everyone waiting on the dialog. Two calls can ask at once (a panel that
  // loads a code and its recovery count together); one dialog answers them
  // all, and cancelling it tells them all.
  const waiters = useRef<{ resolve: () => void; reject: (e: Error) => void }[]>([]);

  const setGrant = useCallback((until: string | null, ext: boolean) => {
    untilRef.current = until;
    setArmedUntil(until);
    setExtended(ext);
  }, []);

  // A reloaded console asks the server whether the window is still open, so
  // the Security room doesn't show shut while sensitive reads quietly work.
  useEffect(() => {
    let alive = true;
    getChangeMode()
      .then((s) => {
        if (!alive) return;
        setGrant(live(s.armed_until) ? s.armed_until : null, s.extended);
      })
      .catch(() => {
        /* no session yet, or an older server: stay shut */
      });
    return () => {
      alive = false;
    };
  }, [setGrant]);

  // Shut the moment it lapses, without waiting for a failed call.
  useEffect(() => {
    if (!armedUntil) return;
    const ms = new Date(armedUntil).getTime() - Date.now();
    if (ms <= 0) {
      setGrant(null, false);
      return;
    }
    const t = setTimeout(() => setGrant(null, false), ms);
    return () => clearTimeout(t);
  }, [armedUntil, setGrant]);

  const requireConfirm = useCallback(async () => {
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
        setStatus({ totp_enrolled: false });
      }
      setOpen(true);
    }
    await wait;
  }, []);

  // Optimistic: run the call; only if the server wants proof does anyone get
  // asked. On a trusted session the dialog never appears at all.
  const guard = useCallback(
    async <T,>(fn: () => Promise<T>): Promise<T> => {
      try {
        return await fn();
      } catch (e) {
        if (e instanceof ApiError && e.code === STEP_UP_REQUIRED) {
          setGrant(null, false);
          await requireConfirm();
          return await fn();
        }
        throw e;
      }
    },
    [requireConfirm, setGrant],
  );

  const enter = useCallback(async () => {
    try {
      await requireConfirm();
    } catch (e) {
      if (!(e instanceof StepUpCancelled)) throw e;
    }
  }, [requireConfirm]);

  const lock = useCallback(async () => {
    try {
      await lockChangeMode();
    } catch {
      // Even if the server could not be told, the window shuts locally: the
      // next sensitive read simply asks again.
    }
    setGrant(null, false);
  }, [setGrant]);

  const extend = useCallback(async () => {
    const s = await extendChangeMode();
    setGrant(s.armed_until, s.extended);
  }, [setGrant]);

  const onVerified = useCallback(
    (grant: StepUpGrant) => {
      setGrant(grant.expires_at, grant.extended ?? false);
      setOpen(false);
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
  const api = useMemo<ConfirmApi>(
    () => ({ armed, armedUntil, extended, enter, lock, extend, requireConfirm, guard }),
    [armed, armedUntil, extended, enter, lock, extend, requireConfirm, guard],
  );

  return (
    <Ctx.Provider value={api}>
      {children}
      <ConfirmModal open={open} status={status} onVerified={onVerified} onCancel={onCancel} />
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
  status: TwoFactorStatus | null;
  onVerified: (grant: StepUpGrant) => void;
  onCancel: () => void;
}

function ConfirmModal({ open, status, onVerified, onCancel }: ModalProps) {
  const totp = status?.totp_enrolled ?? false;
  const telegram = status?.telegram_available ?? false;
  const [method, setMethod] = useState<SecondFactorMethod>("totp");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The phone path: a tap sent, and a poll waiting for it to land.
  const [tapSent, setTapSent] = useState(false);

  // When the dialog opens, reset and default to the easiest available method:
  // one tap on the phone beats typing any code.
  useEffect(() => {
    if (!open) return;
    setMethod(telegram ? "telegram" : "totp");
    setCode("");
    setTapSent(false);
    setError(null);
    setBusy(false);
  }, [open, totp, telegram]);

  // While a tap is out, poll the server until the window opens (the bot has
  // no way to reach this tab — the tab asks). Two minutes, then give up.
  useEffect(() => {
    if (!open || !tapSent) return;
    const startedAt = Date.now();
    const t = setInterval(async () => {
      if (Date.now() - startedAt > 120_000) {
        clearInterval(t);
        setTapSent(false);
        setError("No tap arrived — try again, or use a code.");
        return;
      }
      try {
        const s = await getChangeMode();
        if (s.armed_until && new Date(s.armed_until).getTime() > Date.now()) {
          clearInterval(t);
          onVerified({ method: "telegram", expires_at: s.armed_until, extended: s.extended });
        }
      } catch {
        /* keep polling — a blip is not a verdict */
      }
    }, 2000);
    return () => clearInterval(t);
  }, [open, tapSent, onVerified]);

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
        <p className="text-sm" style={{ color: "var(--fg-dim)" }}>
          This touches the keys to your household. Enter a code once — it
          stays confirmed for 15 minutes.
        </p>

        {(totp || telegram) && (
          <div className="seg">
            {telegram && (
              <MethodTab
                active={method === "telegram"}
                label="Phone"
                onClick={() => {
                  setMethod("telegram");
                  setError(null);
                }}
              />
            )}
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
          </div>
        )}

        {method === "telegram" ? (
          tapSent ? (
            <p className="text-sm" role="status" style={{ color: "var(--fg-dim)" }}>
              Sent. Tap <strong style={{ color: "var(--fg)" }}>✅ It's me</strong> on your
              phone — this dialog closes by itself.
            </p>
          ) : (
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() =>
                void (async () => {
                  setBusy(true);
                  setError(null);
                  try {
                    await startTelegramStepUp();
                    setTapSent(true);
                  } catch (e) {
                    setError(e instanceof Error ? e.message : "Could not reach your phone.");
                  } finally {
                    setBusy(false);
                  }
                })()
              }
            >
              {busy ? "Sending…" : "Send a tap to my phone"}
            </Button>
          )
        ) : (
          <div className="cr-wrap">
            <CodeRing
              value={code}
              disabled={busy}
              error={!!error}
              aria-label="Code from your authenticator"
              onChange={(v) => {
                setCode(v);
                if (error) setError(null);
              }}
              onComplete={(full) => void verify(full)}
            />
            <p className="cr-note" data-error={!!error} role={error ? "alert" : undefined}>
              {busy ? "Checking…" : (error ?? "The 6 digits from your authenticator")}
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
