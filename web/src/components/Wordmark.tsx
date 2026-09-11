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
      {/* the marque: the activity ring, sized to the x-height. The green is time USED — a
          small arc, most of the day still ahead — the same reading as AvatarRing. */}
      <svg
        viewBox="0 0 64 64"
        aria-hidden="true"
        style={{ width: "0.95em", height: "0.95em", marginRight: "0.4em", alignSelf: "center" }}
      >
        <circle cx="32" cy="32" r="22" fill="none" stroke="var(--line-2)" strokeWidth="9" />
        <path
          d="M 32 10 A 22 22 0 0 1 53.67 35.82"
          fill="none"
          stroke="var(--ok)"
          strokeWidth="9"
          strokeLinecap="round"
        />
      </svg>
      <span style={{ fontWeight: 400, color: "var(--fg-dim)" }}>Open</span>
      <span style={{ fontWeight: 600 }}>ScreenTime</span>
    </span>
  );
}
