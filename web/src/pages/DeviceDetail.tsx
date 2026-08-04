import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  assignProfile,
  cancelCommand,
  listCommands,
  creditTime,
  deleteDevice,
  getDevice,
  listDeviceUsers,
  listProfiles,
  lockDevice,
  regenEnrollToken,
  unlockDevice,
  updateDevice,
} from "../api";
import type {
  DeviceDetail as DeviceDetailT,
  DeviceUser,
  EnrollTokenResponse,
  Profile,
  TamperLevel,
} from "../types";
import { useAsync } from "../lib/useAsync";
import type { CommandRow } from "../types";
import { useToast, errMsg } from "../lib/toast";
import { PageHeader } from "../layout/Shell";
import {
  Button,
  EnrollCommand,
  ErrorPanel,
  EventFeed,
  UsageHistory,
  VpnProfiles,
  Modal,
  Panel,
  Select,
  StatusLed,
  statusTone,
  Toggle,
} from "../components";
import { Empty, Loading } from "./Devices";
import { minutesToHm, relTime } from "../lib/format";

export function DeviceDetail() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const { toast } = useToast();
  const device = useAsync<DeviceDetailT>(() => getDevice(id), [id]);
  const profiles = useAsync<Profile[]>(listProfiles, []);
  // Per-user used/earned minutes today (contract §5).
  const usage = useAsync<DeviceUser[]>(() => listDeviceUsers(id), [id]);
  const queue = useAsync<CommandRow[]>(() => listCommands(id), [id]);
  // The queue is live state — poll it while any command is pending.
  useEffect(() => {
    const pending = (queue.data ?? []).some(
      (c) => c.status === "queued" || c.status === "sent",
    );
    if (!pending) return;
    const t = setInterval(queue.reload, 8000);
    return () => clearInterval(t);
  }, [queue.data, queue.reload]);

  const [confirmL3, setConfirmL3] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [lockBusy, setLockBusy] = useState(false);
  const [lockPending, setLockPending] = useState(false);
  const [tamperBusy, setTamperBusy] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [assignBusy, setAssignBusy] = useState<string | null>(null);
  const [grantBusy, setGrantBusy] = useState<string | null>(null);
  const [enroll, setEnroll] = useState<EnrollTokenResponse | null>(null);
  const [enrollBusy, setEnrollBusy] = useState(false);

  const d = device.data;
  const profileList = profiles.data ?? [];
  const profileName = (pid: string) =>
    profileList.find((p) => p.id === pid)?.name ?? "—";
  const usageFor = (userId: string) => usage.data?.find((u) => u.id === userId);

  if (device.loading) return <Loading />;
  if (device.error)
    return (
      <ErrorPanel
        title="Couldn't load this device"
        detail={device.error}
        onRetry={device.reload}
      />
    );
  if (!d) return <Empty label="DEVICE NOT FOUND" />;

  async function setTamper(level: TamperLevel) {
    if (!d) return;
    const prev = d.tamper_level;
    setTamperBusy(true);
    device.setData((p) => (p ? { ...p, tamper_level: level } : p));
    try {
      await updateDevice(d.id, { tamper_level: level });
    } catch (e) {
      device.setData((p) => (p ? { ...p, tamper_level: prev } : p));
      toast(errMsg(e, "Couldn't change the tamper level — try again."));
    } finally {
      setTamperBusy(false);
    }
  }

  async function onLockToggle() {
    if (!d) return;
    const wasLocked = d.status === "locked";
    const prevStatus = d.status;
    setLockBusy(true);
    device.setData((p) => (p ? { ...p, status: wasLocked ? "online" : "locked" } : p));
    try {
      const res = await (wasLocked ? unlockDevice(d.id) : lockDevice(d.id));
      if (res.delivered) {
        setLockPending(false);
      } else {
        // Truthful lock state: the agent is offline — the command is only
        // queued, so keep the real status and flag the pending lock.
        device.setData((p) => (p ? { ...p, status: prevStatus } : p));
        setLockPending(!wasLocked);
        toast(
          `${wasLocked ? "UNLOCK" : "LOCK"} QUEUED — APPLIES WHEN DEVICE RECONNECTS`,
          "warn",
        );
      }
    } catch (e) {
      device.setData((p) => (p ? { ...p, status: prevStatus } : p));
      toast(errMsg(e, `Couldn't ${wasLocked ? "unlock" : "lock"} ${d.name} — try again.`));
    } finally {
      setLockBusy(false);
    }
  }

  async function onGrantTime(user: DeviceUser, minutes: number) {
    setGrantBusy(user.id);
    try {
      await creditTime(user.id, minutes);
      const who = (user.display_name ?? user.os_username).toUpperCase();
      toast(`+${minutes} MIN GRANTED TO ${who} — APPLIES WITHIN ~10S`, "ok");
      usage.reload();
    } catch (e) {
      toast(errMsg(e, "Couldn't grant extra time — try again."));
    } finally {
      setGrantBusy(null);
    }
  }

  async function onShowEnroll() {
    if (!d) return;
    setEnrollBusy(true);
    try {
      setEnroll(await regenEnrollToken(d.id));
    } catch (e) {
      toast(errMsg(e, "Couldn't generate an enroll token — try again."));
    } finally {
      setEnrollBusy(false);
    }
  }

  async function onAssign(user: DeviceUser, pid: string) {
    if (!d) return;
    const prevPid = user.profile_id;
    setAssignBusy(user.id);
    device.setData((p) =>
      p
        ? {
            ...p,
            users: p.users.map((x) => (x.id === user.id ? { ...x, profile_id: pid } : x)),
          }
        : p,
    );
    try {
      await assignProfile(user.id, pid);
    } catch (e) {
      device.setData((p) =>
        p
          ? {
              ...p,
              users: p.users.map((x) =>
                x.id === user.id ? { ...x, profile_id: prevPid } : x,
              ),
            }
          : p,
      );
      toast(errMsg(e, "Couldn't assign the profile — try again."));
    } finally {
      setAssignBusy(null);
    }
  }

  async function onDelete() {
    if (!d) return;
    setDeleting(true);
    try {
      await deleteDevice(d.id);
      toast(`${d.name} removed.`, "ok");
      navigate("/devices", { replace: true });
    } catch (e) {
      toast(errMsg(e, `Couldn't remove ${d.name} — try again.`));
      setDeleting(false);
    }
  }

  return (
    <>
      <PageHeader
        title={d.name.toUpperCase()}
        stat={
          <span className="inline-flex items-center gap-3">
            <StatusLed tone={statusTone(d.status)} label={d.status} pulse />
            {lockPending && d.status !== "locked" && (
              <span
                className="label border rounded px-1.5 py-0.5"
                style={{ color: "var(--warn)", borderColor: "var(--warn)" }}
              >
                LOCK PENDING
              </span>
            )}
          </span>
        }
        actions={
          <>
            <Button variant="ghost" onClick={() => navigate("/devices")}>
              ← BACK
            </Button>
            {d.status === "pending" && (
              <Button
                variant="primary"
                disabled={enrollBusy}
                onClick={() => void onShowEnroll()}
              >
                {enrollBusy ? "GENERATING…" : "SHOW ENROLL COMMAND"}
              </Button>
            )}
            <Button variant="ghost" onClick={() => setConfirmDelete(true)}>
              REMOVE
            </Button>
            <Button
              variant={d.status === "locked" ? "primary" : "danger"}
              disabled={lockBusy}
              onClick={() => void onLockToggle()}
            >
              {lockBusy ? "…" : d.status === "locked" ? "UNLOCK" : "LOCK"}
            </Button>
          </>
        }
      />

      <div className="grid lg:grid-cols-3 gap-6">
        {/* Left column: identity + users + tamper */}
        <div className="lg:col-span-2 flex flex-col gap-6">
          <Panel title="IDENTITY" refCode="ID-01">
            <dl className="grid sm:grid-cols-2 gap-x-8 gap-y-3">
              <Field k="HOSTNAME" v={d.hostname} />
              <Field k="OS" v={d.os} />
              <Field k="AGENT" v={`v${d.agent_version}`} />
              <Field k="PUBLIC IP" v={d.public_ip ?? "—"} />
              <Field k="LAST SEEN" v={relTime(d.last_seen)} />
              <Field k="TAMPER LEVEL" v={`L${d.tamper_level}`} accent={d.tamper_level === 3} />
            </dl>
          </Panel>

          <Panel title="USERS · SCREEN TIME TODAY" refCode="US-01">
            {d.users.length === 0 ? (
              <Empty label="NO OS USERS REPORTED YET" />
            ) : (
              <ul className="flex flex-col">
                {d.users.map((u) => {
                  const stats = usageFor(u.id);
                  return (
                    <li
                      key={u.id}
                      className="flex flex-col gap-2 py-3 border-b last:border-b-0"
                      style={{ borderColor: "var(--line)" }}
                    >
                      <div className="flex items-center gap-4 flex-wrap">
                        <div className="min-w-0 flex-1">
                          <p className="dot text-xs text-fg">
                            {u.display_name ?? u.os_username}
                          </p>
                          <p className="text-[0.625rem]" style={{ color: "var(--fg-faint)" }}>
                            {u.os_username}
                          </p>
                        </div>
                        <Select
                          className="w-48"
                          value={u.profile_id}
                          disabled={assignBusy === u.id}
                          aria-label={`Profile for ${u.display_name ?? u.os_username}`}
                          onChange={(e) => void onAssign(u, e.target.value)}
                        >
                          {profileList.map((p) => (
                            <option key={p.id} value={p.id}>
                              {p.name.toUpperCase()}
                            </option>
                          ))}
                        </Select>
                      </div>
                      <UsageBar
                        used={stats?.used_minutes_today}
                        earned={stats?.earned_minutes_today}
                        loading={usage.loading}
                      />
                      <details>
                        <summary className="label text-muted cursor-pointer select-none">
                          HISTORY
                        </summary>
                        <UsageHistory deviceUserId={u.id} />
                      </details>
                      <div className="flex items-center gap-2 flex-wrap">
                        <span className="label" style={{ color: "var(--fg-faint)" }}>
                          GRANT TIME
                        </span>
                        {[15, 30, 60].map((m) => (
                          <Button
                            key={m}
                            size="sm"
                            variant="ghost"
                            className="font-mono tabular-nums"
                            disabled={grantBusy === u.id}
                            onClick={() => void onGrantTime(u, m)}
                          >
                            +{m}
                          </Button>
                        ))}
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
            {usage.error && (
              <p className="text-[0.6875rem] mt-3" style={{ color: "var(--warn)" }}>
                Usage data unavailable right now — {usage.error}
              </p>
            )}
            <p className="label mt-3" style={{ color: "var(--fg-faint)" }}>
              CURRENT: {d.users.map((u) => profileName(u.profile_id)).join(" · ") || "—"}
            </p>
          </Panel>

          <VpnProfiles deviceId={d.id} />

          <Panel title="TAMPER RESISTANCE" refCode="TR-01">
            <div className="flex flex-col gap-4">
              <Toggle
                label="LEVEL 3 — MAXIMUM LOCKDOWN"
                hint="disable TTY switching, lock systemd unit, physical-mitigation guidance"
                danger
                disabled={tamperBusy}
                checked={d.tamper_level === 3}
                onChange={(v) => {
                  if (v) setConfirmL3(true);
                  else void setTamper(1);
                }}
              />
              <p className="text-[0.6875rem] leading-relaxed" style={{ color: "var(--fg-faint)" }}>
                Level 1 (default): hardened root service, watchdog, boot persistence, tamper
                alerting. Level 3 adds lockdown that can lock the admin out too — the{" "}
                <span className="text-fg">sentinel-admin</span> recovery path always remains.
              </p>
            </div>
          </Panel>
        </div>

        {/* Right column: queue + events */}
        <div className="flex flex-col gap-6">
          <Panel title="COMMAND QUEUE" refCode="CQ-01">
            <CommandQueue
              rows={queue.data ?? []}
              onCancel={async (cmdId) => {
                try {
                  await cancelCommand(cmdId);
                } catch {
                  // Raced the ack — the reload below shows the truth either way.
                }
                queue.reload();
                device.reload();
              }}
            />
          </Panel>
          <Panel title="RECENT EVENTS" refCode="EV-01">
            <EventFeed events={d.recent_events} />
            <Link to={`/events?device_id=${d.id}`}>
              <Button variant="ghost" size="sm" className="mt-3 w-full">
                VIEW ALL →
              </Button>
            </Link>
          </Panel>
        </div>
      </div>

      {/* Level 3 confirm (TAMPER.md — explicit confirm + recovery procedure) */}
      <Modal
        open={confirmL3}
        onClose={() => setConfirmL3(false)}
        title="ENABLE LEVEL 3 LOCKDOWN"
        danger
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmL3(false)}>
              CANCEL
            </Button>
            <Button
              variant="danger"
              disabled={tamperBusy}
              onClick={async () => {
                await setTamper(3);
                setConfirmL3(false);
              }}
            >
              I UNDERSTAND — ENABLE L3
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          <div
            className="flex items-start gap-3 border rounded px-3 py-2.5"
            style={{ borderColor: "var(--accent)" }}
          >
            <span className="led led-glow-crit led-pulse" style={{ background: "var(--accent)" }} />
            <p className="text-xs" style={{ color: "var(--accent)" }}>
              DANGER: LEVEL 3 CAN LOCK THE ADMIN OUT TOO.
            </p>
          </div>
          <p className="text-xs leading-relaxed" style={{ color: "var(--fg-dim)" }}>
            Level 3 disables extra TTYs / Ctrl+Alt+F* switching, locks the systemd unit against
            user <span className="text-fg">systemctl stop</span>, and kills known escape hatches
            for managed users.
          </p>
          <div>
            <p className="label mb-2">RECOVERY PROCEDURE</p>
            <ol className="text-[0.6875rem] leading-relaxed flex flex-col gap-1" style={{ color: "var(--fg-dim)" }}>
              <li>1 · Use the <span className="text-fg">sentinel-admin unlock</span> token (always works).</li>
              <li>2 · Set a GRUB / BIOS admin password to complete physical hardening.</li>
              <li>3 · Lower back to L1 here at any time to restore normal power controls.</li>
            </ol>
          </div>
        </div>
      </Modal>

      {/* Delete confirm */}
      <Modal
        open={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title="REMOVE DEVICE"
        danger
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmDelete(false)}>
              CANCEL
            </Button>
            <Button variant="danger" disabled={deleting} onClick={() => void onDelete()}>
              {deleting ? "REMOVING…" : "REMOVE DEVICE"}
            </Button>
          </>
        }
      >
        <p className="text-xs leading-relaxed" style={{ color: "var(--fg-dim)" }}>
          This removes <span className="dot text-fg">{d.name}</span> and its users, policies
          and history from the control center. The agent on the machine keeps running until
          it is uninstalled. This cannot be undone.
        </p>
      </Modal>

      {/* Enroll command (pending devices — token regenerated with a fresh 24 h TTL) */}
      <Modal
        open={!!enroll}
        onClose={() => setEnroll(null)}
        title="ENROLLMENT TOKEN"
        footer={
          <Button variant="primary" onClick={() => setEnroll(null)}>
            DONE
          </Button>
        }
      >
        {enroll && (
          <div className="flex flex-col gap-4">
            <p className="text-xs" style={{ color: "var(--fg-dim)" }}>
              Device <span className="dot text-fg">{enroll.device.name}</span> is{" "}
              <StatusLed tone="pending" label="PENDING" className="align-middle" />. Install
              and enroll the agent with this single-use command (token valid 24 h):
            </p>
            <EnrollCommand token={enroll.enroll_token} />
          </div>
        )}
      </Modal>

    </>
  );
}

// Small stacked usage bar: used minutes (fg) + earned minutes (ok), today.
function UsageBar({
  used,
  earned,
  loading,
}: {
  used?: number;
  earned?: number;
  loading: boolean;
}) {
  if (loading && used === undefined) {
    return <span className="label" style={{ color: "var(--fg-faint)" }}>USAGE…</span>;
  }
  if (used === undefined && earned === undefined) {
    return (
      <span className="label" style={{ color: "var(--fg-faint)" }}>
        NO USAGE DATA TODAY
      </span>
    );
  }
  const u = used ?? 0;
  const e = earned ?? 0;
  const scale = Math.max(u + e, 120); // bar spans at least 2h so small values stay readable
  return (
    <div className="flex items-center gap-3">
      <div
        className="flex-1 h-1.5 rounded-[1px] overflow-hidden flex"
        style={{ background: "var(--line)" }}
        role="img"
        aria-label={`Used ${minutesToHm(u)} today, earned ${minutesToHm(e)}`}
      >
        <span style={{ width: `${(u / scale) * 100}%`, background: "var(--fg-dim)" }} />
        <span style={{ width: `${(e / scale) * 100}%`, background: "var(--ok)" }} />
      </div>
      <span className="label tabular-nums flex-none" style={{ color: "var(--fg-dim)" }}>
        {minutesToHm(u)} USED{e > 0 ? ` · +${minutesToHm(e)} EARNED` : ""}
      </span>
    </div>
  );
}

function Field({ k, v, accent }: { k: string; v: string; accent?: boolean }) {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="label">{k}</dt>
      <dd className="dot text-xs" style={{ color: accent ? "var(--accent)" : "var(--fg)" }}>
        {v}
      </dd>
    </div>
  );
}

/** The device's command queue: pending rows with CANCEL, settled rows dimmed.
 *  Server-backed — survives reloads, unlike the old optimistic-only chips. */
function CommandQueue({
  rows,
  onCancel,
}: {
  rows: CommandRow[];
  onCancel: (id: string) => void;
}) {
  const pending = rows.filter((c) => c.status === "queued" || c.status === "sent");
  const settled = rows.filter((c) => c.status !== "queued" && c.status !== "sent").slice(0, 8);
  if (rows.length === 0) {
    return <p className="label text-muted">NO COMMANDS YET</p>;
  }
  const age = (iso: string) => {
    const mins = Math.max(0, Math.round((Date.now() - new Date(iso).getTime()) / 60000));
    if (mins < 1) return "JUST NOW";
    if (mins < 60) return `${mins}M AGO`;
    const h = Math.round(mins / 60);
    return h < 48 ? `${h}H AGO` : `${Math.round(h / 24)}D AGO`;
  };
  return (
    <div className="flex flex-col gap-2">
      {pending.length === 0 && <p className="label text-muted">NOTHING PENDING</p>}
      {pending.map((c) => (
        <div key={c.id} className="flex items-center gap-2 border rounded px-2 py-1.5 hairline">
          <span className="label" style={{ color: "var(--warn)" }}>
            {c.type.replace(/_/g, " ").toUpperCase()}
          </span>
          <span className="label text-muted">
            {c.status === "sent" ? "SENT" : "QUEUED"} · {age(c.created_at)}
          </span>
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto"
            onClick={() => onCancel(c.id)}
          >
            CANCEL
          </Button>
        </div>
      ))}
      {settled.length > 0 && (
        <details>
          <summary className="label text-muted cursor-pointer select-none">
            RECENT ({settled.length})
          </summary>
          <div className="mt-2 flex flex-col gap-1">
            {settled.map((c) => (
              <div key={c.id} className="flex items-center gap-2 px-2 py-1 opacity-70">
                <span className="label">{c.type.replace(/_/g, " ").toUpperCase()}</span>
                <span className="label text-muted ml-auto">
                  {c.status.toUpperCase()} · {age(c.created_at)}
                </span>
              </div>
            ))}
          </div>
        </details>
      )}
    </div>
  );
}
