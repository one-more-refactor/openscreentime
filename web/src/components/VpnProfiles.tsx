import { useEffect, useRef, useState } from "react";
import {
  activateVpnProfile,
  createVpnProfile,
  deactivateVpnProfile,
  deleteVpnProfile,
  listVpnProfiles,
  updateVpnProfile,
} from "../api";
import { useAsync } from "../lib/useAsync";
import { useToast, errMsg } from "../lib/toast";
import { Button } from "./Button";
import { TextInput } from "./TextInput";
import { Modal } from "./Modal";
import type { VpnKind, VpnProfile } from "../types";

function sniffVpnKind(name: string, text: string): VpnKind | null {
  if (text.includes("[Interface]")) return "wireguard";
  if (/^\s*(remote\s+\S+|client\s*$)/m.test(text)) return "openvpn";
  if (name.endsWith(".ovpn")) return "openvpn";
  return null;
}

const STATUS_TONE: Record<VpnProfile["status"], string> = {
  untested: "var(--fg-faint)",
  testing: "var(--warn)",
  active: "var(--ok)",
  failed: "var(--accent)",
};

const STATUS_HINT: Record<VpnProfile["status"], string> = {
  untested: "Stored, never tried on the device yet.",
  testing: "The device is bringing the tunnel up and checking it really works…",
  active: "Verified on the device — traffic is going through this tunnel.",
  failed: "The device tried it, the tunnel didn't come up, and the previous setup was restored.",
};

/** Named VPN profiles for one device: upload several, exactly one active.
 *  Activation is test-before-enforce — the agent brings the tunnel up,
 *  verifies it, and reports back; a broken config rolls back automatically.
 *  Private keys never leave the server: configs render masked (•••) and
 *  edits through the mask keep the stored secrets. */
