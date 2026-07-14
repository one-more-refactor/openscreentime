import { Button } from "./Button";

/** Monospace one-liner (enroll token / command) with a COPY button. */
export function TokenBlock({ token }: { token: string }) {
  return (
    <div
      className="flex items-center justify-between gap-3 border rounded px-3 py-2.5"
      style={{ borderColor: "var(--line-2)", background: "var(--bg)" }}
    >
      <code className="text-xs text-fg break-all">{token}</code>
      <Button
        size="sm"
        variant="ghost"
        onClick={() => navigator.clipboard?.writeText(token)}
      >
        COPY
      </Button>
    </div>
  );
}
