import { Link } from "react-router-dom";
import type { Device } from "../types";
import { StatusLed, statusTone } from "./StatusLed";
import { Button } from "./Button";
import { goneDarkDays, relTime } from "../lib/format";

interface Props {
  device: Device;
  /** silkscreen ref-code, e.g. "DV-01" */
  refCode?: string;
  /** a mutation for this device is in flight — disable actions */
  busy?: boolean;
  /** a lock command is queued but not yet applied (device was offline) */
  lockPending?: boolean;
  unlockPending?: boolean;
  onLock?: (d: Device) => void;
  onUnlock?: (d: Device) => void;
  onDelete?: (d: Device) => void;
}

// Hardware module per device: registration ticks, silkscreen ref-code,
// status LED, user chips, quick actions.
export function DeviceCard({
  device,
  refCode,
  busy,
  lockPending,
  unlockPending,
  onLock,
  onUnlock,
  onDelete,
}: Props) {
  const tone = statusTone(device.status);
  const users = device.users ?? [];
  const isLocked = device.status === "locked";
  const isPending = device.status === "pending";
  // Tamper signal: offline for 7+ days = the agent has probably been silenced.
  const darkDays = goneDarkDays(device.status, device.last_seen);

  return (
    <article
      className="relative bg-surface hairline rounded flex flex-col"
      style={{ borderColor: isLocked ? "var(--accent-dim)" : "var(--line)" }}
    >
      <span className="tick tick-tl" />
      <span className="tick tick-tr" />
      <span className="tick tick-bl" />
      <span className="tick tick-br" />
      <div className="p-4 flex flex-col gap-3 flex-1">
        <div className="flex items-center justify-between gap-3">
          <StatusLed
            tone={tone}
            label={device.status}
            pulse={device.status === "online" || isLocked}
          />
          {refCode && <span className="ref">{refCode}</span>}
        </div>
        <div className="min-w-0">
          <Link
            to={`/devices/${device.id}`}
            className="dot text-sm text-fg hover:text-accent transition-colors truncate block focusable"
          >
            {device.name.toUpperCase()}
          </Link>
          <p className="text-[0.625rem] mt-1" style={{ color: "var(--fg-faint)" }}>
            {device.hostname} · {device.os} · v{device.agent_version}
          </p>
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
          {lockPending && (
            <span
              className="label border rounded px-1.5 py-0.5"
              style={{ color: "var(--warn)", borderColor: "var(--warn)" }}
            >
              LOCK PENDING
            </span>
          )}
          {unlockPending && (
            <span
              className="label border rounded px-1.5 py-0.5"
              style={{ color: "var(--warn)", borderColor: "var(--warn)" }}
            >
              UNLOCK PENDING
            </span>
          )}
          {darkDays !== null ? (
            <span className="label" style={{ color: "var(--accent)" }}>
              GONE DARK {darkDays}d
            </span>
          ) : (
            <span className="label" style={{ color: "var(--fg-faint)" }}>
              SEEN {relTime(device.last_seen)}
            </span>
          )}
        </div>

        {users.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {users.map((u) => (
              <span
                key={u.id}
                className="inline-flex items-center gap-1.5 border rounded px-2 py-0.5 text-[0.625rem] font-mono"
                style={{ borderColor: "var(--line-2)", color: "var(--fg-dim)" }}
              >
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
          <>
            <Link to={`/devices/${device.id}`} className="flex-1">
              <Button size="sm" variant="primary" className="w-full">
                AWAITING ENROLL
              </Button>
            </Link>
            {onDelete && (
              <Button size="sm" variant="danger" onClick={() => onDelete(device)}>
                REMOVE
              </Button>
            )}
          </>
        ) : (
          <>
            {isLocked ? (
              <Button size="sm" variant="primary" disabled={busy} onClick={() => onUnlock?.(device)}>
                UNLOCK
              </Button>
            ) : (
              <Button size="sm" variant="danger" disabled={busy} onClick={() => onLock?.(device)}>
                LOCK
              </Button>
            )}
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
