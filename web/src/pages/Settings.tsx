// ============================================================================
// SETTINGS — what a parent needs, and nothing else.
//
// You and Appearance are free to read and change. The keys — each computer's
// unlock code and recovery codes, your passkeys, a paired phone — sit behind
// "Confirm it's you" (a passkey, or a code on your own computer; a fresh
// sign-in counts). Their data is not even fetched until then: the server
// answers these with 428 outside the confirm window (docs/AUTH.md). The
// client gate is comfort; the server is the lock.
//
// Signing out lives in one place only: the rail.
// ============================================================================
import { useEffect, useState } from "react";
import { useLocation } from "react-router-dom";
import {
  ApiError,
  addPasskey,
  deletePasskey,
  getAuthConfig,
  getTelegram,
  listDevices,
  listParentTokens,
  listPasskeys,
  pairTelegram,
  revokeParentToken,
  unpairTelegram,
  usingMock,
} from "../api";
import type { AuthConfig, Device, ParentToken, Passkey, TelegramPairing, TelegramStatus } from "../types";
import { useAsync } from "../lib/useAsync";
import { useSession } from "../lib/session";
import { useTheme, type ThemeMode } from "../lib/theme";
import { useConfirm } from "../lib/confirm";
import { Button } from "../components/Button";
import { Icon } from "../components/Icon";
import { Modal } from "../components/Modal";
import { CopyField } from "../components/CopyField";
import { PasskeyButton } from "../components/PasskeyButton";
import { UnlockCodePanel } from "../components/UnlockCodePanel";
import { PageHead } from "../layout/PageHead";
import { ago } from "../lib/format";

export function Settings() {
  return (
    <div className="page page-narrow settings">
      <PageHead title="Settings" sub="Your account, how the console looks, and the keys to the house." />
      <You />
      <Appearance />
      <Security />
    </div>
  );
}

// ---- free to read -----------------------------------------------------------

function You() {
  const { me } = useSession();
  const name = me?.account?.display_name ?? me?.admin.display_name ?? "—";
  const handle = me?.admin.username ?? me?.account?.email;
  const house = me?.household?.name ?? me?.tenant.name;
  return (
    <section className="section">
      <h2 className="h2">You</h2>
      <div className="card rows">
        <div className="row">
          <div className="row-main">
            <p className="row-title">{name}</p>
            <p className="row-sub">
              {[handle ? `Signs in as ${handle}` : null, house].filter(Boolean).join(" · ")}
            </p>
          </div>
        </div>
      </div>
    </section>
  );
}

const THEMES: { key: ThemeMode; label: string }[] = [
  { key: "light", label: "Light" },
  { key: "dark", label: "Dark" },
  { key: "system", label: "Match my system" },
];

