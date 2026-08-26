// ============================================================================
// FluentSlider — a range input with a filled track and a live value readout.
// The value moves with the drag; the change is committed once, on release,
// so a slide across the scale is one step-up + one save, not forty.
// ============================================================================
import { useEffect, useState } from "react";

interface Props {
  min: number;
  max: number;
  step: number;
  value: number;
  /** live formatted readout shown while dragging */
  format: (v: number) => string;
  onCommit: (v: number) => void;
  /** fires on every movement, before release — for previews (e.g. the theme) */
  onLive?: (v: number) => void;
  disabled?: boolean;
  "aria-label": string;
}

export function FluentSlider({
  min,
  max,
  step,
  value,
  format,
  onCommit,
  onLive,
  disabled,
  "aria-label": ariaLabel,
}: Props) {
  const [live, setLive] = useState(value);
  const [dragging, setDragging] = useState(false);
  // Follow the outside value unless the user is mid-drag.
  useEffect(() => {
    if (!dragging) setLive(value);
  }, [value, dragging]);

  const pct = max > min ? ((live - min) / (max - min)) * 100 : 0;

  function commit() {
    setDragging(false);
    if (live !== value) onCommit(live);
  }

  return (
    <div className="fs" data-dragging={dragging}>
      <span className="fs-value">{format(live)}</span>
      <input
        type="range"
        className="fs-range"
        min={min}
        max={max}
        step={step}
        value={live}
        disabled={disabled}
        aria-label={ariaLabel}
        aria-valuetext={format(live)}
        style={{ "--fs-pct": `${pct}%` } as React.CSSProperties}
        onChange={(e) => {
          setDragging(true);
          setLive(Number(e.target.value));
          onLive?.(Number(e.target.value));
        }}
        onPointerUp={commit}
        onKeyUp={(e) => {
          if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"].includes(e.key)) commit();
        }}
        onBlur={commit}
      />
    </div>
  );
}
