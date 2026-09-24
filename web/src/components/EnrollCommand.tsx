import { CopyField } from "./CopyField";

/** The one-line install for a computer. The token rides in an environment
 * variable, so it stays out of argv; the installer asks which login is whose
 * when the computer has several. */
export function installCommand(token: string, origin = window.location.origin): string {
  return `curl -fsSL ${origin}/install.sh | sudo OST_TOKEN=${token} sh -s -- --server ${origin}`;
}

export function EnrollCommand({ token }: { token: string }) {
  return (
    <div className="enroll">
      <CopyField value={installCommand(token)} label="Copy command" />
      <p className="hint">It works once, within 24 hours. Linux only for now.</p>
    </div>
  );
}
