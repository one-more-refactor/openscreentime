// ============================================================================
// CodeBoxes — a 6-digit code typed in six boxes, grouped three and three
// (brand board § "Signing in — two doors"). One real <input> lies over the
// boxes, so typing, pasting and one-time-code autofill all just work; the
// boxes only draw what it holds. The sixth digit submits.
//
// Deliberately NOT a ring: the ring means time used today and nothing else.
// ============================================================================
import { useEffect, useRef, useState } from "react";

interface Props {
  value: string;
  onChange: (digits: string) => void;
  /** Called once, when the last digit lands. */
  onComplete: (code: string) => void;
  length?: number;
  disabled?: boolean;
  error?: boolean;
  "aria-label": string;
}

export function CodeBoxes({
  value,
  onChange,
  onComplete,
  length = 6,
  disabled = false,
  error = false,
  "aria-label": ariaLabel,
}: Props) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [focused, setFocused] = useState(false);
  const firedFor = useRef<string | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Fire onComplete exactly once per full code.
  useEffect(() => {
    if (value.length === length && firedFor.current !== value) {
      firedFor.current = value;
      onComplete(value);
    }
    if (value.length < length) firedFor.current = null;
  }, [value, length, onComplete]);

  const half = Math.ceil(length / 2);
  const box = (i: number) => (
    <i key={i} className="digit" data-on={focused && !disabled && i === Math.min(value.length, length - 1)}>
      {value[i] ?? ""}
    </i>
  );

  return (
    <div className="digits" data-error={error} onClick={() => inputRef.current?.focus()}>
      <input
        ref={inputRef}
        className="digits-input"
        value={value}
        disabled={disabled}
        inputMode="numeric"
        autoComplete="one-time-code"
        aria-label={ariaLabel}
        aria-invalid={error || undefined}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        onChange={(e) => onChange(e.target.value.replace(/\D/g, "").slice(0, length))}
      />
      {Array.from({ length: half }, (_, i) => box(i))}
      <em className="digits-sep" aria-hidden="true" />
      {Array.from({ length: length - half }, (_, i) => box(half + i))}
    </div>
  );
}
