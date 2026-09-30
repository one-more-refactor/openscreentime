import { useEffect, useState } from "react";
import { Icon } from "./Icon";

/**
 * A literal thing to copy — an install command, a pairing code — in the one
 * place monospace is allowed, with a Copy button that says when it worked.
 */
export function CopyField({ value, label = "Copy" }: { value: string; label?: string }) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1800);
    return () => clearTimeout(t);
  }, [copied]);

  async function copy() {
    try {
      await navigator.clipboard?.writeText(value);
      setCopied(true);
    } catch {
      /* the text is selectable; copying by hand still works */
    }
  }

  return (
    <div className="copy">
      <code className="copy-code">{value}</code>
      <button type="button" className="btn btn-secondary btn-sm copy-btn" onClick={() => void copy()}>
        <Icon name={copied ? "check" : "copy"} size={16} />
        {copied ? "Copied" : label}
      </button>
    </div>
  );
}
