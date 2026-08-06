// ============================================================================
// Step-up 2FA — the "reading is free, every change needs a second factor"
// invariant, on the client side. A mutation calls requireStepUp() (or wraps
// itself in guard()); if a grant is live it resolves instantly, otherwise the
// StepUpModal opens and the promise resolves once a factor is verified. The
// server is still the authority (docs/AUTH.md) — this is the UX that makes the
// server's 428 pleasant instead of a dead end.
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
  getTwoFactorStatus,
  startEmailStepUp,
  verifyStepUp,
  ApiError,
} from "../api";
import { STEP_UP_REQUIRED } from "../types";
import type {
  SecondFactorMethod,
  StepUpGrant,
  TwoFactorStatus,
} from "../types";
import { Modal, Button } from "../components";
import { CodeRing } from "../components/CodeRing";

/** Thrown when the user dismisses the step-up modal — callers no-op on it. */
export class StepUpCancelled extends Error {
  constructor() {
    super("Step-up cancelled");
    this.name = "StepUpCancelled";
  }
}

interface StepUpApi {
  /** Resolve when a step-up grant is live, prompting for a factor if not. */
  requireStepUp: () => Promise<void>;
  /** Run a mutation behind step-up, retrying once if the server demands it. */
  guard: <T>(fn: () => Promise<T>) => Promise<T>;
  /** A grant is live right now — the UI un-greys while this is true. */
  armed: boolean;
  /** When the live grant lapses (ISO), or null when locked. */
  armedUntil: string | null;
}

const Ctx = createContext<StepUpApi | null>(null);

function grantLive(g: StepUpGrant | null): boolean {
  return !!g && new Date(g.expires_at).getTime() > Date.now();
}

export function StepUpProvider({ children }: { children: ReactNode }) {
  const grantRef = useRef<StepUpGrant | null>(null);
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<TwoFactorStatus | null>(null);
  // Mirrors grantRef for rendering: controls grey out while locked, and the
  // whole app visibly relaxes for the grant window after a code clears.
  const [armedUntil, setArmedUntil] = useState<string | null>(null);
  // The pending promise's settlers, held while the modal is up.
  const pending = useRef<{ resolve: () => void; reject: (e: Error) => void } | null>(
    null,
  );

  // Re-lock the moment the grant lapses, without waiting for a failed call.
  useEffect(() => {
    if (!armedUntil) return;
    const ms = new Date(armedUntil).getTime() - Date.now();
    if (ms <= 0) {
      setArmedUntil(null);
      return;
    }
    const t = setTimeout(() => {
      grantRef.current = null;
      setArmedUntil(null);
    }, ms);
    return () => clearTimeout(t);
  }, [armedUntil]);

  const requireStepUp = useCallback(async () => {
    if (grantLive(grantRef.current)) return;
    // Load which factors this account has, so the modal shows the right paths.
    try {
      setStatus(await getTwoFactorStatus());
    } catch {
      setStatus({ totp_enrolled: false, email_available: true });
    }
    await new Promise<void>((resolve, reject) => {
      pending.current = { resolve, reject };
      setOpen(true);
    });
  }, []);

  const guard = useCallback(
    async <T,>(fn: () => Promise<T>): Promise<T> => {
      await requireStepUp();
      try {
        return await fn();
      } catch (e) {
        // The grant may have lapsed between check and call — step up once more.
        if (e instanceof ApiError && e.code === STEP_UP_REQUIRED) {
          grantRef.current = null;
          setArmedUntil(null);
          await requireStepUp();
          return await fn();
        }
        throw e;
      }
    },
    [requireStepUp],
  );

  const onVerified = useCallback((grant: StepUpGrant) => {
    grantRef.current = grant;
    setArmedUntil(grant.expires_at);
    setOpen(false);
    pending.current?.resolve();
    pending.current = null;
  }, []);

  const onCancel = useCallback(() => {
    setOpen(false);
    pending.current?.reject(new StepUpCancelled());
    pending.current = null;
  }, []);

  const armed = armedUntil !== null;
  const api = useMemo(
    () => ({ requireStepUp, guard, armed, armedUntil }),
    [requireStepUp, guard, armed, armedUntil],
  );

  return (
    <Ctx.Provider value={api}>
      {/* display:contents — a pure CSS scope; locked greys the code-gated
          controls, armed releases them. */}
      <div data-stepup={armed ? "armed" : "locked"} style={{ display: "contents" }}>
        {children}
      </div>
      <StepUpModal
        open={open}
        status={status}
        onVerified={onVerified}
        onCancel={onCancel}
      />
    </Ctx.Provider>
  );
}

export function useStepUp(): StepUpApi {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useStepUp must be used within a StepUpProvider");
  return ctx;
}

// ---- The modal -------------------------------------------------------------

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

  // When the modal opens, reset and default to the strongest available method.
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
      title="CONFIRM IT'S YOU"
      footer={
        <Button variant="ghost" onClick={onCancel} disabled={busy}>
          CANCEL
        </Button>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="text-sm" style={{ color: "var(--fg-dim)" }}>
          Making a change needs a second factor. Reading never does.
        </p>

        {(totp || (status?.email_available ?? false)) && (
          <div className="flex gap-1 border rounded p-1 w-fit" style={{ borderColor: "var(--line)" }}>
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
    <button
      onClick={onClick}
      className="focusable px-3 py-1.5 rounded text-[0.625rem] font-mono uppercase tracking-label transition-colors"
      style={active ? { background: "var(--fg)", color: "var(--bg)" } : { color: "var(--fg-dim)" }}
    >
      {label}
    </button>
  );
}
