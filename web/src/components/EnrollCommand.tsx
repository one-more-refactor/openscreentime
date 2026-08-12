import { Button } from "./Button";

/**
 * The enroll instructions shown after creating a device / regenerating a
 * token. Primary path: the curl|sh one-liner served by the server itself
 * (GET /install.sh) with the token passed via env so it stays out of argv
 * and shell history. Secondary: the manual from-source enroll command.
 */
export function EnrollCommand({ token }: { token: string }) {
  const origin = window.location.origin;
  const oneLiner = `curl -fsSL ${origin}/install.sh | sudo OST_TOKEN=${token} sh -s -- --server ${origin}`;
  const manual = `sudo ./openscreentime enroll \\
  --server ${origin} \\
  --token ${token}
sudo ./openscreentime install-service`;

  return (
    <div className="flex flex-col gap-3">
      <div
        className="border rounded"
        style={{ borderColor: "var(--line-2)", background: "var(--bg)" }}
      >
        <div
          className="flex items-center justify-between gap-3 px-3 py-2 border-b"
          style={{ borderColor: "var(--line)" }}
        >
          <span className="label" style={{ color: "var(--fg-faint)" }}>
            RUN AS ROOT ON THE DEVICE
          </span>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => navigator.clipboard?.writeText(oneLiner)}
          >
            COPY
          </Button>
        </div>
        <pre className="text-[0.6875rem] text-fg p-3 overflow-x-auto whitespace-pre-wrap break-all">
          {oneLiner}
        </pre>
      </div>
      <details>
        <summary
          className="focusable label cursor-pointer select-none"
          style={{ color: "var(--fg-faint)" }}
        >
          MANUAL INSTALL (BINARY BUILT FROM SOURCE)
        </summary>
        <pre
          className="text-[0.6875rem] border rounded p-3 mt-2 overflow-x-auto"
          style={{
            borderColor: "var(--line)",
            background: "var(--surface-2)",
            color: "var(--fg-dim)",
          }}
        >
          {manual}
        </pre>
      </details>
    </div>
  );
}