export function VpnProfiles({ deviceId }: { deviceId: string }) {
  const { toast } = useToast();
  const profiles = useAsync<VpnProfile[]>(() => listVpnProfiles(deviceId), [deviceId]);
  const [busy, setBusy] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<VpnProfile | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<VpnProfile | null>(null);

  // A profile in `testing` resolves on the agent's report — poll until it does.
  useEffect(() => {
    if (!(profiles.data ?? []).some((p) => p.status === "testing")) return;
    const t = setInterval(profiles.reload, 6000);
    return () => clearInterval(t);
  }, [profiles.data, profiles.reload]);

  async function run(id: string, fn: () => Promise<unknown>, doneMsg?: string) {
    setBusy(id);
    try {
      await fn();
      if (doneMsg) toast(doneMsg, "ok");
    } catch (e: unknown) {
      toast(errMsg(e, "That didn't work — try again."));
    } finally {
      setBusy(null);
      profiles.reload();
    }
  }

  const list = profiles.data ?? [];

  return (
    <div className="flex flex-col gap-3">
      <p className="text-[0.6875rem] leading-relaxed" style={{ color: "var(--fg-faint)" }}>
        Route this device's traffic through a VPN. Upload one or more client
        configs, then activate the one to use — the device tests a tunnel
        before enforcing it and rolls back if it doesn't come up. Private keys
        stay on the server and are shown as ••• here.
      </p>

      {profiles.loading && !profiles.data && <p className="label text-muted">PROFILES…</p>}
      {list.length === 0 && !profiles.loading && (
        <p className="label text-muted">NO VPN PROFILES — ADD ONE BELOW</p>
      )}

      {list.map((p) => (
        <div key={p.id} className="border rounded hairline px-3 py-2 flex flex-col gap-1">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="dot text-xs text-fg">{p.name.toUpperCase()}</span>
            <span className="label text-muted">{p.kind.toUpperCase()}</span>
            <span
              className="label border rounded px-1.5 py-0.5"
              style={{ color: STATUS_TONE[p.status], borderColor: STATUS_TONE[p.status] }}
              title={STATUS_HINT[p.status]}
            >
              {p.is_active ? `● ${p.status.toUpperCase()}` : p.status.toUpperCase()}
            </span>
            <span className="ml-auto flex items-center gap-1.5">
              {p.is_active ? (
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy !== null}
                  onClick={() =>
                    run(p.id, () => deactivateVpnProfile(p.id), "TUNNEL STOPS ON NEXT AGENT SYNC")
                  }
                >
                  TURN OFF
                </Button>
              ) : (
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy !== null}
                  onClick={() =>
                    run(
                      p.id,
                      () => activateVpnProfile(p.id),
                      "ACTIVATING — THE DEVICE TESTS THE TUNNEL FIRST",
                    )
                  }
                >
                  USE THIS
                </Button>
              )}
              <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => setEditing(p)}>
                EDIT
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={busy !== null}
                onClick={() => setConfirmDelete(p)}
              >
                DELETE
              </Button>
            </span>
          </div>
          {p.status === "failed" && p.last_error && (
            <p className="text-[0.6875rem]" style={{ color: "var(--accent)" }}>
              {p.last_error}
            </p>
          )}
        </div>
      ))}

      <Button variant="ghost" size="sm" onClick={() => setAdding(true)}>
        + ADD VPN PROFILE
      </Button>

      {adding && (
        <AddProfileModal
          onClose={() => setAdding(false)}
          onSubmit={async (name, config, kind) => {
            await createVpnProfile(deviceId, name, config, kind);
            setAdding(false);
            profiles.reload();
            toast("PROFILE SAVED — ACTIVATE IT WHEN YOU'RE READY", "ok");
          }}
        />
      )}
      {editing && (
        <EditProfileModal
          profile={editing}
          onClose={() => setEditing(null)}
          onSubmit={async (name, config) => {
            await updateVpnProfile(editing.id, name, config);
            setEditing(null);
            profiles.reload();
            toast(
              editing.is_active
                ? "SAVED — THE DEVICE RE-TESTS THE ACTIVE TUNNEL"
                : "PROFILE SAVED",
              "ok",
            );
          }}
        />
      )}
      {confirmDelete && (
        <Modal open title="DELETE VPN PROFILE" onClose={() => setConfirmDelete(null)}>
          <p className="text-sm mb-4">
            Delete “{confirmDelete.name}”?
            {confirmDelete.is_active
              ? " It is the active tunnel — the device will drop back to no VPN."
              : ""}
          </p>
          <div className="flex gap-2 justify-end">
            <Button variant="ghost" onClick={() => setConfirmDelete(null)}>
              KEEP IT
            </Button>
            <Button
              onClick={() => {
                const p = confirmDelete;
                setConfirmDelete(null);
                void run(p.id, () => deleteVpnProfile(p.id), "PROFILE DELETED");
              }}
            >
              DELETE
            </Button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function AddProfileModal({
  onClose,
  onSubmit,
}: {
  onClose: () => void;
  onSubmit: (name: string, config: string, kind: VpnKind) => Promise<void>;
}) {
  const { toast } = useToast();
  const [name, setName] = useState("");
  const [config, setConfig] = useState("");
  const [kind, setKind] = useState<VpnKind | null>(null);
  const [busy, setBusy] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  async function takeFile(file: File | undefined) {
    if (!file) return;
    const text = await file.text();
    const k = sniffVpnKind(file.name, text);
    if (!k) {
      toast("That doesn't look like a WireGuard (.conf) or OpenVPN (.ovpn) client config.", "warn");
      return;
    }
    setConfig(text);
    setKind(k);
    if (!name) setName(file.name.replace(/\.(conf|ovpn)$/i, ""));
  }

  async function submit() {
    const k = kind ?? sniffVpnKind("", config);
    if (!name.trim() || !config.trim() || !k) {
      toast("Give the profile a name and paste or drop a client config.", "warn");
      return;
    }
    setBusy(true);
    try {
      await onSubmit(name.trim(), config, k);
    } catch (e: unknown) {
      toast(errMsg(e, "Couldn't save the profile — try again."));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal open title="ADD VPN PROFILE" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <label className="label text-muted" htmlFor="vpn-name">
          NAME — e.g. “home”, “mullvad amsterdam”
        </label>
        <TextInput
          id="vpn-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="home"
          maxLength={64}
        />
        <div
          className={`border rounded hairline px-3 py-4 text-center cursor-pointer ${
            dragOver ? "bg-surface-2" : ""
          }`}
          onClick={() => fileInput.current?.click()}
          onDragOver={(e) => {
            e.preventDefault();
            setDragOver(true);
          }}
          onDragLeave={() => setDragOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setDragOver(false);
            void takeFile(e.dataTransfer.files[0]);
          }}
        >
          <p className="label text-muted">
            {kind ? `${kind.toUpperCase()} CONFIG LOADED` : "DROP A .CONF / .OVPN FILE — OR CLICK"}
          </p>
          <input
            ref={fileInput}
            type="file"
            accept=".conf,.ovpn,.txt"
            className="hidden"
            onChange={(e) => void takeFile(e.target.files?.[0] ?? undefined)}
          />
        </div>
        <label className="label text-muted" htmlFor="vpn-config">
          …OR PASTE THE CONFIG
        </label>
        <textarea
          id="vpn-config"
          className="input font-mono text-xs h-40"
          value={config}
          onChange={(e) => {
            setConfig(e.target.value);
            setKind(sniffVpnKind("", e.target.value));
          }}
          placeholder={"[Interface]\nPrivateKey = …"}
        />
        <div className="flex gap-2 justify-end">
          <Button variant="ghost" onClick={onClose}>
            CANCEL
          </Button>
          <Button disabled={busy} onClick={() => void submit()}>
            SAVE PROFILE
          </Button>
        </div>
      </div>
    </Modal>
  );
}

function EditProfileModal({
  profile,
  onClose,
  onSubmit,
}: {
  profile: VpnProfile;
  onClose: () => void;
  onSubmit: (name: string, config: string) => Promise<void>;
}) {
  const { toast } = useToast();
  const [name, setName] = useState(profile.name);
  const [config, setConfig] = useState(profile.config_masked);
  const [busy, setBusy] = useState(false);

  return (
    <Modal open title={`EDIT ${profile.name.toUpperCase()}`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <label className="label text-muted" htmlFor="vpn-edit-name">
          NAME
        </label>
        <TextInput
          id="vpn-edit-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          maxLength={64}
        />
        <label className="label text-muted" htmlFor="vpn-edit-config">
          CONFIG — ••• LINES ARE YOUR STORED SECRETS; LEAVE THEM AS-IS TO KEEP
          THEM, OR PASTE A NEW VALUE TO REPLACE
        </label>
        <textarea
          id="vpn-edit-config"
          className="input font-mono text-xs h-48"
          value={config}
          onChange={(e) => setConfig(e.target.value)}
        />
        <div className="flex gap-2 justify-end">
          <Button variant="ghost" onClick={onClose}>
            CANCEL
          </Button>
          <Button
            disabled={busy}
            onClick={() => {
              setBusy(true);
              onSubmit(name.trim(), config).catch((e: unknown) => {
                toast(errMsg(e, "Couldn't save — try again."));
                setBusy(false);
              });
            }}
          >
            SAVE
          </Button>
        </div>
      </div>
    </Modal>
  );
}
