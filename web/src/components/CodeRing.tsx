// ============================================================================
// CodeRing — the second-factor code as a ring that fills, one segment per
// digit. The ring is the input: click focuses a hidden field, digits light
// segments clockwise from 12 o'clock, the sixth digit completes the circle
// and submits. A wrong code flashes the ring red, shakes once, and empties.
// Paste works; backspace unwinds.
// ============================================================================
import { useEffect, useRef, useState } from "react";

interface Props {
  value: string;
  onChange: (digits: string) => void;
  /** called once, when the last segment fills */
  onComplete: (code: string) => void;
  length?: number;
  disabled?: boolean;
  error?: boolean;
  "aria-label": string;
}

/** One arc segment of a circle, centered on `cx,cy`, from a1 to a2 (degrees). */
function arc(cx: number, cy: number, r: number, a1: number, a2: number): string {
  const rad = (a: number) => ((a - 90) * Math.PI) / 180;
  const x1 = cx + r * Math.cos(rad(a1));
  const y1 = cy + r * Math.sin(rad(a1));
  const x2 = cx + r * Math.cos(rad(a2));
  const y2 = cy + r * Math.sin(rad(a2));
  return `M ${x1} ${y1} A ${r} ${r} 0 ${a2 - a1 > 180 ? 1 : 0} 1 ${x2} ${y2}`;
}

export function CodeRing({
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

  const size = 132;
  const r = 58;
  const gap = 10; // degrees between segments
  const span = 360 / length - gap;

  return (
    <div
      className="cr"
      data-error={error}
      data-focused={focused}
      onClick={() => inputRef.current?.focus()}
    >
      <input
        ref={inputRef}
        className="cr-input"
        value={value}
        disabled={disabled}
        inputMode="numeric"
        autoComplete="one-time-code"
        aria-label={ariaLabel}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        onChange={(e) => onChange(e.target.value.replace(/\D/g, "").slice(0, length))}
      />
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden="true">
        {Array.from({ length }, (_, i) => {
          const a1 = i * (span + gap) + gap / 2;
          const filled = i < value.length;
          const next = i === value.length && focused && !disabled;
          return (
            <path
              key={i}
              className="cr-seg"
              data-filled={filled}
              data-next={next}
              d={arc(size / 2, size / 2, r, a1, a1 + span)}
            />
          );
        })}
      </svg>
      <span className="cr-digits" aria-hidden="true">
        {value.length === 0 ? (
          <span className="cr-hint">{focused ? "type the code" : "tap to type"}</span>
        ) : (
          value.split("").join(" ")
        )}
      </span>
    </div>
  );
}
