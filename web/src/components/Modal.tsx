import type { ReactNode } from "react";
import { useEffect, useId, useRef } from "react";
import { Icon } from "./Icon";

interface Props {
  open: boolean;
  onClose: () => void;
  title: string;
  /** A destructive confirm. The title stays ink; the final button is the red one. */
  danger?: boolean;
  children: ReactNode;
  footer?: ReactNode;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), textarea, input:not([disabled]), select, [tabindex]:not([tabindex="-1"])';

/**
 * A dialog in the brand: a white sheet with the large radius over a warm
 * scrim, an H2 title in sentence case, a 44 px close. Traps focus, starts on
 * the first control in the body, Escape closes, focus goes back where it was.
 */
export function Modal({ open, onClose, title, danger, children, footer }: Props) {
  const boxRef = useRef<HTMLDivElement>(null);
  const restoreRef = useRef<HTMLElement | null>(null);
  const titleId = useId();
  // The latest onClose, without re-running the focus effect on every render.
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    if (!open) return;
    restoreRef.current = document.activeElement as HTMLElement | null;
    const box = boxRef.current;
    const focusables = () =>
      Array.from(box?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []).filter(
        (el) => !el.hasAttribute("disabled"),
      );
    const first =
      box?.querySelector(".dialog-body")?.querySelector<HTMLElement>(FOCUSABLE) ??
      box?.querySelector(".dialog-foot")?.querySelector<HTMLElement>(FOCUSABLE);
    (first ?? box)?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        closeRef.current();
        return;
      }
      if (e.key !== "Tab") return;
      const els = focusables();
      if (!els.length) return;
      const head = els[0];
      const tail = els[els.length - 1];
      if (e.shiftKey && document.activeElement === head) {
        e.preventDefault();
        tail.focus();
      } else if (!e.shiftKey && document.activeElement === tail) {
        e.preventDefault();
        head.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      restoreRef.current?.focus?.();
    };
  }, [open]);

  if (!open) return null;

  return (
    <div className="dialog-root" role="dialog" aria-modal="true" aria-labelledby={titleId}>
      <div className="dialog-scrim" onClick={onClose} aria-hidden="true" />
      <div ref={boxRef} tabIndex={-1} className="dialog" data-danger={danger || undefined}>
        <header className="dialog-head">
          <h2 id={titleId} className="dialog-title">
            {title}
          </h2>
          <button type="button" onClick={onClose} className="btn-icon" aria-label="Close">
            <Icon name="close" size={20} />
          </button>
        </header>
        <div className="dialog-body">{children}</div>
        {footer && <footer className="dialog-foot">{footer}</footer>}
      </div>
    </div>
  );
}