function Appearance() {
  const { mode, setMode } = useTheme();
  return (
    <section className="section">
      <h2 className="h2">Appearance</h2>
      <div className="card rows">
        <div className="row row-wrap">
          <div className="row-main">
            <p className="row-title" id="theme-label">
              Theme
            </p>
            <p className="row-sub">For this browser. The computers' own screens stay light.</p>
          </div>
          <div className="row-end">
            <div className="seg" role="radiogroup" aria-labelledby="theme-label">
              {THEMES.map((t) => (
                <button
                  key={t.key}
                  type="button"
                  role="radio"
                  aria-checked={mode === t.key}
                  className="seg-btn"
                  onClick={() => setMode(t.key)}
                >
                  {t.label}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

// ---- the keys ---------------------------------------------------------------

function Security() {
  const { enter, armed } = useConfirm();
  const [checking, setChecking] = useState(false);

  // Design review only: ?mock=confirm opens the dialog for a screenshot.
  useEffect(() => {
    if (usingMock && new URLSearchParams(window.location.search).get("mock") === "confirm") void enter();
  }, [enter]);

  // Open exactly while the confirm window is — when it lapses, the gate
  // closes by itself.
  async function unlock() {
    setChecking(true);
    try {
      await enter();
    } finally {
      setChecking(false);
    }
  }

  return (
    <section className="section">
      <h2 className="h2">Security</h2>
      {armed ? (
        <div className="stack">
          <Passkeys />
          <UnlockCodes />
          <Phone />
          <Companions />
        </div>
      ) : (
        <div className="card gate">
          <span className="gate-ic" aria-hidden="true">
            <Icon name="lock" size={22} />
          </span>
          <p className="gate-title">Confirm it's you to see the keys</p>
          <p className="gate-sub">The computers' unlock and recovery codes, and your passkeys.</p>
          <Button disabled={checking} onClick={() => void unlock()}>
            {checking ? "Checking…" : "Confirm it's you"}
          </Button>
        </div>
      )}
    </section>
  );
}

function Passkeys() {
  const recovered = (useLocation().state as { recovered?: boolean } | null)?.recovered === true;
  const authConfig = useAsync<AuthConfig>(getAuthConfig, []);
  const passkeys = useAsync<Passkey[]>(listPasskeys, []);
  const [confirmDelete, setConfirmDelete] = useState<Passkey | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ msg: string; error: boolean } | null>(null);

  const oidc = authConfig.data?.oidc ?? false;
  const keys = passkeys.data ?? [];
  const lastKey = keys.length <= 1 && !oidc;

  async function add() {
    setStatus(null);
    try {
      await addPasskey();
      passkeys.reload();
      setStatus({ msg: "Passkey added.", error: false });
    } catch (e) {
      if (e instanceof Error && (e.name === "NotAllowedError" || e.name === "AbortError")) return;
      setStatus({ msg: e instanceof Error ? e.message : "The passkey wasn't added.", error: true });
    }
  }

  async function remove() {
    if (!confirmDelete) return;
    const key = confirmDelete;
    setBusy(true);
    try {
      await deletePasskey(key.id);
      passkeys.setData((prev) => (prev ?? []).filter((k) => k.id !== key.id));
      setStatus({ msg: `Removed ${key.nickname}.`, error: false });
    } catch (e) {
      setStatus({
        msg:
          e instanceof ApiError && e.status === 409
            ? "That's your last passkey. Removing it would lock you out."
            : e instanceof Error
              ? e.message
              : "Couldn't remove the passkey.",
        error: true,
      });
    } finally {
      setBusy(false);
      setConfirmDelete(null);
    }
  }

  return (
    <div className="card">
      <div className="card-head">
        <div>
          <p className="row-title">Passkeys</p>
          <p className="row-sub">
            {recovered
              ? "You came in with a recovery link. Add a passkey now, so next time is one tap."
              : "One tap to sign in, on each phone or computer you trust."}
          </p>
        </div>
      </div>
      <div className="rows">
        {passkeys.error && (
          <div className="row">
            <p className="hint" data-error="true">
              Couldn't load your passkeys: {passkeys.error}
            </p>
          </div>
        )}
        {keys.map((k) => (
          <div className="row" key={k.id}>
            <Icon name="passkey" size={20} className="row-ic" />
            <div className="row-main">
              <p className="row-title">{k.nickname}</p>
              <p className="row-sub">
                Added {ago(k.created_at)} · {k.last_used_at ? `last used ${ago(k.last_used_at)}` : "not used yet"}
              </p>
            </div>
            <div className="row-end">
              <button
                type="button"
                className="btn-icon"
                disabled={lastKey}
                title={lastKey ? "Your last passkey can't be removed — you'd lock yourself out" : "Remove"}
                aria-label={`Remove passkey ${k.nickname}`}
                onClick={() => setConfirmDelete(k)}
              >
                <Icon name="remove" size={18} />
              </button>
            </div>
          </div>
        ))}
        <div className="row">
          <div className="row-main">
            <div className="settings-add">
              <PasskeyButton label="Add a passkey" onActivate={add} block={false} />
            </div>
            {status && (
              <p className="hint" data-error={status.error} role="status">
                {status.msg}
              </p>
            )}
          </div>
        </div>
      </div>

      <Modal
        open={!!confirmDelete}
        onClose={() => setConfirmDelete(null)}
        title="Remove this passkey?"
        danger
        footer={
          <>
            <Button variant="quiet" onClick={() => setConfirmDelete(null)}>
              Cancel
            </Button>
            <Button variant="danger-solid" disabled={busy} onClick={() => void remove()}>
              {busy ? "Removing…" : "Remove passkey"}
            </Button>
          </>
        }
      >
        <p className="dialog-lede">
          <strong>{confirmDelete?.nickname}</strong> won't sign you in any more. You can add it again later.
        </p>
      </Modal>
    </div>
  );
}

/**
 * Each computer's unlock code — the 6-digit code that unlocks its screen,
 * gives time back and allows `sudo` there, checked on the computer itself
 * with no internet. Read it here when you need it; recovery codes are the
 * spare keys.
 */
function UnlockCodes() {
  const devices = useAsync<Device[]>(listDevices, []);
  const list = devices.data ?? [];
  return (
    <div className="card">
      <div className="card-head">
        <div>
          <p className="row-title">Unlock codes</p>
          <p className="row-sub">
            One per computer. It unlocks the screen and gives time back, right there — even with no
            internet.
          </p>
        </div>
      </div>
      <div className="rows">
        {devices.error && (
          <div className="row">
            <p className="hint" data-error="true">
              Couldn't load the computers: {devices.error}
            </p>
          </div>
        )}
        {list.map((d) => (
          <div className="row settings-uc" key={d.id}>
            <div className="row-main">
              <UnlockCodePanel device={d} />
            </div>
          </div>
        ))}
        {!devices.loading && list.length === 0 && (
          <div className="row">
            <p className="row-sub">No computers yet. Each one gets an unlock code when you add it.</p>
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * A paired phone (Telegram): alerts, and a one-tap yes to a request for time.
 * Shown only when this server has a bot — otherwise there is nothing to do.
 */
function Phone() {
  const tg = useAsync<TelegramStatus>(getTelegram, []);
  const [pairing, setPairing] = useState<TelegramPairing | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ msg: string; error: boolean } | null>(null);

  // While the pairing sheet is open, watch for the pair to land; the sheet
  // closes by itself when it does.
  useEffect(() => {
    if (!pairing) return;
    const t = setInterval(() => tg.reload(), 3000);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pairing]);
  useEffect(() => {
    if (pairing && tg.data?.paired) {
      setPairing(null);
      setStatus({ msg: "Your phone is paired.", error: false });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tg.data?.paired]);

  async function begin() {
    setBusy(true);
    setStatus(null);
    try {
      setPairing(await pairTelegram());
    } catch (e) {
      setStatus({ msg: e instanceof Error ? e.message : "Couldn't start pairing.", error: true });
    } finally {
      setBusy(false);
    }
  }

  async function unpair() {
    setBusy(true);
    setStatus(null);
    try {
      await unpairTelegram();
      setStatus({ msg: "Your phone is no longer paired.", error: false });
      tg.reload();
    } catch (e) {
      setStatus({ msg: e instanceof Error ? e.message : "Couldn't unpair.", error: true });
    } finally {
      setBusy(false);
    }
  }

  const d = tg.data;
  if (!d?.configured) return null;
  return (
    <div className="card rows">
      <div className="row">
        <Icon name="bell" size={20} className="row-ic" />
        <div className="row-main">
          <p className="row-title">Phone</p>
          <p className="row-sub">
            {d.paired
              ? `Paired${d.username ? ` as @${d.username}` : ""}. Alerts and one-tap answers to requests go to your phone.`
              : "Pair your phone for alerts, and to answer a request for time with one tap."}
          </p>
          {status && (
            <p className="hint" data-error={status.error} role="status">
              {status.msg}
            </p>
          )}
        </div>
        <div className="row-end">
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => void (d.paired ? unpair() : begin())}>
            {d.paired ? "Unpair" : "Pair phone"}
          </Button>
        </div>
      </div>

      <Modal
        open={!!pairing}
        onClose={() => setPairing(null)}
        title="Pair your phone"
        footer={
          <Button variant="quiet" onClick={() => setPairing(null)}>
            Cancel
          </Button>
        }
      >
        {pairing && (
          <div className="stack">
            <p className="dialog-lede">
              Send this to the bot in Telegram. This closes by itself once your phone is paired. The
              code works for {pairing.expires_in_minutes} minutes.
            </p>
            {pairing.deep_link && (
              <a className="btn btn-primary btn-block" href={pairing.deep_link} target="_blank" rel="noreferrer">
                Open @{pairing.bot} in Telegram
              </a>
            )}
            <CopyField value={`/start ${pairing.code}`} />
          </div>
        )}
      </Modal>
    </div>
  );
}

/**
 * Older companion pairings. Nothing new is paired this way any more; the list
 * appears only while one is left, so it can be revoked.
 */
function Companions() {
  const tokens = useAsync<ParentToken[]>(listParentTokens, []);
  const live = (tokens.data ?? []).filter((t) => !t.revoked);
  const [status, setStatus] = useState<string | null>(null);

  async function revoke(t: ParentToken) {
    try {
      await revokeParentToken(t.id);
      tokens.setData((prev) => (prev ?? []).map((x) => (x.id === t.id ? { ...x, revoked: true } : x)));
      setStatus(`Revoked ${t.label || "that companion"}.`);
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "Couldn't revoke it.");
    }
  }

  if (live.length === 0 && !status) return null;
  return (
    <div className="card">
      <div className="card-head">
        <div>
          <p className="row-title">Paired companions</p>
          <p className="row-sub">Apps you paired earlier to answer requests. Revoke any you no longer use.</p>
        </div>
      </div>
      <div className="rows">
        {live.map((t) => (
          <div className="row" key={t.id}>
            <div className="row-main">
              <p className="row-title">{t.label || "Companion"}</p>
              <p className="row-sub">{t.last_used_at ? `Last used ${ago(t.last_used_at)}` : "Never used"}</p>
            </div>
            <div className="row-end">
              <Button size="sm" variant="danger" onClick={() => void revoke(t)}>
                Revoke
              </Button>
            </div>
          </div>
        ))}
        {status && (
          <div className="row">
            <p className="hint" role="status">
              {status}
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
