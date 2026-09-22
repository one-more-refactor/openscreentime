import type { InputHTMLAttributes } from "react";

interface Props extends InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  hint?: string;
}

// Text input with a sentence-case label (docs/DESIGN.md §5): surface bg, 1px
// line-2, --r-sm, 44px min-height, Figtree 16px — never mono.
export function TextInput({ label, hint, className = "", id, ...rest }: Props) {
  const inputId = id ?? (label ? `in-${label.replace(/\s+/g, "-").toLowerCase()}` : undefined);
  return (
    <div className={`flex flex-col gap-1.5 ${className}`}>
      {label && (
        <label htmlFor={inputId} className="label">
          {label}
        </label>
      )}
      <input
        id={inputId}
        className="ost-field"
        {...rest}
      />
      {hint && (
        <span className="text-[0.78rem]" style={{ color: "var(--ink-3)" }}>
          {hint}
        </span>
      )}
    </div>
  );
}

interface SelectProps
  extends React.SelectHTMLAttributes<HTMLSelectElement> {
  label?: string;
  hint?: string;
}

export function Select({ label, hint, className = "", id, children, ...rest }: SelectProps) {
  const selId = id ?? (label ? `sel-${label.replace(/\s+/g, "-").toLowerCase()}` : undefined);
  return (
    <div className={`flex flex-col gap-1.5 ${className}`}>
      {label && (
        <label htmlFor={selId} className="label">
          {label}
        </label>
      )}
      <select
        id={selId}
        className="ost-field ost-select"
        {...rest}
      >
        {children}
      </select>
      {hint && (
        <span className="text-[0.78rem]" style={{ color: "var(--ink-3)" }}>
          {hint}
        </span>
      )}
    </div>
  );
}
