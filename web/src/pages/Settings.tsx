import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { ApiError, auth, deletePasskey, getAuthConfig, listPasskeys } from "../api";
import type { AuthConfig, Passkey } from "../types";
import { useAsync } from "../lib/useAsync";
import { useToast, errMsg } from "../lib/toast";
import { useSession } from "../lib/session";
import { useTheme } from "../lib/theme";
import { PageHeader } from "../layout/Shell";
import {
  Button,
  ErrorPanel,
  Modal,
  PasskeyButton,
  Panel,
  StatusLed,
  Toggle,
} from "../components";
import { relTime } from "../lib/format";

export function Settings() {
  const { me, mock, logout, refresh } = useSession();
  const { theme, setTheme } = useTheme();
  const { toast } = useToast();
  const navigate = useNavigate();
  const passkeys = useAsync<Passkey[]>(listPasskeys, []);
  const authConfig = useAsync<AuthConfig>(getAuthConfig, []);

  const [confirmDelete, setConfirmDelete] = useState<Passkey | null>(null);
  const [deleting, setDeleting] = useState(false);

  const oidc = authConfig.data?.oidc ?? false;
  const oidcName = authConfig.data?.oidc_name || "SSO";

  async function handleLogout() {
    await logout();
    navigate("/login", { replace: true });
  }

  async function addPasskey() {
    if (!me) return;
    try {
      await auth.register(me.admin.email, me.admin.display_name);
      await refresh();
      passkeys.reload();
      toast("Passkey added.", "ok");
    } catch (e) {
      toast(errMsg(e, "Passkey registration failed — try again."));
    }
  }

  async function handleDelete() {
    if (!confirmDelete) return;
    const key = confirmDelete;
    setDeleting(true);
    try {
      await deletePasskey(key.id);
      passkeys.setData((prev) => (prev ?? []).filter((k) => k.id !== key.id));
      setConfirmDelete(null);
      toast(`Passkey "${key.nickname}" removed.`, "ok");
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        toast(
          "This is your last passkey — removing it would lock you out. Add another passkey (or enable SSO) first.",
          "warn",
        );
      } else {
        toast(errMsg(e, "Couldn't remove the passkey — try again."));
      }
      setConfirmDelete(null);
    } finally {
      setDeleting(false);
    }
  }

  return (
    <>
      <PageHeader title="SETTINGS" />

      <div className="grid lg:grid-cols-2 gap-6 items-start">
        <Panel title="ADMIN IDENTITY" refCode="AD-01">
          <dl className="grid grid-cols-2 gap-x-6 gap-y-3">
            <Field k="DISPLAY NAME" v={me?.admin.display_name ?? "—"} />
            <Field k="EMAIL" v={me?.admin.email ?? "—"} />
            <Field k="TENANT" v={me?.tenant.name ?? "—"} />
            <Field k="AUTH" v={oidc ? `PASSKEY + ${oidcName.toUpperCase()}` : "PASSKEY-ONLY"} />
          </dl>
          {oidc && (
            <div className="mt-4 flex items-center gap-2">
              <StatusLed tone="ok" label={`${oidcName.toUpperCase()} SSO ENABLED`} />
            </div>
          )}
          <div className="mt-5 pt-4 border-t" style={{ borderColor: "var(--line)" }}>
            <Button variant="danger" onClick={() => void handleLogout()}>
              LOGOUT
            </Button>
          </div>
        </Panel>

        <Panel title="APPEARANCE" refCode="AP-01">
          <div className="flex flex-col gap-4">
            <Toggle
              label="LIGHT THEME"
              hint="silkscreen-on-white — same language"
              checked={theme === "light"}
              onChange={(v) => setTheme(v ? "light" : "dark")}
            />
            {mock && (
              <div
                className="flex items-center gap-2 border rounded px-3 py-2"
                style={{ borderColor: "var(--warn)" }}
              >
                <StatusLed tone="warn" label="DESIGN-REVIEW MODE — MOCK DATA (VITE_USE_MOCK=1)" />
              </div>
            )}
          </div>
        </Panel>

        <Panel
          title="PASSKEYS"
          className="lg:col-span-2"
          refCode="PK-01"
          aside={<StatusLed tone="ok" label={`${passkeys.data?.length ?? 0} REGISTERED`} />}
        >
          {passkeys.error ? (
            <ErrorPanel
              title="Couldn't load your passkeys"
              detail={passkeys.error}
              onRetry={passkeys.reload}
            />
          ) : (
            <ul className="flex flex-col mb-4">
              {(passkeys.data ?? []).map((k) => (
                <li
                  key={k.id}
                  className="flex items-center gap-4 py-3 border-b last:border-b-0 flex-wrap"
                  style={{ borderColor: "var(--line)" }}
                >
                  <span className="led led-glow-ok" style={{ background: "var(--ok)" }} />
                  <div className="flex-1 min-w-0">
                    <p className="dot text-xs text-fg">{k.nickname}</p>
                    <p className="text-[0.625rem]" style={{ color: "var(--fg-faint)" }}>
                      ADDED {relTime(k.created_at)} · LAST USED {relTime(k.last_used_at)}
                    </p>
                  </div>
                  <Button
                    size="sm"
                    variant="danger"
                    disabled={(passkeys.data?.length ?? 0) <= 1 && !oidc}
                    title={
                      (passkeys.data?.length ?? 0) <= 1 && !oidc
                        ? "Your last passkey can't be removed — you'd lock yourself out"
                        : undefined
                    }
                    onClick={() => setConfirmDelete(k)}
                  >
                    REMOVE
                  </Button>
                </li>
              ))}
              {(passkeys.data ?? []).length === 0 && !passkeys.loading && (
                <p className="label py-4" style={{ color: "var(--fg-faint)" }}>
                  NO PASSKEYS
                </p>
              )}
            </ul>
          )}
          <div className="max-w-xs">
            <PasskeyButton label="+ ADD PASSKEY" onActivate={addPasskey} />
          </div>
        </Panel>
      </div>

      {/* Passkey delete confirm */}
      <Modal
        open={!!confirmDelete}
        onClose={() => setConfirmDelete(null)}
        title="REMOVE PASSKEY"
        danger
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmDelete(null)}>
              CANCEL
            </Button>
            <Button variant="danger" disabled={deleting} onClick={() => void handleDelete()}>
              {deleting ? "REMOVING…" : "REMOVE PASSKEY"}
            </Button>
          </>
        }
      >
        <p className="text-xs leading-relaxed" style={{ color: "var(--fg-dim)" }}>
          Remove <span className="dot text-fg">{confirmDelete?.nickname}</span>? Devices that
          signed in with it will need another passkey{oidc ? ` or ${oidcName}` : ""} to get
          back in.
        </p>
      </Modal>
    </>
  );
}

function Field({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="label">{k}</dt>
      <dd className="dot text-xs text-fg break-all">{v}</dd>
    </div>
  );
}
