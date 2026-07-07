import { Link } from "react-router-dom";
import type { Device } from "../types";
import { StatusLed, statusTone } from "./StatusLed";
import { Button } from "./Button";
import { relTime } from "../lib/format";

interface Props {
  device: Device;
  onLock?: (d: Device) => void;
  onUnlock?: (d: Device) => void;
  onSsh?: (d: Device) => void;
}

// name + StatusLed + last-seen + per-user chips + quick actions (lock/ssh).
export function DeviceCard({ device, onLock, onUnlock, onSsh }: Props) {
  const tone = statusTone(device.status);
  const users = device.users ?? [];
  const isLocked = device.status === "locked";
  const isPending = device.status === "pending";

  return (
    <article
      className="bg-surface hairline rounded flex flex-col"
      style={{ borderColor: isLocked ? "var(--accent-dim)" : "var(--line)" }}
    >
      <div className="p-4 flex flex-col gap-3 flex-1">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <Link
              to={`/devices/${device.id}`}
              className="dot text-sm text-fg hover:text-accent transition-colors truncate block focusable"
            >
              {device.name}
            </Link>
            <p className="text-[0.625rem] mt-1" style={{ color: "var(--fg-faint)" }}>
              {device.hostname} · {device.os} · v{device.agent_version}
            </p>
          </div>
          <StatusLed
            tone={tone}
            label={device.status}
            pulse={device.status === "online" || isLocked}
          />
        </div>

        <div className="flex items-center gap-2 flex-wrap">
          {device.tamper_level === 3 && (
            <span
              className="label border rounded px-1.5 py-0.5"
              style={{ color: "var(--accent)", borderColor: "var(--accent-dim)" }}
            >
              TAMPER L3
            </span>
          )}
          <span className="label" style={{ color: "var(--fg-faint)" }}>
            SEEN {relTime(device.last_seen)}
          </span>
        </div>

        {users.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {users.map((u) => (
              <span
                key={u.id}
                className="inline-flex items-center gap-1.5 border rounded px-2 py-0.5 text-[0.625rem] font-mono"
                style={{ borderColor: "var(--line)", background: "var(--surface-2)" }}
              >
                <span className="led" style={{ width: 5, height: 5, background: "var(--fg-faint)" }} />
                {u.display_name ?? u.os_username}
              </span>
            ))}
          </div>
        )}
      </div>

      <div
        className="flex items-center gap-2 px-4 py-2.5 border-t"
        style={{ borderColor: "var(--line)" }}
      >
        {isPending ? (
          <Link to={`/devices/${device.id}`} className="w-full">
            <Button size="sm" variant="primary" className="w-full">
              AWAITING ENROLL
            </Button>
          </Link>
        ) : (
          <>
            {isLocked ? (
              <Button size="sm" variant="primary" onClick={() => onUnlock?.(device)}>
                UNLOCK
              </Button>
            ) : (
              <Button size="sm" variant="danger" onClick={() => onLock?.(device)}>
                LOCK
              </Button>
            )}
            <Button size="sm" variant="ghost" onClick={() => onSsh?.(device)}>
              SSH
            </Button>
            <Link to={`/devices/${device.id}`} className="ml-auto">
              <Button size="sm" variant="ghost">
                OPEN →
              </Button>
            </Link>
          </>
        )}
      </div>
    </article>
  );
}
