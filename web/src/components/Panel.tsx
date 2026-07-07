import type { ReactNode } from "react";

interface Props {
  title?: string;
  /** right-aligned header slot (actions, LED, count) */
  aside?: ReactNode;
  /** dot-grid texture behind the body — for empty/hero panels */
  dots?: boolean;
  className?: string;
  bodyClassName?: string;
  children?: ReactNode;
}

// Bordered card. Hairline border, mono uppercase header, optional dot-grid body.
export function Panel({
  title,
  aside,
  dots = false,
  className = "",
  bodyClassName = "",
  children,
}: Props) {
  return (
    <section
      className={`bg-surface hairline rounded ${className}`}
      style={{ borderColor: "var(--line)" }}
    >
      {(title || aside) && (
        <header
          className="flex items-center justify-between gap-3 px-4 h-11 border-b"
          style={{ borderColor: "var(--line)" }}
        >
          {title ? (
            <h2 className="label" style={{ color: "var(--fg-dim)" }}>
              {title}
            </h2>
          ) : (
            <span />
          )}
          {aside && <div className="flex items-center gap-2">{aside}</div>}
        </header>
      )}
      <div className={`${dots ? "dotgrid" : ""} p-4 ${bodyClassName}`}>
        {children}
      </div>
    </section>
  );
}
