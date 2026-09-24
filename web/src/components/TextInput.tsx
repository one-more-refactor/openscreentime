import type { InputHTMLAttributes, SelectHTMLAttributes } from "react";

interface Props extends InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  hint?: string;
  /** Shows the hint as the error, in stop red, and marks the field invalid. */
  error?: string | null;
}

function idFor(prefix: string, label?: string): string | undefined {
  return label ? `${prefix}-${label.replace(/\W+/g, "-").toLowerCase()}` : undefined;
}

// A field with a sentence-case label above it (brand board): white, a
// line-2 hairline, 10 px radius, 44 px tall, Figtree 16 px — never mono.
// Focus is a brand border with a soft glow.
export function TextInput({ label, hint, error, className = "", id, ...rest }: Props) {
  const inputId = id ?? idFor("in", label);
  const hintId = inputId && (hint || error) ? `${inputId}-hint` : undefined;
  return (
    <div className={`fieldset ${className}`}>
      {label && (
        <label htmlFor={inputId} className="label">
          {label}
        </label>
      )}
      <input
        id={inputId}
        className="field"
        aria-invalid={error ? true : undefined}
        aria-describedby={hintId}
        {...rest}
      />
      {(error || hint) && (
        <span id={hintId} className="hint" data-error={!!error}>
          {error || hint}
        </span>
      )}
    </div>
  );
}

interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  label?: string;
  hint?: string;
}

export function Select({ label, hint, className = "", id, children, ...rest }: SelectProps) {
  const selId = id ?? idFor("sel", label);
  return (
    <div className={`fieldset ${className}`}>
      {label && (
        <label htmlFor={selId} className="label">
          {label}
        </label>
      )}
      <select id={selId} className="field field-select" {...rest}>
        {children}
      </select>
      {hint && <span className="hint">{hint}</span>}
    </div>
  );
}
