import { CopyField } from "./CopyField";

/** The one-line install for a computer. It downloads with wget where there is
 * one (stock Debian and Ubuntu have wget and no curl — acceptance round 4),
 * else curl (Arch). Both are quiet (`2>/dev/null` on the download, so a
 * computer without one of them never says "command not found"). When neither
 * download works — no downloader, the server down, a typo in the address —
 * sh is handed a two-command script instead: one clear sentence naming the
 * server, then `exit 1`, never an empty script that "succeeds" (acceptance
 * round 5: the only word was "curl: command not found" on a computer that
 * had wget). The token rides in an environment variable, so it stays out of
 * argv; the installer asks which login is whose when the computer has
 * several. A console on plain http (trying it out at home) gets
 * `--insecure-http`, without which the installer refuses. */
export function installCommand(token: string, origin = window.location.origin): string {
  const insecure = origin.startsWith("http://") ? " --insecure-http" : "";
  const script = `${origin}/install.sh`;
  const failed = `echo \\"Couldn't download the installer from ${origin} — is the address right and the server up?\\" >&2; exit 1`;
  return (
    `(wget -qO- ${script} || curl -fsSL ${script} || echo "${failed}") 2>/dev/null | ` +
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
