interface Props {
  start: string;
  end: string;
  onChange: (start: string, end: string) => void;
  disabled?: boolean;
}

// A start→end pair of 24h time fields, mono styled.
export function TimeRange({ start, end, onChange, disabled }: Props) {
  return (
    <div className="inline-flex items-center gap-2">
      <TimeField value={start} onChange={(v) => onChange(v, end)} disabled={disabled} />
      <span className="text-fg-faint text-xs" aria-hidden>
        →
      </span>
      <TimeField value={end} onChange={(v) => onChange(start, v)} disabled={disabled} />
    </div>
  );
}

function TimeField({
  value,
  onChange,
  disabled,
}: {
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
}) {
  return (
    <input
      type="time"
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value)}
      className="focusable bg-transparent border rounded px-2 py-1 text-sm font-mono tabular-nums text-fg disabled:opacity-40"
      style={{ borderColor: "var(--line-2)", colorScheme: "dark light" }}
    />
  );
}
