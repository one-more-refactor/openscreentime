// ============================================================================
// PAUSE EVERYTHING — the one control that stops every screen in the house.
//
// Three deliberate decisions:
//
// 1. **Hold, don't tap.** Pausing the whole house mid-sentence is not an
//    action to fire on a stray click. A bar fills along the card while you
//    hold, for 600 ms, and only then commits — long enough to mean it, short
//    enough to never feel like a chore. Releasing early cancels, visibly.
//    (A bar, not a ring: the ring only ever means time used today.)
//    Resuming is a plain tap: undoing a pause needs no ceremony.
//
// 2. **It reports what actually happened.** Pausing N computers is N commands
//    that can individually fail. One that is offline gets the command queued,
//    not applied — and the copy says so, instead of claiming the house is
//    paused when one laptop never got the message.
//
// 3. **It can be undone** from the note that reports it.
// ============================================================================
import { useCallback, useEffect, useRef, useState } from "react";
import { STEP_UP_REQUIRED, type Device } from "../types";
import { ApiError, lockDevice, unlockDevice } from "../api";
import { useConfirm, StepUpCancelled } from "../lib/confirm";
import { useToast } from "../lib/toast";
import { Icon } from "./Icon";

/** How long the hold must last before the pause commits. */
const HOLD_MS = 600;

interface Props {
  devices: Device[];
  allPaused: boolean;
  /** Drives the calm settle across the family grid. */
  onSweep: (sweeping: boolean) => void;
  onDone: () => void | Promise<void>;
}

type Phase = "idle" | "holding" | "working";

const computers = (n: number) => `${n} ${n === 1 ? "computer" : "computers"}`;

export function PauseEverything({ devices, allPaused, onSweep, onDone }: Props) {
  const { guard } = useConfirm();
  const { toast } = useToast();
  const [phase, setPhase] = useState<Phase>("idle");
  const [progress, setProgress] = useState(0);
  const raf = useRef(0);
  const startedAt = useRef(0);
  const committed = useRef(false);
  const runRef = useRef<(pause: boolean) => Promise<void>>(async () => {});

  const cancelHold = useCallback(() => {
    cancelAnimationFrame(raf.current);
    committed.current = false;
    setProgress(0);
    setPhase((p) => (p === "holding" ? "idle" : p));
  }, []);

  useEffect(() => () => cancelAnimationFrame(raf.current), []);

  const run = useCallback(
    async (pause: boolean) => {
      setPhase("working");
      if (pause) onSweep(true);
      try {
        const results = await guard(async () => {
          const settled = await Promise.allSettled(
            devices.map((d) => (pause ? lockDevice(d.id) : unlockDevice(d.id))),
          );
          // allSettled would swallow the server's "prove it's you" — rethrow
          // it so guard() can ask once and re-run the whole batch (pausing is
          // idempotent, so the retry is safe).
          const ask = settled.find(
            (r): r is PromiseRejectedResult =>
              r.status === "rejected" &&
              r.reason instanceof ApiError &&
              r.reason.code === STEP_UP_REQUIRED,
          );
          if (ask) throw ask.reason;
          return settled;
        });

        const failed = results.filter((r) => r.status === "rejected").length;
        // `delivered: false` means the command is queued for a computer that
        // is not connected right now — true when it next checks in, not now.
        const queued = results.filter((r) => r.status === "fulfilled" && !r.value.delivered).length;
        const undo = { label: "Undo", run: () => runRef.current(!pause) };

        if (failed === results.length) {
          toast(pause ? "Couldn't pause anything. Try again." : "Couldn't resume. Try again.", "crit");
        } else if (failed > 0) {
          toast(
            `${results.length - failed} of ${results.length} ${pause ? "paused" : "resumed"} — ${failed} failed.`,
            "warn",
          );
        } else if (queued > 0) {
          toast(
            pause
              ? `Paused. ${computers(queued)} ${queued === 1 ? "is" : "are"} offline and will pause when back online.`
              : `Resumed. ${computers(queued)} offline will follow when back online.`,
            "warn",
            undo,
          );
        } else {
          toast(pause ? "Every screen is paused." : "Everyone is back on.", "ok", undo);
        }
        await onDone();
      } catch (e) {
        if (!(e instanceof StepUpCancelled)) {
          toast(e instanceof Error ? e.message : "That didn't work.", "crit");
        }
      } finally {
        setTimeout(() => onSweep(false), 520);
        setProgress(0);
        setPhase("idle");
      }
    },
    [devices, guard, onDone, onSweep, toast],
  );
  runRef.current = run;

  function beginHold() {
    if (phase === "working") return;
    // Resuming is a plain tap — only the pausing direction is held.
    if (allPaused) {
      void run(false);
      return;
    }
    committed.current = false;
    startedAt.current = performance.now();
    setPhase("holding");
    const tick = (now: number) => {
      const t = Math.min(1, (now - startedAt.current) / HOLD_MS);
      setProgress(t);
      if (t >= 1) {
        committed.current = true;
        void run(true);
        return;
      }
      raf.current = requestAnimationFrame(tick);
    };
    raf.current = requestAnimationFrame(tick);
  }

  function endHold() {
    if (committed.current) return;
    cancelHold();
  }

  const busy = phase === "working";
  const label = allPaused
    ? busy
      ? "Resuming…"
      : "Resume everything"
    : busy
      ? "Pausing…"
      : "Pause everything";
  const hint = allPaused
    ? "Every screen in the house is paused. Tap to resume."
    : phase === "holding"
      ? "Keep holding…"
      : `Stops all ${computers(devices.length)} at once. Press and hold for a second.`;

  return (
    <div className="pause card" data-paused={allPaused} data-phase={phase}>
      <button
        type="button"
        className="pause-btn"
        data-phase={phase}
        aria-label={label}
        aria-pressed={allPaused}
        disabled={busy}
        onPointerDown={beginHold}
        onPointerUp={endHold}
        onPointerLeave={endHold}
        onPointerCancel={endHold}
        // Keyboard: space/enter can't express a hold, so they commit directly.
        // Requiring a held key would make the control unusable without a mouse.
        onKeyDown={(e) => {
          if ((e.key === " " || e.key === "Enter") && !e.repeat && !busy) {
            e.preventDefault();
            void run(!allPaused);
          }
        }}
      >
        <Icon name={allPaused ? "play" : "pause"} size={22} />
      </button>
      <div className="pause-copy">
        <p className="pause-label">{label}</p>
        <p className="pause-hint">{hint}</p>
      </div>
      <span className="pause-hold" style={{ transform: `scaleX(${progress})` }} aria-hidden="true" />
    </div>
  );
}
