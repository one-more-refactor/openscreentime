import { DotMatrix } from "./DotMatrix";

interface Props {
  /** the big value, e.g. "07" or "42" */
  value: string;
  /** mono uppercase caption under it */
  caption: string;
  /** tint the numeral with the accent (locked/tamper only) */
  accent?: boolean;
  size?: "md" | "lg" | "xl";
  className?: string;
}

const dotSize = { md: 4, lg: 6, xl: 8 };

// Big stat as a true 5×7 LED matrix with a silkscreen caption below.
export function Stat({ value, caption, accent, size = "lg", className = "" }: Props) {
  return (
    <div className={`flex flex-col gap-2 ${className}`}>
      <DotMatrix
        text={value}
        dot={dotSize[size]}
        color={accent ? "var(--accent)" : "var(--fg)"}
      />
      <span className="label">{caption}</span>
    </div>
  );
}
