import { Panel } from "./Panel";
import { Button } from "./Button";
import { StatusLed } from "./StatusLed";

interface Props {
  /** what failed, e.g. "Couldn't load devices" */
  title: string;
  /** the underlying error message from the API */
  detail?: string | null;
  onRetry?: () => void;
  className?: string;
}

// Styled failure panel with a retry affordance — used wherever useAsync fails.
export function ErrorPanel({ title, detail, onRetry, className = "" }: Props) {
  return (
    <Panel dots refCode="ERR" className={className}>
      <div className="flex flex-col items-center gap-3 py-8 text-center">
        <StatusLed tone="crit" pulse />
        <p className="text-xs" style={{ color: "var(--fg)" }}>
          {title}.
        </p>
        {detail && (
          <p className="text-[0.6875rem] max-w-md" style={{ color: "var(--fg-dim)" }}>
            {detail}
          </p>
        )}
        {onRetry && (
          <Button variant="primary" size="sm" onClick={onRetry}>
            RETRY
          </Button>
        )}
      </div>
    </Panel>
  );
}
