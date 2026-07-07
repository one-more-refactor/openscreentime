import { useNavigate } from "react-router-dom";
import { listPasskeys } from "../api";
import { auth } from "../api";
import type { Passkey } from "../types";
import { useAsync } from "../lib/useAsync";
import { useSession } from "../lib/session";
import { useTheme } from "../lib/theme";
import { PageHeader } from "../layout/Shell";
import { Button, PasskeyButton, Panel, StatusLed, Toggle } from "../components";
import { relTime } from "../lib/format";

export function Settings() {
  const { me, mock, logout, refresh } = useSession();
  const { theme, setTheme } = useTheme();
  const navigate = useNavigate();
  const passkeys = useAsync<Passkey[]>(listPasskeys, []);

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
    } catch (e) {
      alert(e instanceof Error ? e.message : "Passkey registration failed");
    }
  }

  return (
    <>
      <PageHeader title="SETTINGS" />

      <div className="grid lg:grid-cols-2 gap-6 items-start">
        <Panel title="ADMIN IDENTITY">
          <dl className="grid grid-cols-2 gap-x-6 gap-y-3">
            <Field k="DISPLAY NAME" v={me?.admin.display_name ?? "—"} />
            <Field k="EMAIL" v={me?.admin.email ?? "—"} />
            <Field k="TENANT" v={me?.tenant.name ?? "—"} />
            <Field k="AUTH" v="PASSKEY-ONLY" />
          </dl>
          <div className="mt-5 pt-4 border-t" style={{ borderColor: "var(--line)" }}>
            <Button variant="danger" onClick={handleLogout}>
              LOGOUT
            </Button>
          </div>
        </Panel>

        <Panel title="APPEARANCE">
          <div className="flex flex-col gap-4">
            <Toggle
              label="LIGHT THEME"
              hint="dark is the primary theme"
              checked={theme === "light"}
              onChange={(v) => setTheme(v ? "light" : "dark")}
            />
            {mock && (
              <div
                className="flex items-center gap-2 border rounded px-3 py-2"
                style={{ borderColor: "var(--warn)" }}
              >
                <StatusLed tone="warn" label="RUNNING ON MOCK DATA — BACKEND OFFLINE" />
              </div>
            )}
          </div>
        </Panel>

        <Panel
          title="PASSKEYS"
          className="lg:col-span-2"
          aside={<StatusLed tone="ok" label={`${passkeys.data?.length ?? 0} REGISTERED`} />}
        >
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
              </li>
            ))}
            {(passkeys.data ?? []).length === 0 && (
              <p className="label py-4" style={{ color: "var(--fg-faint)" }}>
                NO PASSKEYS
              </p>
            )}
          </ul>
          <div className="max-w-xs">
            <PasskeyButton label="+ ADD PASSKEY" onActivate={addPasskey} />
          </div>
        </Panel>
      </div>
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
