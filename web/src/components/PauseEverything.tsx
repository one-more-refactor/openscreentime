// ============================================================================
// PAUSE EVERYTHING — the brief's hero control: "a single prominent control
// freezes every managed screen in the house *now*; tap again to resume."
//
// Three deliberate decisions:
//
// 1. **Hold, don't tap.** Freezing every screen in the house mid-sentence is
//    not an action to fire on a stray click. The ring fills under your finger
//    for 600ms and only then commits — long enough to mean it, short enough
//    that it never feels like a chore. Releasing early cancels, visibly.
//    Resuming is a plain tap: undoing a pause needs no ceremony.
//
// 2. **The freeze is shown, not reported.** On commit the sweep runs across
//    the family grid (the CSS in theme.css keys off [data-sweeping]), so the
//    parent sees the house going quiet rather than reading that it did.
//
// 3. **It reports what actually happened.** Locking N devices means N commands
//    that can individually fail. A device that is offline gets the command
//    queued, not applied — and the copy says so, instead of claiming the house
//    is paused when one laptop never got the message.
// ============================================================================
import { useCallback, useEffect, useRef, useState } from "react";
import type { Device } from "../types";
import { lockDevice, unlockDevice } from "../api";
import { useStepUp, StepUpCancelled } from "../lib/stepup";
import { useToast } from "../lib/toast";

/** How long the hold must last before the pause commits. */
const HOLD_MS = 600;

interface Props {
  devices: Device[];
  allPaused: boolean;
  /** Drives the sweep across the family grid. */
  onSweep: (sweeping: boolean) => void;
  onDone: () => void | Promise<void>;
}

type Phase = "idle" | "holding" | "working";

export function PauseEverything({ devices, allPaused, onSweep, onDone }: Props) {
  const { guard } = useStepUp();
  const { toast } = useToast();
  const [phase, setPhase] = useState<Phase>("idle");
  const [progress, setProgress] = useState(0);
  const raf = useRef(0);
  const startedAt = useRef(0);
  const committed = useRef(false);

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
        const results = await guard(async () =>
          Promise.allSettled(
            devices.map((d) => (pause ? lockDevice(d.id) : unlockDevice(d.id))),
          ),
        );

        const failed = results.filter((r) => r.status === "rejected").length;
        // `delivered: false` means the command is queued for a device that is
        // not currently connected — true when it next checks in, not now.
        const queued = results.filter(
          (r) => r.status === "fulfilled" && !r.value.delivered,
        ).length;

        if (failed === results.length) {
          toast(pause ? "Could not pause anything." : "Could not resume.", "crit");
        } else if (failed > 0) {
          toast(
            `${results.length - failed} of ${results.length} ${pause ? "paused" : "resumed"} — ${failed} failed.`,
            "warn",
          );
        } else if (queued > 0) {
          toast(
            pause
              ? `Paused. ${queued} ${queued === 1 ? "device is" : "devices are"} offline and will pause on reconnect.`
              : `Resumed. ${queued} offline ${queued === 1 ? "device" : "devices"} will follow.`,
            "warn",
          );
        } else {
          toast(pause ? "Every screen is paused." : "Everyone is back on.", "ok");
        }
        await onDone();
      } catch (e) {
        if (!(e instanceof StepUpCancelled)) {
          toast(e instanceof Error ? e.message : "That didn't work.", "crit");
        }
      } finally {
        // Let the sweep finish before the grid settles back.
        setTimeout(() => onSweep(false), 520);
        setProgress(0);
        setPhase("idle");
      }
    },
    [devices, guard, onDone, onSweep, toast],
  );

  function beginHold() {
    if (phase === "working") return;
    // Resuming is a plain tap — only the destructive direction is held.
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
    ? "Every screen in the house is frozen."
    : phase === "holding"
      ? "Keep holding…"
      : `Freezes ${devices.length} ${devices.length === 1 ? "screen" : "screens"} at once. Hold to confirm.`;

  return (
    <div className="pause-wrap" data-paused={allPaused} data-busy={busy}>
      <button
        type="button"
        className="focusable pause-btn"
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
        <span
          className="pause-ring"
          style={{ ["--p" as string]: progress }}
          aria-hidden="true"
        />
        <span className="pause-glyph" aria-hidden="true">
          {allPaused ? (
            <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor">
              <path d="M8 5v14l11-7z" />
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor">
              <rect x="6" y="5" width="4" height="14" rx="1" />
              <rect x="14" y="5" width="4" height="14" rx="1" />
            </svg>
          )}
        </span>
      </button>
      <div className="pause-copy">
        <p className="pause-label">{label}</p>
        <p className="pause-hint">{hint}</p>
      </div>
    </div>
  );
}
