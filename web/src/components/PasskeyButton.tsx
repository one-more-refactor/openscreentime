import { useState } from "react";
import { Button, type ButtonVariant } from "./Button";

interface Props {
  onActivate: () => Promise<void> | void;
  label: string;
  busyLabel?: string;
  disabled?: boolean;
  variant?: ButtonVariant;
  /** Full width — the sign-in door. */
  block?: boolean;
}

// The passkey affordance: a pill with the passkey icon. Owns its own busy
// state while the browser's passkey prompt is up.
export function PasskeyButton({
  onActivate,
  label,
  busyLabel = "Waiting for your passkey…",
  disabled,
  variant = "secondary",
  block = true,
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
      variant={variant}
      block={block}
      icon="passkey"
      onClick={() => void handle()}
      disabled={busy || disabled}
    >
      {busy ? busyLabel : label}
    </Button>
  );
}
