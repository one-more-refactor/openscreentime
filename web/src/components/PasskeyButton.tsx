import { useState } from "react";
import { Button } from "./Button";

interface Props {
  onActivate: () => Promise<void> | void;
  label: string;
  busyLabel?: string;
  disabled?: boolean;
}

// The passkey affordance: a full-width secondary pill with a key glyph. Owns
// its own busy state while the browser's passkey prompt is up.
export function PasskeyButton({
  onActivate,
  label,
  busyLabel = "Waiting for your passkey…",
  disabled,
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

  return (
    <Button
      type="button"
      variant="secondary"
      className="w-full"
      onClick={() => void handle()}
      disabled={busy || disabled}
    >
      <KeyGlyph />
      {busy ? busyLabel : label}
    </Button>
  );
}

function KeyGlyph() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 16 16"
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
