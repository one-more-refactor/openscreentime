import { CopyField } from "./CopyField";

/** The one-line install for a computer. It downloads with wget where there is
 * one (stock Debian and Ubuntu have wget and no curl — acceptance round 4),
 * else curl (Arch). wget is quiet either way, so a computer without it says
 * nothing about it, and curl names its own errors. With neither, sh is handed
 * `exit 1`: the line fails out loud ("curl: command not found") instead of
 * running an empty script that "succeeds". The token rides in an environment
 * variable, so it stays out of argv; the installer asks which login is whose
 * when the computer has several. A console on plain http (trying it out at
 * home) gets `--insecure-http`, without which the installer refuses. */
export function installCommand(token: string, origin = window.location.origin): string {
  const insecure = origin.startsWith("http://") ? " --insecure-http" : "";
  const script = `${origin}/install.sh`;
  return (
    `(wget -qO- ${script} 2>/dev/null || curl -fsSL ${script} || echo exit 1) | ` +
    `sudo OST_TOKEN=${token} sh -s -- --server ${origin}${insecure}`
  );
}

export function EnrollCommand({ token, origin = window.location.origin }: { token: string; origin?: string }) {
  return (
    <div className="enroll">
      <CopyField value={installCommand(token, origin)} label="Copy command" />
      <p className="hint">It works once, within 24 hours. Linux only for now.</p>
      {origin.startsWith("http://") && (
        <p className="hint" data-error="true">
          This console isn't on https, so the command says <code>--insecure-http</code>: its token and the
          download travel unencrypted. Fine for trying it out on your own network, not across the internet.
        </p>
      )}
    </div>
  );
}
