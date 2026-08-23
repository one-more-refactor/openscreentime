import type { ReactNode } from "react";
import { Link } from "react-router-dom";

interface Props {
  /** Mono eyebrow above the title — the section's name (FAMILY, DEVICES…). */
  eyebrow?: ReactNode;
  title: ReactNode;
  /** One quiet line under the title. */
  sub?: ReactNode;
  /** Something that sits to the left of the title — an avatar. */
  lead?: ReactNode;
  /** Right-aligned actions (a link, a button). */
  actions?: ReactNode;
  /** A back link above everything: { to, label }. */
  back?: { to: string; label: string };
  /** Extra content under the title block (pickers, identity rows). */
  children?: ReactNode;
}

/**
 * Every page opens the same way: an optional back link, a mono eyebrow, one
 * display-weight title, one quiet line, actions on the right. Pages used to
 * each carry their own header markup at their own sizes; this is the one
 * rhythm they all share now.
 */
export function PageHead({ eyebrow, title, sub, lead, actions, back, children }: Props) {
  return (
    <header className="ph">
      {back && (
        <Link to={back.to} className="focusable ph-back">
          ← {back.label}
        </Link>
      )}
      <div className="ph-row">
        {lead && <div className="ph-lead">{lead}</div>}
        <div className="ph-main">
          {eyebrow && <p className="ph-eyebrow">{eyebrow}</p>}
          <h1 className="ph-title">{title}</h1>
          {sub && <p className="ph-sub">{sub}</p>}
          {children}
        </div>
        {actions && <div className="ph-actions">{actions}</div>}
      </div>
    </header>
  );
}
