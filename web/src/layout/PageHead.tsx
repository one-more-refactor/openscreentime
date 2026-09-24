import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { Icon } from "../components/Icon";

interface Props {
  /** The small line above the title — the section's name ("Family"). */
  eyebrow?: ReactNode;
  title: ReactNode;
  /** One quiet sentence under the title: the verdict. */
  sub?: ReactNode;
  /** Something left of the title — a face. */
  lead?: ReactNode;
  /** Right-aligned actions: usually the page's one primary button. */
  actions?: ReactNode;
  /** A way back, shown in place of the eyebrow: { to, label }. */
  back?: { to: string; label: string };
  /** Extra content under the title block (pickers, identity rows). */
  children?: ReactNode;
}

/**
 * Every page opens the same way (brand board, console shell): a small crumb,
 * one H1, one verdict sentence, the page's action on the right.
 */
export function PageHead({ eyebrow, title, sub, lead, actions, back, children }: Props) {
  const crumb = back ? (
    <Link to={back.to} className="ph-crumb">
      <Icon name="arrow-left" size={16} />
      {back.label}
    </Link>
  ) : eyebrow ? (
    <p className="ph-crumb">{eyebrow}</p>
  ) : null;

  const text = (
    <div className="ph-main">
      {crumb}
      <h1 className="ph-title">{title}</h1>
      {sub && <p className="ph-sub">{sub}</p>}
      {children}
    </div>
  );

  return (
    <header className="ph">
      {lead ? (
        <div className="ph-row">
          <div className="ph-lead">{lead}</div>
          {text}
        </div>
      ) : (
        text
      )}
      {actions && <div className="ph-actions">{actions}</div>}
    </header>
  );
}
