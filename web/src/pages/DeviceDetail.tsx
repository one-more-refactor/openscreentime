import { useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  assignProfile,
  creditTime,
  deleteDevice,
  getDevice,
  listDeviceUsers,
  listProfiles,
  lockDevice,
  regenEnrollToken,
  removeDeviceVpn,
  setDeviceVpn,
  unlockDevice,
  updateDevice,
} from "../api";
import type {
  DeviceDetail as DeviceDetailT,
  DeviceUser,
  EnrollTokenResponse,
  Profile,
  TamperLevel,
  VpnKind,
} from "../types";
import { useAsync } from "../lib/useAsync";
import { useToast, errMsg } from "../lib/toast";
import { PageHeader } from "../layout/Shell";
import {
  Button,
  EnrollCommand,
  ErrorPanel,
  EventFeed,
  Modal,
  Panel,
  Select,
  SshTerminal,
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

  const [confirmL3, setConfirmL3] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [ssh, setSsh] = useState<{ id: string; name: string } | null>(null);
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
            <Button
              variant="ghost"
              disabled={d.status !== "online" && d.status !== "locked"}
              title={d.status === "offline" || d.status === "pending" ? "Device is offline" : undefined}
              onClick={() => setSsh({ id: d.id, name: d.name })}
            >
              SHELL
            </Button>
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

          <VpnPanel
            vpn={d.vpn ?? null}
            onUpload={async (kind, config) => {
              const dev = await setDeviceVpn(d.id, kind, config);
              device.setData((p) => (p ? { ...p, vpn: dev.vpn } : p));
            }}
            onRemove={async () => {
              const dev = await removeDeviceVpn(d.id);
              device.setData((p) => (p ? { ...p, vpn: dev.vpn } : p));
            }}
          />

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

        {/* Right column: events */}
        <div className="flex flex-col gap-6">
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

      <SshTerminal target={ssh} onClose={() => setSsh(null)} />
    </>
  );
}

/** Sniff whether an uploaded file is a WireGuard or OpenVPN client config.
 * Filename is only a hint; the content decides. */
function sniffVpnKind(name: string, text: string): VpnKind | null {
  if (text.includes("[Interface]")) return "wireguard";
  if (/^\s*(remote\s+\S+|client\s*$)/m.test(text)) return "openvpn";
  if (name.endsWith(".ovpn")) return "openvpn";
  return null;
}

/** VPN profile: a drop/browse upload field for a wg/ovpn client config. The
 * server never echoes the config back — the panel shows presence only. */
function VpnPanel({
  vpn,
  onUpload,
  onRemove,
}: {
  vpn: { kind: VpnKind; updated_at: string | null } | null;
  onUpload: (kind: VpnKind, config: string) => Promise<void>;
  onRemove: () => Promise<void>;
}) {
  const { toast } = useToast();
  const [busy, setBusy] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  async function handleFile(file: File | undefined) {
    if (!file || busy) return;
    setBusy(true);
    try {
      const text = await file.text();
      const kind = sniffVpnKind(file.name, text);
      if (!kind) {
        toast(
          "That doesn't look like a WireGuard (.conf) or OpenVPN (.ovpn) client config.",
          "warn",
        );
        return;
      }
      await onUpload(kind, text);
      toast(`${kind.toUpperCase()} PROFILE SET — APPLIES ON NEXT AGENT SYNC`, "ok");
    } catch (e) {
      toast(errMsg(e, "Couldn't upload the VPN config — try again."));
    } finally {
      setBusy(false);
    }
  }

  async function handleRemove() {
    setBusy(true);
    try {
      await onRemove();
      toast("VPN PROFILE REMOVED — TUNNEL STOPS ON NEXT AGENT SYNC", "ok");
    } catch (e) {
      toast(errMsg(e, "Couldn't remove the VPN profile — try again."));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="VPN PROFILE" refCode="VP-01">
      <div className="flex flex-col gap-3">
        {vpn ? (
          <div className="flex items-center gap-4 flex-wrap">
            <StatusLed tone="ok" label={vpn.kind.toUpperCase()} />
            <span className="label" style={{ color: "var(--fg-faint)" }}>
              UPLOADED {vpn.updated_at ? relTime(vpn.updated_at) : "—"}
            </span>
            <span className="flex-1" />
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => void handleRemove()}>
              {busy ? "…" : "REMOVE"}
            </Button>
          </div>
        ) : (
          <p className="label" style={{ color: "var(--fg-faint)" }}>
            NO VPN PROFILE
          </p>
        )}
        <div
          role="button"
          tabIndex={0}
          aria-label="Upload a WireGuard or OpenVPN client config"
          className="border border-dashed rounded px-4 py-6 text-center cursor-pointer outline-none"
          style={{
            borderColor: dragOver ? "var(--fg)" : "var(--line)",
            background: dragOver ? "var(--panel-2, transparent)" : "transparent",
          }}
          onClick={() => fileInput.current?.click()}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") fileInput.current?.click();
          }}
          onDragOver={(e) => {
            e.preventDefault();
            setDragOver(true);
          }}
          onDragLeave={() => setDragOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setDragOver(false);
            void handleFile(e.dataTransfer.files?.[0]);
          }}
        >
          <p className="dot text-xs text-fg">
            {busy ? "UPLOADING…" : vpn ? "DROP A NEW CONFIG TO REPLACE" : "DROP CONFIG HERE"}
          </p>
          <p className="text-[0.625rem] mt-1" style={{ color: "var(--fg-faint)" }}>
            .conf (WireGuard) or .ovpn (OpenVPN) — or click to browse
          </p>
          <input
            ref={fileInput}
            type="file"
            accept=".conf,.ovpn,.txt"
            className="hidden"
            onChange={(e) => {
              void handleFile(e.target.files?.[0]);
              e.target.value = "";
            }}
          />
        </div>
        <p className="text-[0.6875rem] leading-relaxed" style={{ color: "var(--fg-faint)" }}>
          The device routes its traffic through this tunnel (wg-quick / openvpn-client must be
          installed there). The config — including its private key — is sent once to the device
          and never shown here again. The firewall automatically lets the tunnel through, even
          with VPN-blocking lockdown on.
        </p>
      </div>
    </Panel>
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
