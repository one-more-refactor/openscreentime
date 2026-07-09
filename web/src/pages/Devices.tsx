import { useState } from "react";
import {
  createDevice,
  getDiscoveryResults,
  listDevices,
  lockDevice,
  scanDiscovery,
  unlockDevice,
} from "../api";
import type { Device, EnrollTokenResponse, SshSessionResponse } from "../types";
import { useAsync } from "../lib/useAsync";
import { PageHeader } from "../layout/Shell";
import {
  Button,
  DeviceCard,
  Modal,
  Panel,
  SshModal,
  Stat,
  StatusLed,
  TextInput,
  TokenBlock,
  openSshSession,
} from "../components";
import { pad2, relTime } from "../lib/format";

export function Devices() {
  const devices = useAsync<Device[]>(listDevices, []);
  const discovery = useAsync(getDiscoveryResults, []);

  const [addOpen, setAddOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [enroll, setEnroll] = useState<EnrollTokenResponse | null>(null);
  const [ssh, setSsh] = useState<SshSessionResponse | null>(null);
  const [busy, setBusy] = useState(false);

  const list = devices.data ?? [];
  const online = list.filter((d) => d.status === "online").length;
  const locked = list.filter((d) => d.status === "locked").length;

  async function handleAdd() {
    setBusy(true);
    try {
      const res = await createDevice(newName || "New Device");
      setEnroll(res);
      setNewName("");
      devices.reload();
    } catch (e) {
      alert(e instanceof Error ? e.message : "Failed to create device");
    } finally {
      setBusy(false);
    }
  }

  async function onLock(d: Device) {
    await lockDevice(d.id).catch(() => {});
    devices.setData((prev) =>
      (prev ?? []).map((x) => (x.id === d.id ? { ...x, status: "locked" } : x)),
    );
  }
  async function onUnlock(d: Device) {
    await unlockDevice(d.id).catch(() => {});
    devices.setData((prev) =>
      (prev ?? []).map((x) => (x.id === d.id ? { ...x, status: "online" } : x)),
    );
  }
  async function onSsh(d: Device) {
    setSsh(await openSshSession(d));
  }

  const latestScan = discovery.data?.[0];

  return (
    <>
      <PageHeader
        title="DEVICES"
        stat={
          <div className="flex items-end gap-6">
            <Stat value={pad2(list.length)} caption="TOTAL" size="lg" />
            <Stat value={pad2(online)} caption="ONLINE" size="md" />
            {locked > 0 && (
              <Stat value={pad2(locked)} caption="LOCKED" size="md" accent />
            )}
          </div>
        }
        actions={
          <Button variant="primary" onClick={() => setAddOpen(true)}>
            + ADD DEVICE
          </Button>
        }
      />

      {devices.loading ? (
        <Loading />
      ) : list.length === 0 ? (
        <Panel dots>
          <Empty label="NO DEVICES ENROLLED" />
        </Panel>
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {list.map((d) => (
            <DeviceCard
              key={d.id}
              device={d}
              onLock={onLock}
              onUnlock={onUnlock}
              onSsh={onSsh}
            />
          ))}
        </div>
      )}

      {/* Discovery */}
      <div className="mt-8">
        <Panel
          title="LAN DISCOVERY"
          aside={
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={async () => {
                const target = list.find((d) => d.status === "online");
                if (!target) return;
                setBusy(true);
                await scanDiscovery(target.id).catch(() => {});
                setBusy(false);
                setTimeout(() => discovery.reload(), 300);
              }}
            >
              {busy ? "SCANNING…" : "SCAN NOW"}
            </Button>
          }
        >
          {!latestScan || latestScan.hosts.length === 0 ? (
            <Empty label="NO SCAN RESULTS — TRIGGER A SCAN" />
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-xs font-mono">
                <thead>
                  <tr className="label" style={{ color: "var(--fg-faint)" }}>
                    <th className="text-left font-normal py-2 pr-4">IP</th>
                    <th className="text-left font-normal py-2 pr-4">HOSTNAME</th>
                    <th className="text-left font-normal py-2 pr-4">MAC</th>
                    <th className="text-left font-normal py-2 pr-4">VENDOR</th>
                    <th className="text-left font-normal py-2 pr-4">OPEN PORTS</th>
                    <th className="text-right font-normal py-2"></th>
                  </tr>
                </thead>
                <tbody>
                  {latestScan.hosts.map((h) => (
                    <tr
                      key={h.mac}
                      className="border-t"
                      style={{ borderColor: "var(--line)" }}
                    >
                      <td className="py-2 pr-4 tabular-nums text-fg">{h.ip}</td>
                      <td className="py-2 pr-4" style={{ color: "var(--fg-dim)" }}>
                        {h.hostname ?? "—"}
                      </td>
                      <td className="py-2 pr-4" style={{ color: "var(--fg-faint)" }}>
                        {h.mac}
                      </td>
                      <td className="py-2 pr-4" style={{ color: "var(--fg-dim)" }}>
                        {h.vendor ?? "—"}
                      </td>
                      <td className="py-2 pr-4">
                        {h.open_ports.length ? (
                          <span className="flex gap-1 flex-wrap">
                            {h.open_ports.map((p) => (
                              <span
                                key={p}
                                className="border rounded px-1.5 py-0.5 tabular-nums"
                                style={{ borderColor: "var(--line)" }}
                              >
                                {p}
                              </span>
                            ))}
                          </span>
                        ) : (
                          <span style={{ color: "var(--fg-faint)" }}>—</span>
                        )}
                      </td>
                      <td className="py-2 text-right">
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            setNewName(h.hostname ?? h.ip);
                            setAddOpen(true);
                          }}
                        >
                          ENROLL →
                        </Button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {latestScan && (
                <p className="label mt-3" style={{ color: "var(--fg-faint)" }}>
                  LAST SCAN {relTime(latestScan.created_at)}
                </p>
              )}
            </div>
          )}
        </Panel>
      </div>

      {/* Add device modal */}
      <Modal
        open={addOpen}
        onClose={() => {
          setAddOpen(false);
          setEnroll(null);
        }}
        title={enroll ? "ENROLLMENT TOKEN" : "ADD DEVICE"}
        footer={
          enroll ? (
            <Button
              variant="primary"
              onClick={() => {
                setAddOpen(false);
                setEnroll(null);
              }}
            >
              DONE
            </Button>
          ) : (
            <>
              <Button variant="ghost" onClick={() => setAddOpen(false)}>
                CANCEL
              </Button>
              <Button variant="primary" disabled={busy} onClick={handleAdd}>
                {busy ? "CREATING…" : "CREATE"}
              </Button>
            </>
          )
        }
      >
        {enroll ? (
          <div className="flex flex-col gap-4">
            <p className="text-xs" style={{ color: "var(--fg-dim)" }}>
              Device <span className="dot text-fg">{enroll.device.name}</span> is{" "}
              <StatusLed tone="pending" label="PENDING" className="align-middle" />. Run the
              agent with this single-use token:
            </p>
            <TokenBlock token={enroll.enroll_token} />
            <pre
              className="text-[0.6875rem] border rounded p-3 overflow-x-auto"
              style={{ borderColor: "var(--line)", background: "var(--surface-2)", color: "var(--fg-dim)" }}
            >
{`sudo ./sentinel-agent enroll \\
  --server http://localhost:8080 \\
  --token ${enroll.enroll_token}`}
            </pre>
          </div>
        ) : (
          <TextInput
            label="DEVICE NAME"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            placeholder="Living Room PC"
            autoFocus
          />
        )}
      </Modal>

      <SshModal ssh={ssh} onClose={() => setSsh(null)} />
    </>
  );
}

export function Loading() {
  return (
    <div className="flex items-center gap-3 py-16 justify-center">
      <StatusLed tone="ok" pulse />
      <span className="label">LOADING…</span>
    </div>
  );
}

export function Empty({ label }: { label: string }) {
  return (
    <p className="label py-10 text-center" style={{ color: "var(--fg-faint)" }}>
      {label}
    </p>
  );
}
