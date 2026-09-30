// The OpenScreenTime lockup (brand/gen.py `lockup_h`): the mark, then "Open"
// in 500 ink-2 and "ScreenTime" in 700 ink, tracked −0.02em. The mark box is
// 0.96 × the type size and sits 0.22 × to the left of the text.

/** The mark: the ring at a fixed 40 %, with its tick at twelve. */
export function Mark({ size = 24, className = "" }: { size?: number; className?: string }) {
  return (
    <svg
      className={`mark ${className}`}
      width={size}
      height={size}
      viewBox="0 0 64 64"
      aria-hidden="true"
    >
      <circle cx="32" cy="32" r="22" fill="none" stroke="var(--line-2)" strokeWidth="7" />
      <path d="M32 10A22 22 0 0 1 44.93 49.8" fill="none" stroke="var(--brand)" strokeWidth="7" />
      <circle cx="44.93" cy="49.8" r="3.5" fill="var(--brand)" />
      <path d="M32 4.5v11" stroke="var(--ink)" strokeWidth="3" strokeLinecap="round" fill="none" />
    </svg>
  );
}

interface Props {
  /** Type size in rem. The mark and the gap scale from it. */
  size?: number;
  className?: string;
}

export function Wordmark({ size = 1.0625, className = "" }: Props) {
  return (
    <span
      className={`lockup ${className}`}
      style={{ fontSize: `${size}rem` }}
      role="img"
      aria-label="OpenScreenTime"
    >
      <Mark className="lockup-mark" />
      <span className="lockup-open" aria-hidden="true">
        Open
      </span>
      <span className="lockup-st" aria-hidden="true">
        ScreenTime
      </span>
    </span>
  );
}
