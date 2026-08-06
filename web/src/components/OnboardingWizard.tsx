import { useEffect, useMemo, useState } from "react";
import { assignProfile, createDevice, getDevice, listProfiles } from "../api";
import { useToast, errMsg } from "../lib/toast";
import { useAsync } from "../lib/useAsync";
import { Button } from "./Button";
import { TextInput, Select } from "./TextInput";
import { Modal } from "./Modal";
import { EnrollCommand } from "./EnrollCommand";
import type { DeviceDetail, Profile } from "../types";

type Step = "name" | "enroll" | "users" | "done";

/** First-device setup, held by the hand: name it → run one command on the
 *  device → watch it appear live → put each person on a profile → done.
 *  Opened automatically from the empty devices page, and reusable any time
 *  from the SET UP A DEVICE button. */
export function OnboardingWizard({
  onClose,
  onEnrolled,
}: {
  onClose: () => void;
  /** Called whenever the wizard changed the world (device created/enrolled). */
  onEnrolled: () => void;
}) {
  const { toast } = useToast();
  const [step, setStep] = useState<Step>("name");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [deviceId, setDeviceId] = useState<string | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [detail, setDetail] = useState<DeviceDetail | null>(null);
  const profiles = useAsync<Profile[]>(listProfiles, []);

  // ENROLL step: poll until the agent checks in (status leaves `pending`).
  useEffect(() => {
    if (step !== "enroll" || !deviceId) return;
    const t = setInterval(() => {
      getDevice(deviceId)
        .then((d) => {
          if (d.status !== "pending") {
            setDetail(d);
            setStep("users");
            onEnrolled();
          }
        })
        .catch(() => {
          // transient — keep polling
        });
    }, 3000);
    return () => clearInterval(t);
  }, [step, deviceId, onEnrolled]);

  // USERS step: the first heartbeat may not have reported OS users yet.
  useEffect(() => {
    if (step !== "users" || !deviceId) return;
    if ((detail?.users ?? []).length > 0) return;
    const t = setInterval(() => {
      getDevice(deviceId)
        .then(setDetail)
        .catch(() => {});
    }, 4000);
    return () => clearInterval(t);
  }, [step, deviceId, detail?.users]);

  async function submitName() {
    const n = name.trim();
    if (!n) {
      toast("Give the device a name — e.g. “kids-laptop”.", "warn");
      return;
    }
    setBusy(true);
    try {
      const res = await createDevice(n);
      setDeviceId(res.device.id);
      setToken(res.enroll_token);
      setStep("enroll");
      onEnrolled();
    } catch (e: unknown) {
      toast(errMsg(e, "Couldn't create the device — try again."));
    } finally {
      setBusy(false);
    }
  }

  const stepIndex = useMemo(
    () => ["name", "enroll", "users", "done"].indexOf(step) + 1,
    [step],
  );

  return (
    <Modal
      open
      title={`SET UP A DEVICE — STEP ${stepIndex}/4`}
      onClose={onClose}
    >
      {step === "name" && (
        <div className="flex flex-col gap-4">
          <p className="text-sm leading-relaxed">
            OpenScreenTime sets up a device in three steps: you name it here, run
            one command on the device itself, and pick who gets which rules. The
            whole thing takes about two minutes.
          </p>
          <TextInput
            label="WHAT DEVICE IS THIS?"
            hint="e.g. kids-laptop, living-room-pc"
            value={name}
            onChange={(e) => setName(e.target.value)}
            maxLength={64}
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter") void submitName();
            }}
          />
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={onClose}>
              LATER
            </Button>
            <Button variant="primary" disabled={busy} onClick={() => void submitName()}>
              NEXT →
            </Button>
          </div>
        </div>
      )}

      {step === "enroll" && token && (
        <div className="flex flex-col gap-4">
          <p className="text-sm leading-relaxed">
            Now switch to <span className="dot text-xs">{name.trim().toUpperCase()}</span> and
            run this in a terminal. It installs the agent and connects it here —
            the token is single-use and valid for 24&nbsp;hours.
          </p>
          <EnrollCommand token={token} />
          <p className="label" style={{ color: "var(--warn)" }}>
            WAITING FOR THE DEVICE TO CHECK IN<span className="animate-pulse">…</span>
          </p>
          <p className="text-[0.6875rem]" style={{ color: "var(--fg-faint)" }}>
            This page moves on by itself the moment the agent connects. Nothing
            happening after a minute? Make sure the device can reach{" "}
            {window.location.origin} and ran the command as root.
          </p>
          <div className="flex justify-end">
            <Button variant="ghost" onClick={onClose}>
              FINISH LATER
            </Button>
          </div>
        </div>
      )}

      {step === "users" && (
        <div className="flex flex-col gap-4">
          <p className="text-sm leading-relaxed">
            <span className="dot text-xs">CONNECTED.</span> The agent reported
            the people on this device. Everyone starts on the DEFAULT profile —
            switch anyone to KIDS or TEEN now (you can always change it later,
            or edit the profiles themselves under PROFILES).
          </p>
          {(detail?.users ?? []).length === 0 ? (
            <p className="label text-muted">
              WAITING FOR THE AGENT TO REPORT OS USERS<span className="animate-pulse">…</span>
            </p>
          ) : (
            <div className="flex flex-col gap-2">
              {(detail?.users ?? []).map((u) => (
                <div key={u.id} className="flex items-center gap-3 border rounded hairline px-3 py-2">
                  <span className="dot text-xs flex-1">
                    {(u.display_name ?? u.os_username).toUpperCase()}
                  </span>
                  <Select
                    className="w-44"
                    value={u.profile_id}
                    onChange={(e) => {
                      const pid = e.target.value;
                      assignProfile(u.id, pid)
                        .then(() => {
                          setDetail((d) =>
                            d
                              ? {
                                  ...d,
                                  users: d.users.map((x) =>
                                    x.id === u.id ? { ...x, profile_id: pid } : x,
                                  ),
                                }
                              : d,
                          );
                          toast("PROFILE ASSIGNED — APPLIES ON NEXT SYNC", "ok");
                        })
                        .catch((e: unknown) => toast(errMsg(e, "Couldn't assign — try again.")));
                    }}
                  >
                    {(profiles.data ?? []).map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name.toUpperCase()}
                      </option>
                    ))}
                  </Select>
                </div>
              ))}
            </div>
          )}
          <div className="flex justify-end gap-2">
            <Button variant="primary" onClick={() => setStep("done")}>
              {(detail?.users ?? []).length === 0 ? "SKIP FOR NOW →" : "NEXT →"}
            </Button>
          </div>
        </div>
      )}

      {step === "done" && (
        <div className="flex flex-col gap-4">
          <p className="text-sm leading-relaxed">
            <span className="dot text-xs">DONE.</span> {name.trim() || "The device"} is
            enrolled and enforcing its profiles. From here:
          </p>
          <ul className="text-sm leading-relaxed list-none flex flex-col gap-2">
            <li>
              <span className="label text-muted">LOCK / UNLOCK</span> — right on the
              device card, instant when the device is online.
            </li>
            <li>
              <span className="label text-muted">SCREEN TIME</span> — daily limits and
              bedtime live in the profile; usage history is on the device page.
            </li>
            <li>
              <span className="label text-muted">APPROVALS</span> — earn-time requests
              land there (and can ping your phone — see SETTINGS).
            </li>
            <li>
              <span className="label text-muted">VPN</span> — upload a tunnel config on
              the device page; it's tested before it's enforced.
            </li>
          </ul>
          <div className="flex justify-end">
            <Button variant="primary" onClick={onClose}>
              GO TO DEVICES
            </Button>
          </div>
        </div>
      )}
    </Modal>
  );
}
