// The OpenScreenTime wordmark. Replaces the old LED dot-matrix "SENTINEL"
// mark — the rebrand's most visible change and the first step away from the
// loud Nothing-style aesthetic toward "warm, not loud": a clean typographic
// lockup, tight tracking, one weight contrast, no accent noise.
//
// "Open" reads as the quiet qualifier; "ScreenTime" carries the name. One
// component, one place to retune, used in the top bar, login, and anywhere
// the product needs to sign its name.

interface Props {
  /** Font size in rem for the wordmark. The rest scales from it. */
  size?: number;
  /** Override the ink color (defaults to the theme foreground). */
  color?: string;
  className?: string;
}

export function Wordmark({ size = 1.0625, color = "var(--fg)", className = "" }: Props) {
  return (
    <span
      className={`inline-flex items-baseline select-none ${className}`}
      style={{
        fontSize: `${size}rem`,
        letterSpacing: "-0.015em",
        lineHeight: 1,
        color,
      }}
      aria-label="OpenScreenTime"
    >
      <span style={{ fontWeight: 400, color: "var(--fg-dim)" }}>Open</span>
      <span style={{ fontWeight: 600 }}>ScreenTime</span>
    </span>
  );
}
