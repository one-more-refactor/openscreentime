import type { ReactNode } from "react";

interface Props {
  title?: string;
  /** silkscreen ref-code shown top right, e.g. "DV-01" */
  refCode?: string;
  /** right-aligned header slot (actions, LED, count) */
  aside?: ReactNode;
  /** dot-grid texture behind the body — for empty/hero panels */
  dots?: boolean;
  className?: string;
  bodyClassName?: string;
  children?: ReactNode;
}

// Hardware module: hairline border, 7px registration corner ticks, mono
// uppercase silkscreen header with optional ref-code, optional dot-grid body.
export function Panel({
  title,
  refCode,
  aside,
  dots = false,
  className = "",
  bodyClassName = "",
  children,
}: Props) {
  return (
    <section
      className={`relative bg-surface hairline rounded ${className}`}
      style={{ borderColor: "var(--line)" }}
    >
      <span className="tick tick-tl" />
      <span className="tick tick-tr" />
      <span className="tick tick-bl" />
      <span className="tick tick-br" />
      {(title || aside || refCode) && (
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
          <div className="flex items-center gap-3">
            {aside && <div className="flex items-center gap-2">{aside}</div>}
            {refCode && <span className="ref">{refCode}</span>}
          </div>
        </header>
      )}
      <div className={`${dots ? "dotgrid" : ""} p-4 ${bodyClassName}`}>
        {children}
      </div>
    </section>
  );
}
