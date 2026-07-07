import type { DeviceStatus, Severity } from "../types";

export type LedTone = "ok" | "warn" | "crit" | "idle" | "pending";

const toneColor: Record<LedTone, string> = {
  ok: "var(--ok)",
  warn: "var(--warn)",
  crit: "var(--crit)",
  idle: "var(--idle)",
  pending: "var(--warn)",
};

const toneGlow: Record<LedTone, string> = {
  ok: "led-glow-ok",
  warn: "led-glow-warn",
  crit: "led-glow-crit",
  idle: "led-glow-idle",
  pending: "led-glow-warn",
};

export function statusTone(status: DeviceStatus): LedTone {
  switch (status) {
    case "online":
      return "ok";
    case "locked":
      return "crit";
    case "pending":
      return "pending";
    case "offline":
    default:
      return "idle";
  }
}

export function severityTone(sev: Severity): LedTone {
  switch (sev) {
    case "critical":
      return "crit";
    case "warn":
      return "warn";
    case "info":
    default:
      return "ok";
  }
}

interface Props {
  tone: LedTone;
  label?: string;
  pulse?: boolean;
  className?: string;
}

// Small filled circle with a soft glow, optionally labelled with a mono caption.
export function StatusLed({ tone, label, pulse, className = "" }: Props) {
  return (
    <span className={`inline-flex items-center gap-2 ${className}`}>
      <span
        className={`led ${toneGlow[tone]} ${pulse ? "led-pulse" : ""}`}
        style={{ background: toneColor[tone] }}
        aria-hidden
      />
      {label !== undefined && (
        <span className="label" style={{ color: "var(--fg-dim)" }}>
          {label}
        </span>
      )}
    </span>
  );
}
