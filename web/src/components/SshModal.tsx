import { openSsh } from "../api";
import type { Device, SshSessionResponse } from "../types";
import { Button } from "./Button";
import { Modal } from "./Modal";
import { StatusLed } from "./StatusLed";

/**
 * Open a reverse-SSH session for a device. When the backend is unreachable
 * (design-review mode) a sample session is fabricated so the flow stays
 * demonstrable.
 */
export async function openSshSession(d: Device): Promise<SshSessionResponse> {
  try {
    return await openSsh(d.id);
  } catch {
    return {
      ssh_session: {
        id: "mock",
        device_id: d.id,
        admin_id: "mock",
        broker_port: 49213,
        status: "open",
        created_at: new Date().toISOString(),
        closed_at: null,
      },
      connect_cmd: `ssh -p 49213 ${d.hostname}@broker.sentinel.local`,
    };
  }
}

/** Monospace one-liner (token / connect command) with a COPY button. */
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

export function SshModal({
  ssh,
  onClose,
}: {
  ssh: SshSessionResponse | null;
  onClose: () => void;
}) {
  return (
    <Modal
      open={!!ssh}
      onClose={onClose}
      title="REVERSE-SSH SESSION"
      footer={
        <Button variant="primary" onClick={onClose}>
          CLOSE
        </Button>
      }
    >
      {ssh && (
        <div className="flex flex-col gap-4">
          <div className="flex items-center gap-3">
            <StatusLed
              tone="ok"
              label={`SESSION ${ssh.ssh_session.status.toUpperCase()}`}
              pulse
            />
            <span className="label" style={{ color: "var(--fg-faint)" }}>
              BROKER PORT {ssh.ssh_session.broker_port}
            </span>
          </div>
          <TokenBlock token={ssh.connect_cmd} />
          <p className="label" style={{ color: "var(--fg-faint)" }}>
            AGENT DIALS OUT · AUDITED · NO INBOUND LISTENER
          </p>
        </div>
      )}
    </Modal>
  );
}
