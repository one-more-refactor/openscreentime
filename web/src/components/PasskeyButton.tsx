import { useState } from "react";

interface Props {
  onActivate: () => Promise<void> | void;
  label: string;
  busyLabel?: string;
  disabled?: boolean;
  variant?: "primary" | "danger";
}

// The passkey affordance: a wide mono-outline button with a pixel key glyph.
// Owns its own busy/lock state during the WebAuthn ceremony.
export function PasskeyButton({
  onActivate,
  label,
  busyLabel = "WAITING FOR PASSKEY…",
  disabled,
  variant = "primary",
}: Props) {
  const [busy, setBusy] = useState(false);

  async function handle() {
    if (busy || disabled) return;
    setBusy(true);
    try {
      await onActivate();
    } finally {
      setBusy(false);
    }
  }

  const border = variant === "danger" ? "var(--accent)" : "var(--line-2)";

  return (
    <button
      type="button"
      onClick={handle}
      disabled={busy || disabled}
      className="focusable group w-full flex items-center justify-center gap-3 border rounded px-4 py-3.5 font-mono uppercase tracking-label text-sm text-fg transition-colors hover:bg-fg hover:text-bg disabled:opacity-50 disabled:hover:bg-transparent disabled:hover:text-fg"
      style={{ borderColor: border }}
    >
      <KeyGlyph className={busy ? "led-pulse" : ""} />
      {busy ? busyLabel : label}
    </button>
  );
}

function KeyGlyph({ className = "" }: { className?: string }) {
  // pixel-style key, monoline
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 16 16"
      className={className}
      aria-hidden
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
    >
      <circle cx="5" cy="5" r="3" />
      <path d="M7 7 L13 13" />
      <path d="M11 11 L12.5 9.5" />
      <path d="M13 13 L14 12" />
    </svg>
  );
}
