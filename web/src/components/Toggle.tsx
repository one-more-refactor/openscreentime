interface Props {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
  /** mono caption under the label, hardware-style */
  hint?: string;
  disabled?: boolean;
  /** use accent-red when ON (for "danger"/tamper toggles) */
  danger?: boolean;
}

// Square, hard-cornered switch. Monochrome; optional accent when engaged.
export function Toggle({ checked, onChange, label, hint, disabled, danger }: Props) {
  const onColor = danger ? "var(--accent)" : "var(--fg)";
  return (
    <label
      className={`flex items-center justify-between gap-4 ${
        disabled ? "opacity-40" : "cursor-pointer"
      }`}
    >
      {(label || hint) && (
        <span className="flex flex-col gap-0.5">
          {label && <span className="label">{label}</span>}
          {hint && (
            <span className="text-[0.625rem]" style={{ color: "var(--fg-faint)" }}>
              {hint}
            </span>
          )}
        </span>
      )}
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        onClick={() => !disabled && onChange(!checked)}
        className="focusable relative w-11 h-6 border rounded-[3px] transition-colors flex-none"
        style={{
          borderColor: checked ? onColor : "var(--line-2)",
          background: checked ? onColor : "transparent",
        }}
      >
        <span
          className="absolute top-1/2 -translate-y-1/2 w-4 h-4 rounded-[2px] transition-all"
          style={{
            left: checked ? "calc(100% - 1.125rem)" : "0.125rem",
            background: checked ? "var(--bg)" : "var(--fg-dim)",
          }}
        />
      </button>
    </label>
  );
}
