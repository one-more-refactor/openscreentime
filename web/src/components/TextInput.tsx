import type { InputHTMLAttributes } from "react";

interface Props extends InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  hint?: string;
}

// Hairline text input with a mono uppercase label above.
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
        className="focusable bg-transparent border rounded px-3 py-2 text-sm font-mono text-fg placeholder:text-fg-faint"
        style={{ borderColor: "var(--line-2)" }}
        {...rest}
      />
      {hint && (
        <span className="text-[0.625rem]" style={{ color: "var(--fg-faint)" }}>
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
        className="focusable bg-surface border rounded px-3 py-2 text-sm font-mono uppercase tracking-label text-fg"
        style={{ borderColor: "var(--line-2)" }}
        {...rest}
      >
        {children}
      </select>
      {hint && (
        <span className="text-[0.625rem]" style={{ color: "var(--fg-faint)" }}>
          {hint}
        </span>
      )}
    </div>
  );
}
