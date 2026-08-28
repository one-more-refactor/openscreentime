// ============================================================================
// UnlockCodePanel — one computer's keys, owned by the console.
//
// On a child's computer the parent's 6-digit unlock code unlocks the screen,
// reopens time and allows `sudo`. It is verified on the device, offline — but
// the secret behind it never leaves the server and the agent. There is no QR
// to scan, no authenticator entry to keep: when a parent needs the code they
// open this (on their phone, usually), prove it's them once (change mode), and
// read the code that is valid right now. Recovery codes are the phone-is-dead
// fallback: eight one-time 8-digit codes, shown once, generated here.
//
// Used in two places with one body: Add a child (step 2) and Settings →
// Unlock codes. Every read here is a sensitive read (428 without change mode),
// so everything goes through guard().
// ============================================================================
import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import * as api from "../api";
import type { RecoveryCodesStatus, UnlockCode } from "../types";
import { useConfirm, StepUpCancelled } from "../lib/confirm";
import { Button } from "./Button";
import { Modal } from "./Modal";
import { relTime } from "../lib/format";

export interface UnlockCodeDevice {
  id: string;
  name: string;
  status?: "pending" | "online" | "offline";
  last_seen?: string | null;
  recovery_codes_unused?: number;
}

interface Props {
  device: UnlockCodeDevice;
  /** Show the live code as soon as the panel mounts (Add a child, step 2). */
  autoShow?: boolean;
  /** Compact row (Settings list) vs. the full step (Add a child). */
  variant?: "row" | "step";
}

function errMsg(e: unknown, fallback: string): string {
  return e instanceof Error && e.message ? e.message : fallback;
}

/** The code as it reads aloud: "123 456". */
function spaced(code: string): string {
  return code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : code;
}

