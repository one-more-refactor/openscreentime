import { useState } from "react";

/**
 * The one-line install for a computer, with a Copy button. The token rides in
 * an environment variable, so it stays out of argv; the installer asks which
 * login is whose when the computer has several.
 */
export function EnrollCommand({ token }: { token: string }) {
  const [copied, setCopied] = useState(false);
  const origin = window.location.origin;
  const oneLiner = `curl -fsSL ${origin}/install.sh | sudo OST_TOKEN=${token} sh -s -- --server ${origin}`;

  function copy() {
    void navigator.clipboard?.writeText(oneLiner);
    setCopied(true);
    setTimeout(() => setCopied(false), 1800);
  }

  return (
    <>
      <pre className="add-code">{oneLiner}</pre>
      <button className="ch-btn" onClick={copy}>
        {copied ? "Copied" : "Copy command"}
      </button>
      <p className="ch-meta" style={{ marginTop: "0.75rem" }}>
        This command works for 24 hours and only once.
      </p>
    </>
  );
}
