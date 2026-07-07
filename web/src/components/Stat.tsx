interface Props {
  /** the big value, e.g. "07" or "42:00" */
  value: string;
  /** mono uppercase caption under/next to it */
  caption: string;
  /** tint the numeral with the accent */
  accent?: boolean;
  size?: "md" | "lg" | "xl";
  className?: string;
}

const sizeClass = {
  md: "text-3xl",
  lg: "text-5xl",
  xl: "text-7xl",
};

// Oversized dot/LED numeral with a mono caption, e.g. `07 DEVICES`.
export function Stat({ value, caption, accent, size = "lg", className = "" }: Props) {
  return (
    <div className={`flex flex-col gap-1 ${className}`}>
      <span
        className={`dot leading-none tabular-nums ${sizeClass[size]}`}
        style={{ color: accent ? "var(--accent)" : "var(--fg)" }}
      >
        {value}
      </span>
      <span className="label">{caption}</span>
    </div>
  );
}
