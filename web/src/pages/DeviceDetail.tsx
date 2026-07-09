import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  assignProfile,
  getDevice,
  listProfiles,
  lockDevice,
  unlockDevice,
  updateDevice,
} from "../api";
import type { DeviceDetail as DeviceDetailT, Profile, SshSessionResponse, TamperLevel } from "../types";
import { useAsync } from "../lib/useAsync";
import { PageHeader } from "../layout/Shell";
import {
  Button,
  EventFeed,
  Modal,
  Panel,
  Select,
  SshModal,
  StatusLed,
  statusTone,
  Toggle,
  openSshSession,
} from "../components";
import { Empty, Loading } from "./Devices";
import { relTime } from "../lib/format";

export function DeviceDetail() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const device = useAsync<DeviceDetailT>(() => getDevice(id), [id]);
  const profiles = useAsync<Profile[]>(listProfiles, []);

  const [confirmL3, setConfirmL3] = useState(false);
  const [ssh, setSsh] = useState<SshSessionResponse | null>(null);

  const d = device.data;
  const profileList = profiles.data ?? [];
  const profileName = (pid: string) =>
    profileList.find((p) => p.id === pid)?.name ?? "—";

  if (device.loading) return <Loading />;
  if (!d) return <Empty label="DEVICE NOT FOUND" />;

  async function setTamper(level: TamperLevel) {
    if (!d) return;
    await updateDevice(d.id, { tamper_level: level }).catch(() => {});
    device.setData((prev) => (prev ? { ...prev, tamper_level: level } : prev));
  }

  async function onLockToggle() {
    if (!d) return;
    if (d.status === "locked") {
      await unlockDevice(d.id).catch(() => {});
      device.setData((prev) => (prev ? { ...prev, status: "online" } : prev));
    } else {
      await lockDevice(d.id).catch(() => {});
      device.setData((prev) => (prev ? { ...prev, status: "locked" } : prev));
    }
  }

  async function onSsh() {
    if (!d) return;
    setSsh(await openSshSession(d));
  }

  return (
    <>
      <PageHeader
        title={d.name.toUpperCase()}
        stat={<StatusLed tone={statusTone(d.status)} label={d.status} pulse />}
        actions={
          <>
            <Button variant="ghost" onClick={() => navigate("/devices")}>
              ← BACK
            </Button>
            <Button variant="ghost" onClick={onSsh}>
              SSH
            </Button>
            <Button
              variant={d.status === "locked" ? "primary" : "danger"}
              onClick={onLockToggle}
            >
              {d.status === "locked" ? "UNLOCK" : "LOCK"}
            </Button>
          </>
        }
      />

      <div className="grid lg:grid-cols-3 gap-6">
        {/* Left column: identity + users + tamper */}
        <div className="lg:col-span-2 flex flex-col gap-6">
          <Panel title="IDENTITY">
            <dl className="grid sm:grid-cols-2 gap-x-8 gap-y-3">
              <Field k="HOSTNAME" v={d.hostname} />
              <Field k="OS" v={d.os} />
              <Field k="AGENT" v={`v${d.agent_version}`} />
              <Field k="PUBLIC IP" v={d.public_ip ?? "—"} />
              <Field k="LAST SEEN" v={relTime(d.last_seen)} />
              <Field k="TAMPER LEVEL" v={`L${d.tamper_level}`} accent={d.tamper_level === 3} />
            </dl>
          </Panel>

          <Panel title="USERS · PER-PERSON POLICY">
            {d.users.length === 0 ? (
              <Empty label="NO OS USERS REPORTED YET" />
            ) : (
              <ul className="flex flex-col">
                {d.users.map((u) => (
                  <li
                    key={u.id}
                    className="flex items-center gap-4 py-3 border-b last:border-b-0 flex-wrap"
                    style={{ borderColor: "var(--line)" }}
                  >
                    <span className="led" style={{ width: 6, height: 6, background: "var(--fg-faint)" }} />
                    <div className="min-w-0 flex-1">
                      <p className="dot text-xs text-fg">{u.display_name ?? u.os_username}</p>
                      <p className="text-[0.625rem]" style={{ color: "var(--fg-faint)" }}>
                        {u.os_username}
                      </p>
                    </div>
                    <Select
                      className="w-48"
                      value={u.profile_id}
                      onChange={async (e) => {
                        const pid = e.target.value;
                        await assignProfile(u.id, pid).catch(() => {});
                        device.setData((prev) =>
                          prev
                            ? {
                                ...prev,
                                users: prev.users.map((x) =>
                                  x.id === u.id ? { ...x, profile_id: pid } : x,
                                ),
                              }
                            : prev,
                        );
                      }}
                    >
                      {profileList.map((p) => (
                        <option key={p.id} value={p.id}>
                          {p.name.toUpperCase()}
                        </option>
                      ))}
                    </Select>
                  </li>
                ))}
              </ul>
            )}
            <p className="label mt-3" style={{ color: "var(--fg-faint)" }}>
              CURRENT: {d.users.map((u) => profileName(u.profile_id)).join(" · ") || "—"}
            </p>
          </Panel>

          <Panel title="TAMPER RESISTANCE">
            <div className="flex flex-col gap-4">
              <Toggle
                label="LEVEL 3 — MAXIMUM LOCKDOWN"
                hint="disable TTY switching, lock systemd unit, physical-mitigation guidance"
                danger
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
          <Panel title="RECENT EVENTS">
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

      <SshModal ssh={ssh} onClose={() => setSsh(null)} />
    </>
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
