import type { ReactNode } from "react";
import { useEffect, useRef } from "react";

interface Props {
  open: boolean;
  onClose: () => void;
  title: string;
  /** accent-red header for destructive/danger flows */
  danger?: boolean;
  /** md = dialog, full = full-screen-ish (SSH terminal) */
  size?: "md" | "full";
  /** set false when Escape must reach the content (terminal) */
  closeOnEscape?: boolean;
  children: ReactNode;
  footer?: ReactNode;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), textarea, input, select, [tabindex]:not([tabindex="-1"])';

// Hardware-module dialog over a dimmed dot-grid backdrop. Traps focus, puts
// initial focus on the first focusable element, restores focus on close.
export function Modal({
  open,
  onClose,
  title,
  danger,
  size = "md",
  closeOnEscape = true,
  children,
  footer,
}: Props) {
  const boxRef = useRef<HTMLDivElement>(null);
  const restoreRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;
    restoreRef.current = document.activeElement as HTMLElement | null;

    // Initial focus: first focusable inside the body, else the close button.
    const box = boxRef.current;
    const focusables = () =>
      Array.from(box?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []).filter(
        (el) => el.offsetParent !== null,
      );
    const first = focusables()[1] ?? focusables()[0]; // [0] is the ✕ button
    (first ?? box)?.focus();

    const onKey = (e: KeyboardEvent) => {
      // Keys inside the terminal belong to the remote shell.
      if ((e.target as HTMLElement | null)?.closest?.(".xterm")) return;
      if (e.key === "Escape") {
        if (!closeOnEscape) return;
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== "Tab") return;
      const els = focusables();
      if (!els.length) return;
      const firstEl = els[0];
      const lastEl = els[els.length - 1];
      if (e.shiftKey && document.activeElement === firstEl) {
        e.preventDefault();
        lastEl.focus();
      } else if (!e.shiftKey && document.activeElement === lastEl) {
        e.preventDefault();
        firstEl.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      restoreRef.current?.focus?.();
    };
  }, [open, onClose, closeOnEscape]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
      aria-label={title}
    >
      <div
        className="absolute inset-0 dotgrid"
        style={{ background: "rgba(0,0,0,0.72)" }}
        onClick={onClose}
        aria-hidden
      />
      <div
        ref={boxRef}
        tabIndex={-1}
        className={`relative w-full bg-surface hairline rounded flex flex-col ${
          size === "full" ? "max-w-5xl h-[min(80vh,44rem)]" : "max-w-lg max-h-[85vh]"
        }`}
        style={{ borderColor: danger ? "var(--accent)" : "var(--line-2)" }}
      >
        <span className="tick tick-tl" />
        <span className="tick tick-tr" />
        <span className="tick tick-bl" />
        <span className="tick tick-br" />
        <header
          className="flex items-center justify-between px-4 h-11 border-b flex-none"
          style={{ borderColor: "var(--line)" }}
        >
          <h2
            className="dot text-[0.6875rem]"
            style={{ color: danger ? "var(--accent)" : "var(--fg)" }}
          >
            {title}
          </h2>
          <button
            onClick={onClose}
            className="text-fg-faint hover:text-fg focusable text-sm"
            aria-label="Close dialog"
          >
            ✕
          </button>
        </header>
        <div className={`p-4 flex-1 min-h-0 ${size === "full" ? "flex flex-col" : "overflow-y-auto"}`}>
          {children}
        </div>
        {footer && (
          <footer
            className="flex items-center justify-end gap-2 px-4 py-3 border-t flex-none"
            style={{ borderColor: "var(--line)" }}
          >
            {footer}
          </footer>
        )}
      </div>
    </div>
  );
}
