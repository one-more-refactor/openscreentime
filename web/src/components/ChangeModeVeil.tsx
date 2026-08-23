// ============================================================================
// ChangeModeVeil — the full-screen beat when change mode turns on or off.
//
// A state change this consequential ("from now on, what you touch changes
// the children's computers") should be felt once, full-screen, rather than
// inferred from a chip. So: an ink field rises, a lock glyph draws itself and
// opens (or closes), one ring sweeps round it, the words say which way it
// went, and the field falls away — the console underneath is already in its
// new state. Entering takes ≈1.1 s, locking ≈0.7 s; it never blocks input
// (pointer-events: none) and is hidden from assistive tech — the rail control
// carries the state for screen readers. Under prefers-reduced-motion it is a
// 150 ms fade with no choreography.
//
// All timing lives in one number (--cm-ms) so the CSS keyframes are percentages
// of the same clock the component uses to unmount itself.
// ============================================================================
import { useEffect, useMemo, type CSSProperties } from "react";

export type VeilKind = "enter" | "lock";

export const VEIL_MS: Record<VeilKind, number> = { enter: 1100, lock: 700 };
export const VEIL_REDUCED_MS = 150;

/** True when the viewer asked for less motion (SSR/test-safe). */
export function prefersReducedMotion(): boolean {
  try {
    return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

interface Props {
  kind: VeilKind;
  /** Called once the veil has fully played out; the owner unmounts it. */
  onDone: () => void;
}

export function ChangeModeVeil({ kind, onDone }: Props) {
  const reduced = useMemo(prefersReducedMotion, []);
  const ms = reduced ? VEIL_REDUCED_MS : VEIL_MS[kind];

  useEffect(() => {
    const t = setTimeout(onDone, ms);
    return () => clearTimeout(t);
    // Re-arming on a new onDone identity would cut a playing veil short.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind, ms]);

  // Ring geometry: one circle, drawn by dashoffset.
  const size = 168;
  const r = 78;
  const c = 2 * Math.PI * r;

  return (
    <div
      className="cm-veil"
      data-kind={kind}
      data-reduced={reduced}
      data-testid="changemode-veil"
      aria-hidden="true"
      style={{ "--cm-ms": `${ms}ms`, "--cm-c": c } as CSSProperties}
    >
      <div className="cm-veil-field dotgrid" />
      <div className="cm-veil-center">
        <div className="cm-veil-mark" style={{ width: size, height: size }}>
          <svg className="cm-veil-ring" viewBox={`0 0 ${size} ${size}`} width={size} height={size}>
            <circle cx={size / 2} cy={size / 2} r={r} fill="none" strokeWidth="2" className="cm-veil-track" />
            <circle
              cx={size / 2}
              cy={size / 2}
              r={r}
              fill="none"
              strokeWidth="2.5"
              strokeLinecap="round"
              className="cm-veil-arc"
              strokeDasharray={c}
              transform={`rotate(-90 ${size / 2} ${size / 2})`}
            />
          </svg>
          <svg className="cm-veil-lock" viewBox="0 0 48 48" width="56" height="56" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            {/* body */}
            <rect x="10" y="21" width="28" height="21" rx="4" className="cm-veil-body" />
            {/* keyhole */}
            <circle cx="24" cy="30" r="2" fill="currentColor" stroke="none" className="cm-veil-key" />
            <path d="M24 32v4" className="cm-veil-key" />
            {/* shackle — hinged at the right leg; "open" lifts the left leg */}
            <path d="M16 21v-5a8 8 0 0 1 16 0v5" className="cm-veil-shackle" />
          </svg>
        </div>
        <p className="cm-veil-title">{kind === "enter" ? "Change mode" : "Locked"}</p>
        <p className="cm-veil-sub">
          {kind === "enter" ? "15 minutes · lock it any time" : "reading stays free"}
        </p>
      </div>
    </div>
  );
}