export function UnlockCodePanel({ device, autoShow = false, variant = "row" }: Props) {
  const { guard } = useConfirm();
  const [code, setCode] = useState<UnlockCode | null>(null);
  const [showing, setShowing] = useState(autoShow);
  const [secondsLeft, setSecondsLeft] = useState(0);
  const [recovery, setRecovery] = useState<RecoveryCodesStatus | null>(
    device.recovery_codes_unused !== undefined
      ? { unused: device.recovery_codes_unused, total: 8, generated_at: null }
      : null,
  );
  const [fresh, setFresh] = useState<string[] | null>(null);
  const [confirmGenerate, setConfirmGenerate] = useState(false);
  const [confirmReplace, setConfirmReplace] = useState(false);
  const [busy, setBusy] = useState(false);
  // One line of inline status: a note ("New unlock code…") or a failure.
  const [status, setStatus] = useState<{ msg: string; crit: boolean } | null>(null);
  const fetching = useRef(false);

  const note = useCallback((msg: string) => setStatus({ msg, crit: false }), []);
  const fail = useCallback((msg: string) => setStatus({ msg, crit: true }), []);

  /** Read the code valid right now. A sensitive read: guarded. */
  const load = useCallback(async () => {
    if (fetching.current) return;
    fetching.current = true;
    try {
      const c = await guard(() => api.getUnlockCode(device.id));
      setCode(c);
      setSecondsLeft(c.seconds_left);
      // A read that works clears an earlier read failure — never a note.
      setStatus((prev) => (prev?.crit ? null : prev));
    } catch (e) {
      if (e instanceof StepUpCancelled) {
        setShowing(false);
        return;
      }
      fail(errMsg(e, "Couldn't read the unlock code."));
    } finally {
      fetching.current = false;
    }
  }, [device.id, guard, fail]);

  const loadRecovery = useCallback(async () => {
    try {
      setRecovery(await guard(() => api.getRecoveryCodes(device.id)));
    } catch {
      /* the count is a nicety; the code is the point */
    }
  }, [device.id, guard]);

  // Showing = fetch now, then tick down and refetch each time the code rolls.
  useEffect(() => {
    if (!showing) return;
    void load();
    void loadRecovery();
  }, [showing, load, loadRecovery]);

  useEffect(() => {
    if (!showing || !code) return;
    const t = setInterval(() => {
      setSecondsLeft((s) => {
        if (s <= 1) {
          void load();
          return 0;
        }
        return s - 1;
      });
    }, 1000);
    return () => clearInterval(t);
  }, [showing, code, load]);

  async function generate() {
    setConfirmGenerate(false);
    setBusy(true);
    setStatus(null);
    try {
      const r = await guard(() => api.generateRecoveryCodes(device.id));
      setFresh(r.codes);
      setRecovery({ unused: r.codes.length, total: r.codes.length, generated_at: r.generated_at });
    } catch (e) {
      if (!(e instanceof StepUpCancelled)) fail(errMsg(e, "Couldn't make recovery codes."));
    } finally {
      setBusy(false);
    }
  }

  async function replace() {
    setConfirmReplace(false);
    setBusy(true);
    setStatus(null);
    try {
      const c = await guard(() => api.rotateUnlockCode(device.id));
      setCode(c);
      setSecondsLeft(c.seconds_left);
      setShowing(true);
      setRecovery({ unused: 0, total: recovery?.total ?? 8, generated_at: null });
      note(
        c.recovery_codes_cleared
          ? "New unlock code. The old one stops working once the computer checks in — and its recovery codes are gone, so make new ones."
          : "New unlock code. The old one stops working once the computer checks in.",
      );
    } catch (e) {
      if (!(e instanceof StepUpCancelled)) fail(errMsg(e, "Couldn't replace the unlock code."));
    } finally {
      setBusy(false);
    }
  }

  const pending = device.status === "pending";
  const period = code?.period ?? 30;
  const frac = code ? secondsLeft / period : 0;
  const unused = recovery?.unused ?? 0;

  return (
    <div className="uc" data-variant={variant}>
      {variant === "row" && (
        <div className="uc-head">
          <span className="uc-name">{device.name}</span>
          <span className="uc-meta">
            {pending
              ? "not set up yet"
              : device.last_seen
                ? `last heard ${relTime(device.last_seen)}`
                : ""}
            {recovery && !pending ? ` · ${unused} of ${recovery.total} recovery codes left` : ""}
          </span>
          <span className="uc-actions">
            <button
              type="button"
              className="ch-btn"
              disabled={busy}
              onClick={() => setShowing((s) => !s)}
              aria-expanded={showing}
            >
              {showing ? "Hide code" : "Show code"}
            </button>
            <button
              type="button"
              className="ch-btn"
              disabled={busy}
              onClick={() => (unused > 0 ? setConfirmGenerate(true) : void generate())}
            >
              Recovery codes
            </button>
            <button type="button" className="ch-btn" disabled={busy} onClick={() => setConfirmReplace(true)}>
              Replace
            </button>
          </span>
        </div>
      )}

      {showing && (
        <div className="uc-live" data-testid="unlock-code-live">
          <div className="uc-ring" style={{ "--p": frac } as CSSProperties} aria-hidden="true">
            <span className="uc-ring-s">{code ? secondsLeft : "·"}</span>
          </div>
          <div className="uc-code-wrap">
            <p className="uc-label">Unlock code for {device.name}</p>
            <p className="uc-code" aria-live="polite" aria-atomic="true">
              {code ? spaced(code.code) : "··· ···"}
            </p>
            <p className="uc-note">
              {code
                ? `Changes in ${secondsLeft}s · works on the computer even offline`
                : (status?.msg ?? "Reading…")}
            </p>
          </div>
        </div>
      )}

      {variant === "step" && (
        <div className="uc-step-actions">
          <button
            type="button"
            className="ch-btn"
            disabled={busy}
            onClick={() => (unused > 0 ? setConfirmGenerate(true) : void generate())}
          >
            {unused > 0 ? `Recovery codes · ${unused} left` : "Make recovery codes"}
          </button>
          <button type="button" className="ch-btn" disabled={busy} onClick={() => setConfirmReplace(true)}>
            Replace the code
          </button>
        </div>
      )}

      {status && (!showing || code) && (
        <p className="dev-inline-status" data-tone={status.crit ? "crit" : undefined} role="status">
          {status.msg}
        </p>
      )}

      {/* Fresh recovery codes — shown once. */}
      <Modal
        open={!!fresh}
        onClose={() => setFresh(null)}
        title="Recovery codes"
        footer={
          <>
            <Button variant="ghost" onClick={() => window.print()}>
              PRINT
            </Button>
            <Button
              variant="ghost"
              onClick={() => void navigator.clipboard?.writeText((fresh ?? []).join("\n"))}
            >
              COPY ALL
            </Button>
            <Button onClick={() => setFresh(null)}>I've saved them</Button>
          </>
        }
      >
        <div className="rc-sheet">
          <p className="text-sm" style={{ color: "var(--fg-dim)", margin: 0 }}>
            For <span style={{ color: "var(--fg)" }}>{device.name}</span>, when your phone is out of
            reach. Each code works once, on the computer itself, with no internet. They are shown
            only now — print them or put them somewhere safe.
          </p>
          <ol className="rc-grid">
            {(fresh ?? []).map((c) => (
              <li key={c} className="rc-code">
                {c}
              </li>
            ))}
          </ol>
          <p className="rc-foot">OpenScreenTime · {device.name} · recovery codes</p>
        </div>
      </Modal>

      <Modal
        open={confirmGenerate}
        onClose={() => setConfirmGenerate(false)}
        title="New recovery codes"
        danger
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmGenerate(false)} disabled={busy}>
              CANCEL
            </Button>
            <Button variant="danger" disabled={busy} onClick={() => void generate()}>
              {busy ? "MAKING…" : "MAKE NEW CODES"}
            </Button>
          </>
        }
      >
        <p className="text-xs leading-relaxed" style={{ color: "var(--fg-dim)" }}>
          <span className="dot text-fg">{device.name}</span> still has {unused} unused recovery
          {unused === 1 ? " code" : " codes"}. Making new ones throws those away — anything you
          printed stops working once the computer checks in.
        </p>
      </Modal>

      <Modal
        open={confirmReplace}
        onClose={() => setConfirmReplace(false)}
        title="Replace unlock code"
        danger
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmReplace(false)} disabled={busy}>
              CANCEL
            </Button>
            <Button variant="danger" disabled={busy} onClick={() => void replace()}>
              {busy ? "REPLACING…" : "REPLACE"}
            </Button>
          </>
        }
      >
        <p className="text-xs leading-relaxed" style={{ color: "var(--fg-dim)" }}>
          Give <span className="dot text-fg">{device.name}</span> a new key? The current codes stop
          working as soon as that computer next checks in
          {unused > 0 ? `, and its ${unused} recovery ${unused === 1 ? "code is" : "codes are"} cleared too` : ""}
          . Do this if you think someone has been reading the code over your shoulder.
        </p>
      </Modal>
    </div>
  );
}
