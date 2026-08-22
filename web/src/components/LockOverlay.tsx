import type { CSSProperties } from "react";
import type { UnlockChallenge } from "../types";

interface Props {
  mode?: "locked" | "timesup" | "earn";
  /** big dot-numeral countdown, e.g. "00:00" or streak flame text */
  countdown?: string;
  challenge?: UnlockChallenge;
  earnLabel?: string;
  earnMinutes?: number;
  /** render inside a device-frame preview instead of true fullscreen */
  preview?: boolean;
}

// The agent screen is deliberately ALWAYS dark regardless of console theme;
// scope its own token block here so a global token retune reaches it in one
// place instead of scattered hex literals.
const overlayVars: CSSProperties = {
  ["--lo-bg" as string]: "#000000",
  ["--lo-dot" as string]: "#242424",
  ["--lo-fg" as string]: "#fafafa",
  ["--lo-dim" as string]: "#7a7a7a",
  ["--lo-faint" as string]: "#5a5a5a",
  ["--lo-ok" as string]: "var(--ok)",
  ["--lo-accent" as string]: "var(--accent)",
};

// Design-reference mock of the host-side full-screen interruption the AGENT
// renders. Shares the language: black bg, dot grid, one big dot-numeral, one
// accent-red action, mono uppercase copy. Calm and game-like, not punitive.
export function LockOverlay({
  mode = "timesup",
  countdown = "00:00",
  challenge = "math",
  earnLabel = "Read for 20 min",
  earnMinutes = 15,
  preview = true,
}: Props) {
  const headline =
    mode === "locked" ? "LOCKED" : mode === "earn" ? "EARN MORE TIME" : "TIME'S UP";

  const challengeCopy: Record<UnlockChallenge, string> = {
    math: "SOLVE A PROBLEM TO CONTINUE",
    wait: "COOL-DOWN IN PROGRESS",
    parent_pin: "ASK A PARENT FOR THEIR CODE",
  };

  return (
    <div
      className={`relative overflow-hidden flex flex-col items-center justify-center text-center select-none ${
        preview ? "rounded aspect-video hairline" : "fixed inset-0 z-50"
      }`}
      style={{
        ...overlayVars,
        background: "var(--lo-bg)",
        color: "var(--lo-fg)",
        borderColor: "var(--line-2)",
      }}
    >
      {/* dot grid */}
      <div
        className="absolute inset-0 opacity-60"
        style={{
          backgroundImage: "radial-gradient(var(--lo-dot) 1px, transparent 1px)",
          backgroundSize: "18px 18px",
        }}
        aria-hidden
      />

      <div className="relative z-10 flex flex-col items-center gap-5 px-6">
        <span
          className="label"
          style={{ color: "var(--lo-dim)", letterSpacing: "0.3em" }}
        >
          OPENSCREENTIME
        </span>

        <h1
          className="dot text-2xl"
          style={{ color: mode === "earn" ? "var(--lo-ok)" : "var(--lo-fg)" }}
        >
          {headline}
        </h1>

        {mode === "earn" ? (
          <>
            <span className="dot text-6xl" style={{ color: "var(--lo-ok)" }}>
              +{earnMinutes}
            </span>
            <p className="text-xs" style={{ color: "var(--lo-dim)" }}>
              {earnLabel.toUpperCase()}
            </p>
          </>
        ) : (
          <>
            <span
              className="dot text-7xl tabular-nums"
              style={{ color: "var(--lo-fg)" }}
            >
              {countdown}
            </span>
            <p className="label" style={{ color: "var(--lo-dim)" }}>
              {mode === "locked" ? "AWAITING ADMIN UNLOCK" : challengeCopy[challenge]}
            </p>
          </>
        )}

        <button
          type="button"
          className="mt-2 border rounded px-4 py-2 font-mono uppercase tracking-label text-xs"
          style={{
            borderColor: "var(--lo-accent)",
            color: mode === "earn" ? "#fff" : "var(--lo-accent)",
            background: mode === "earn" ? "var(--lo-accent)" : "transparent",
          }}
        >
          {mode === "locked"
            ? "REQUEST UNLOCK"
            : mode === "earn"
              ? "START TASK"
              : "EARN 15 MIN — READ FOR 20"}
        </button>
      </div>

      {preview && (
        <span
          className="absolute top-2 left-2 label z-10"
          style={{ color: "var(--lo-faint)" }}
        >
          PREVIEW · AGENT GUI
        </span>
      )}
    </div>
  );
}
