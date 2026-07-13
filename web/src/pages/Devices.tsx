import { useRef, useState } from "react";
import {
  createDevice,
  deleteDevice,
  getDiscoveryResults,
  listDevices,
  lockDevice,
  scanDiscovery,
  unlockDevice,
} from "../api";
import type { Device, EnrollTokenResponse } from "../types";
import { useAsync } from "../lib/useAsync";
import { useToast, errMsg } from "../lib/toast";
import { PageHeader } from "../layout/Shell";
import {
  Button,
  DeviceCard,
  ErrorPanel,
  Modal,
  Panel,
  SshTerminal,
  Stat,
  StatusLed,
  TextInput,
  TokenBlock,
} from "../components";
import { pad2, relTime } from "../lib/format";

const SCAN_POLL_MS = 2_000;
const SCAN_TIMEOUT_MS = 30_000;

export function Devices() {
  const devices = useAsync<Device[]>(listDevices, []);
  const discovery = useAsync(getDiscoveryResults, []);
  const { toast } = useToast();

  const [addOpen, setAddOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [nameError, setNameError] = useState<string | null>(null);
  const [enroll, setEnroll] = useState<EnrollTokenResponse | null>(null);
  const [ssh, setSsh] = useState<{ id: string; name: string } | null>(null);
  const [creating, setCreating] = useState(false);
  const [busyIds, setBusyIds] = useState<Set<string>>(new Set());
  const [confirmDelete, setConfirmDelete] = useState<Device | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [scanning, setScanning] = useState(false);
  const scanRun = useRef(0);

  const list = devices.data ?? [];
  const online = list.filter((d) => d.status === "online").length;
  const locked = list.filter((d) => d.status === "locked").length;

  const setBusy = (id: string, on: boolean) =>
    setBusyIds((prev) => {
      const next = new Set(prev);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  async function handleAdd() {
    const name = newName.trim();
    if (!name) {
      setNameError("Enter a device name first.");
      return;
    }
    setNameError(null);
    setCreating(true);
    try {
      const res = await createDevice(name);
      setEnroll(res);
      setNewName("");
      devices.reload();
    } catch (e) {
      toast(errMsg(e, "Couldn't create the device — try again."));
    } finally {
      setCreating(false);
    }
  }

  async function setLockState(d: Device, lock: boolean) {
    const prevStatus = d.status;
    setBusy(d.id, true);
    // optimistic
    devices.setData((prev) =>
      (prev ?? []).map((x) =>
        x.id === d.id ? { ...x, status: lock ? "locked" : "online" } : x,
      ),
    );
    try {
      await (lock ? lockDevice(d.id) : unlockDevice(d.id));
    } catch (e) {
      // roll back
      devices.setData((prev) =>
        (prev ?? []).map((x) => (x.id === d.id ? { ...x, status: prevStatus } : x)),
      );
      toast(errMsg(e, `Couldn't ${lock ? "lock" : "unlock"} ${d.name} — try again.`));
    } finally {
      setBusy(d.id, false);
    }
  }

  async function handleDelete() {
    if (!confirmDelete) return;
    const target = confirmDelete;
    setDeleting(true);
    try {
      await deleteDevice(target.id);
      devices.setData((prev) => (prev ?? []).filter((x) => x.id !== target.id));
      setConfirmDelete(null);
      toast(`${target.name} removed.`, "ok");
    } catch (e) {
      toast(errMsg(e, `Couldn't remove ${target.name} — try again.`));
    } finally {
      setDeleting(false);
    }
  }

  async function handleScan() {
    const target = list.find((d) => d.status === "online");
    if (!target) {
      toast("No online device can run a scan — bring a device online first.", "warn");
      return;
    }
    const run = ++scanRun.current;
    setScanning(true);
    try {
      await scanDiscovery(target.id);
    } catch (e) {
      setScanning(false);
      toast(errMsg(e, "Couldn't start the scan — try again."));
      return;
    }
    // Poll for fresh results every 2 s, up to 30 s or until they change.
    const before = discovery.data?.[0]?.id ?? null;
    const startedAt = Date.now();
    const poll = async () => {
      if (run !== scanRun.current) return; // superseded
      let latest: string | null = before;
      try {
        const results = await getDiscoveryResults();
        latest = results[0]?.id ?? null;
        discovery.setData(() => results);
      } catch {
        /* transient — keep polling */
      }
      if (latest !== before) {
        setScanning(false);
        toast("Scan finished — results updated.", "ok");
        return;
      }
      if (Date.now() - startedAt >= SCAN_TIMEOUT_MS) {
        setScanning(false);
        toast("Scan timed out after 30 s — no new results yet.", "warn");
        return;
      }
      window.setTimeout(() => void poll(), SCAN_POLL_MS);
    };
    window.setTimeout(() => void poll(), SCAN_POLL_MS);
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
          <Button
            variant="primary"
            onClick={() => {
              setNameError(null);
              setAddOpen(true);
            }}
          >
            + ADD DEVICE
          </Button>
        }
      />

      {devices.loading ? (
        <Loading />
      ) : devices.error ? (
        <ErrorPanel
          title="Couldn't load devices"
          detail={devices.error}
          onRetry={devices.reload}
        />
      ) : list.length === 0 ? (
        <Panel dots refCode="DV-00">
          <Empty label="NO DEVICES ENROLLED" />
        </Panel>
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {list.map((d, i) => (
            <DeviceCard
              key={d.id}
              device={d}
              refCode={`DV-${pad2(i + 1)}`}
              busy={busyIds.has(d.id)}
              onLock={(x) => void setLockState(x, true)}
              onUnlock={(x) => void setLockState(x, false)}
              onSsh={(x) => setSsh({ id: x.id, name: x.name })}
              onDelete={(x) => setConfirmDelete(x)}
            />
          ))}
        </div>
      )}

      {/* Discovery */}
      <div className="mt-8">
        <Panel
          title="LAN DISCOVERY"
          refCode="SCAN-01"
          aside={
            <Button size="sm" variant="ghost" disabled={scanning} onClick={() => void handleScan()}>
              {scanning ? "SCANNING…" : "SCAN NOW"}
            </Button>
          }
        >
          {discovery.error ? (
            <ErrorPanel
              title="Couldn't load scan results"
              detail={discovery.error}
              onRetry={discovery.reload}
            />
          ) : !latestScan || latestScan.hosts.length === 0 ? (
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
                        {h.hostname || "—"}
                      </td>
                      <td className="py-2 pr-4" style={{ color: "var(--fg-faint)" }}>
                        {h.mac}
                      </td>
                      <td className="py-2 pr-4" style={{ color: "var(--fg-dim)" }}>
                        {h.vendor || "—"}
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
                            setNameError(null);
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
              <Button variant="primary" disabled={creating} onClick={() => void handleAdd()}>
                {creating ? "CREATING…" : "CREATE"}
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
            onChange={(e) => {
              setNewName(e.target.value);
              if (nameError) setNameError(null);
            }}
            placeholder="Living Room PC"
            autoFocus
            aria-invalid={!!nameError}
            hint={nameError ?? undefined}
            style={nameError ? { borderColor: "var(--accent)" } : undefined}
          />
        )}
      </Modal>

      {/* Delete device confirm */}
      <Modal
        open={!!confirmDelete}
        onClose={() => setConfirmDelete(null)}
        title="REMOVE DEVICE"
        danger
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmDelete(null)}>
              CANCEL
            </Button>
            <Button variant="danger" disabled={deleting} onClick={() => void handleDelete()}>
              {deleting ? "REMOVING…" : "REMOVE DEVICE"}
            </Button>
          </>
        }
      >
        <p className="text-xs leading-relaxed" style={{ color: "var(--fg-dim)" }}>
          This removes <span className="dot text-fg">{confirmDelete?.name}</span> and its
          users, policies and history from the control center. The agent on the machine
          keeps running until it is uninstalled. This cannot be undone.
        </p>
      </Modal>

      <SshTerminal target={ssh} onClose={() => setSsh(null)} />
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
