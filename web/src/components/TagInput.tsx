import { useState, type KeyboardEvent } from "react";

interface Props {
  label?: string;
  values: string[];
  onChange: (next: string[]) => void;
  placeholder?: string;
  /** show each tag with a tiny leading LED (e.g. allow=ok) */
  tone?: "ok" | "crit" | "neutral";
}

const toneColor = {
  ok: "var(--ok)",
  crit: "var(--crit)",
  neutral: "var(--fg-faint)",
};

// Chip/token input — used for DNS allowlists & port-style lists.
export function TagInput({ label, values, onChange, placeholder, tone = "neutral" }: Props) {
  const [draft, setDraft] = useState("");

  function commit() {
    const v = draft.trim();
    if (!v) return;
    if (!values.includes(v)) onChange([...values, v]);
    setDraft("");
  }

  function onKey(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter" || e.key === "," || e.key === " ") {
      e.preventDefault();
      commit();
    } else if (e.key === "Backspace" && !draft && values.length) {
      onChange(values.slice(0, -1));
    }
  }

  return (
    <div className="flex flex-col gap-1.5">
      {label && <span className="label">{label}</span>}
      <div
        className="flex flex-wrap gap-1.5 border rounded p-2 min-h-[2.5rem]"
        style={{ borderColor: "var(--line-2)" }}
      >
        {values.map((v) => (
          <span
            key={v}
            className="inline-flex items-center gap-1.5 border rounded px-2 py-0.5 text-xs font-mono"
            style={{ borderColor: "var(--line)", background: "var(--surface-2)" }}
          >
            {tone !== "neutral" && (
              <span
                className="led"
                style={{ width: 6, height: 6, background: toneColor[tone] }}
                aria-hidden
              />
            )}
            {v}
            <button
              type="button"
              onClick={() => onChange(values.filter((x) => x !== v))}
              className="text-fg-faint hover:text-accent focusable"
              aria-label={`remove ${v}`}
            >
              ✕
            </button>
          </span>
        ))}
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKey}
          onBlur={commit}
          placeholder={placeholder ?? "add…"}
          className="flex-1 min-w-[6rem] bg-transparent text-sm font-mono text-fg placeholder:text-fg-faint focus:outline-none"
        />
      </div>
    </div>
  );
}
