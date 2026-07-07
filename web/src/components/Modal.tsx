import type { ReactNode } from "react";
import { useEffect } from "react";

interface Props {
  open: boolean;
  onClose: () => void;
  title: string;
  /** accent-red header for destructive/danger flows */
  danger?: boolean;
  children: ReactNode;
  footer?: ReactNode;
}

// Centered hairline dialog over a dimmed dot-grid backdrop.
export function Modal({ open, onClose, title, danger, children, footer }: Props) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
    >
      <button
        className="absolute inset-0 dotgrid"
        style={{ background: "rgba(0,0,0,0.72)" }}
        onClick={onClose}
        aria-label="close"
      />
      <div
        className="relative w-full max-w-lg bg-surface hairline rounded"
        style={{ borderColor: danger ? "var(--accent)" : "var(--line-2)" }}
      >
        <header
          className="flex items-center justify-between px-4 h-11 border-b"
          style={{ borderColor: "var(--line)" }}
        >
          <h2 className="dot text-[0.6875rem]" style={{ color: danger ? "var(--accent)" : "var(--fg)" }}>
            {title}
          </h2>
          <button
            onClick={onClose}
            className="text-fg-faint hover:text-fg focusable text-sm"
            aria-label="close"
          >
            ✕
          </button>
        </header>
        <div className="p-4">{children}</div>
        {footer && (
          <footer
            className="flex items-center justify-end gap-2 px-4 py-3 border-t"
            style={{ borderColor: "var(--line)" }}
          >
            {footer}
          </footer>
        )}
      </div>
    </div>
  );
}
