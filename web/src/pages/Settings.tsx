// ============================================================================
// SETTINGS — two rooms with very different locks.
//
// The front room is harmless: who you are, how the app looks. It renders
// immediately, because reading is free.
//
// The back room — the computers' unlock codes, passkeys, the Telegram
// pairing, paired companions — is the set of levers that would let someone
// take the family over. It is not rendered, and its data is NOT EVEN FETCHED,
// until the person confirms it's them (a passkey, or a code on their own
// computer; a fresh sign-in counts): the server (docs/AUTH.md) answers these
// with 428 unless the session holds a live confirm window. The client gate is
// comfort; the server is the lock.
// ============================================================================
import { useEffect, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import {
  ApiError,
  addPasskey,
  deletePasskey,
  getAuthConfig,
  getTelegram,
  listDevices,
  listParentTokens,
  listPasskeys,
  mintParentToken,
  pairTelegram,
  revokeParentToken,
  unpairTelegram,
} from "../api";
import type {
  AuthConfig,
  Device,
  MintedParentToken,
  ParentToken,
  Passkey,
  TelegramPairing,
  TelegramStatus,
} from "../types";
import { useAsync } from "../lib/useAsync";
import { useSession } from "../lib/session";
import { useTheme, type ThemeMode } from "../lib/theme";
import { FluentSlider } from "../components/FluentSlider";
import { useConfirm } from "../lib/confirm";
import { Button, Modal, PasskeyButton, TokenBlock } from "../components";
import { UnlockCodePanel } from "../components/UnlockCodePanel";
import { LockGlyph } from "../layout/Shell";
import { PageHead } from "../layout/PageHead";
import { relTime } from "../lib/format";

export function Settings() {
  const { me, mock } = useSession();

  return (
    <div className="dev-wrap">
      <PageHead eyebrow="Settings" title="Your household, your rules." />

      <You />
      <Appearance />
      <Security />

      {mock && (
        <p className="rail-mock" style={{ marginTop: "2rem" }}>
          DESIGN-REVIEW MODE — MOCK DATA (VITE_USE_MOCK=1) · {me?.account?.email ?? ""}
        </p>
      )}
    </div>
  );
}

// ---- the front room --------------------------------------------------------

function You() {
  const { me, logout } = useSession();
  const navigate = useNavigate();

  async function handleLogout() {
    await logout();
    navigate("/login", { replace: true });
  }

  return (
    <section className="ch-section">
      <h2 className="ch-h2">You</h2>
      <div className="rl">
        <div className="rl-row">
          <div className="rl-what">
            <p className="rl-name">{me?.account?.display_name ?? me?.admin.display_name ?? "—"}</p>
            <p className="rl-value">
              {me?.admin.username ?? me?.account?.email ?? "—"} ·{" "}
              {me?.household?.name ?? me?.tenant.name ?? "your household"}
            </p>
          </div>
          <span className="rl-controls">
            <button className="ch-btn" onClick={() => void handleLogout()}>
              Log out
            </button>
          </span>
        </div>
      </div>
    </section>
  );
}

// The theme control is a three-stop slider: Light — Match my system — Dark.
// Dragging previews the theme live; the choice sticks on release.
const THEME_STOPS: { key: ThemeMode; label: string }[] = [
  { key: "light", label: "Light" },
  { key: "system", label: "Match my system" },
  { key: "dark", label: "Dark" },
];

function Appearance() {
  const { mode, setTheme, followSystem } = useTheme();

  function apply(idx: number, persist: boolean) {
    const stop = THEME_STOPS[idx]?.key ?? "system";
    if (stop === "system") {
      if (persist) followSystem();
      // Live preview of "system" = whatever the OS says right now.
      else setTheme(window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light", false);
    } else {
      setTheme(stop, persist);
    }
  }

  return (
    <section className="ch-section">
      <h2 className="ch-h2">Appearance</h2>
      <div className="rl">
        <div className="rl-row">
          <div className="rl-what">
            <p className="rl-name">Theme</p>
            <p className="rl-value">Both modes are first-class — slide to pick one, or let the OS decide</p>
          </div>
          <FluentSlider
            min={0}
            max={2}
            step={1}
            value={THEME_STOPS.findIndex((s) => s.key === mode)}
            format={(v) => THEME_STOPS[v]?.label ?? ""}
            onLive={(v) => apply(v, false)}
            onCommit={(v) => apply(v, true)}
            aria-label="Theme"
          />
        </div>
      </div>
    </section>
  );
}

// ---- the back room ---------------------------------------------------------

function Security() {
  const { enter, armed } = useConfirm();
  const [checking, setChecking] = useState(false);

  // The room is open exactly while the confirm window is — when it lapses,
  // the gate closes again by itself. No stale "unlocked" state to forget.
  async function unlock() {
    setChecking(true);
    try {
      await enter();
    } finally {
      setChecking(false);
    }
  }

  return (
    <section className="ch-section">
      <h2 className="ch-h2">Security &amp; access</h2>
      {armed ? (
        <SecurityPanels />
      ) : (
        <div className="gate card">
          <span className="gate-glyph" aria-hidden="true">
            <LockGlyph open={false} size={22} />
          </span>
          <p className="gate-title">Confirm it's you to see this</p>
          <p className="gate-sub">
            The computers' unlock codes, your passkeys and paired companions live here.
          </p>
          <button className="ch-btn ch-btn-yes" disabled={checking} onClick={() => void unlock()}>
            {checking ? "Checking…" : "Confirm it's you"}
          </button>
        </div>
      )}
    </section>
  );
}

/** Mounted only while confirmed — these fetches never fire on an idle visit. */
function SecurityPanels() {
  return (
    <div className="rl">
      <Passkeys />
      <UnlockCodes />
      <Telegram />
      <ParentAccess />
    </div>
  );
}

/**
 * The Telegram companion: pair once, then the phone gets alerts and can ok a
 * time request with one tap. It is not a way to sign in or confirm.
 */
function Telegram() {
  const tg = useAsync<TelegramStatus>(getTelegram, []);
  const [pairing, setPairing] = useState<TelegramPairing | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);

  // While the pairing sheet is open, watch for the bot to report the pair —
  // the moment it lands, the sheet closes itself.
  useEffect(() => {
    if (!pairing) return;
    const t = setInterval(() => tg.reload(), 3000);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pairing]);
  useEffect(() => {
    if (pairing && tg.data?.paired) {
      setPairing(null);
      setStatus("Phone paired ✓");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tg.data?.paired]);

  async function begin() {
    setBusy(true);
    setStatus(null);
    try {
      setPairing(await pairTelegram());
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "Couldn't start pairing.");
    } finally {
      setBusy(false);
    }
  }

  async function unpair() {
    setBusy(true);
    setStatus(null);
    try {
      await unpairTelegram();
      setStatus("Unpaired.");
      tg.reload();
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "Couldn't unpair.");
    } finally {
      setBusy(false);
    }
  }

  const d = tg.data;
  return (
    <div className="rl-row">
      <div className="rl-what">
        <p className="rl-name">Phone (Telegram)</p>
        <p className="rl-value">
          {tg.loading
            ? "Checking…"
            : !d?.configured
              ? "No bot on this server — set OST_TELEGRAM_BOT_TOKEN to enable phone taps"
              : d.paired
                ? `Paired${d.username ? ` as @${d.username}` : ""} — alerts and one-tap time approvals go to your phone`
                : "Pair your phone for alerts, and to ok a time request with one tap"}
        </p>
        {status && (
          <p className="dev-inline-status" role="status" style={{ marginTop: "0.35rem" }}>
            {status}
          </p>
        )}
      </div>
      <span className="rl-controls">
        {d?.configured && !tg.loading && (
          <button className="ch-btn" disabled={busy} onClick={() => void (d.paired ? unpair() : begin())}>
            {d.paired ? "Unpair" : "Pair phone"}
          </button>
        )}
      </span>

      <Modal
        open={!!pairing}
        onClose={() => setPairing(null)}
        title="Pair your phone"
        footer={
          <Button variant="ghost" onClick={() => setPairing(null)} disabled={busy}>
            Cancel
          </Button>
        }
      >
        {pairing && (
          <div className="flex flex-col gap-4">
            <p className="text-sm" style={{ color: "var(--fg-dim)" }}>
              Open Telegram and send this code to the bot — the sheet closes by
              itself once the pair lands. The code works for{" "}
              {pairing.expires_in_minutes} minutes.
            </p>
            {pairing.deep_link ? (
              <a
                className="focusable ch-btn ch-btn-yes"
                style={{ textAlign: "center" }}
                href={pairing.deep_link}
                target="_blank"
                rel="noreferrer"
              >
                Open @{pairing.bot} in Telegram
              </a>
            ) : (
              <p className="text-sm">
                Message your bot: <code>/start {pairing.code}</code>
              </p>
            )}
            <TokenBlock token={`/start ${pairing.code}`} />
          </div>
        )}
      </Modal>
    </div>
  );
}

/**
 * Each computer's unlock code — the 6-digit code that unlocks its screen,
 * reopens time and allows `sudo` there, verified on the device with no
 * internet. The secret behind it stays on the server: a parent reads the code
 * here when they need it. Recovery codes and replacing the key live in the
 * same row.
 */
function UnlockCodes() {
  const devices = useAsync<Device[]>(listDevices, []);
  const list = devices.data ?? [];

  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what">
        <p className="rl-name">Unlock codes</p>
        <p className="rl-value">
          One per computer. The 6-digit code unlocks the screen, reopens time and allows{" "}
          <code>sudo</code> there — verified on the device, offline. Read it here on your phone
          when you need it; no authenticator app involved.
          {devices.error ? ` · couldn't load: ${devices.error}` : ""}
        </p>
      </div>
      {list.map((d) => (
        <UnlockCodePanel key={d.id} device={d} />
      ))}
      {!devices.loading && list.length === 0 && (
        <p className="fam-quiet">No computers yet — an unlock code is made when you set one up.</p>
      )}
    </div>
  );
}

function Passkeys() {
  const recovered = (useLocation().state as { recovered?: boolean } | null)?.recovered === true;
  const authConfig = useAsync<AuthConfig>(getAuthConfig, []);
  const passkeys = useAsync<Passkey[]>(listPasskeys, []);
  const [confirmDelete, setConfirmDelete] = useState<Passkey | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);

  const oidc = authConfig.data?.oidc ?? false;
  const keys = passkeys.data ?? [];
  const lastKey = keys.length <= 1 && !oidc;

  async function add() {
    setStatus(null);
    try {
      await addPasskey();
      passkeys.reload();
      setStatus("Passkey added.");
    } catch (e) {
      if (e instanceof Error && (e.name === "NotAllowedError" || e.name === "AbortError")) return;
      setStatus(e instanceof Error ? e.message : "The passkey wasn't added.");
    }
  }

  async function remove() {
    if (!confirmDelete) return;
    const key = confirmDelete;
    setBusy(true);
    try {
      await deletePasskey(key.id);
      passkeys.setData((prev) => (prev ?? []).filter((k) => k.id !== key.id));
      setStatus(`Passkey "${key.nickname}" removed.`);
    } catch (e) {
      setStatus(
        e instanceof ApiError && e.status === 409
          ? "That's your last passkey — removing it would lock you out."
          : e instanceof Error
            ? e.message
            : "Couldn't remove the passkey.",
      );
    } finally {
      setBusy(false);
      setConfirmDelete(null);
    }
  }

  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what">
        <p className="rl-name">Passkeys</p>
        <p className="rl-value">
          {recovered
            ? "You signed in with a recovery link. Add a passkey now, so next time is one tap."
            : "One tap to sign in — one per phone or computer you trust"}
          {passkeys.error ? ` · couldn't load: ${passkeys.error}` : ""}
        </p>
        {status && <p className="dev-inline-status" role="status" style={{ marginTop: "0.35rem" }}>{status}</p>}
      </div>
      {keys.map((k) => (
        <div className="rl-app" key={k.id}>
          <span className="rl-app-name">{k.nickname}</span>
          <span className="rl-app-mins">
            added {relTime(k.created_at)} · used {relTime(k.last_used_at)}
          </span>
          <button
            className="chip-x"
            disabled={lastKey}
            title={lastKey ? "Your last passkey can't be removed — you'd lock yourself out" : undefined}
            aria-label={`Remove passkey ${k.nickname}`}
            onClick={() => setConfirmDelete(k)}
          >
            ✕
          </button>
        </div>
      ))}
      <div style={{ maxWidth: "16rem" }}>
        <PasskeyButton label="Add a passkey" onActivate={add} />
      </div>

      <Modal
        open={!!confirmDelete}
        onClose={() => setConfirmDelete(null)}
        title="Remove passkey"
        danger
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmDelete(null)}>
              CANCEL
            </Button>
            <Button variant="danger" disabled={busy} onClick={() => void remove()}>
              {busy ? "Removing…" : "Remove passkey"}
            </Button>
          </>
        }
      >
        <p className="text-xs leading-relaxed" style={{ color: "var(--fg-dim)" }}>
          Remove <span className="dot text-fg">{confirmDelete?.nickname}</span>? Devices that
          signed in with it will need another way back in.
        </p>
      </Modal>
    </div>
  );
}

function ParentAccess() {
  const parentTokens = useAsync<ParentToken[]>(listParentTokens, []);
  const [label, setLabel] = useState("");
  const [minting, setMinting] = useState(false);
  const [minted, setMinted] = useState<MintedParentToken | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  async function mint() {
    setMinting(true);
    setStatus(null);
    try {
      setMinted(await mintParentToken(label.trim()));
      setLabel("");
      parentTokens.reload();
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "Couldn't create the pairing token.");
    } finally {
      setMinting(false);
    }
  }

  async function revoke(t: ParentToken) {
    try {
      await revokeParentToken(t.id);
      parentTokens.setData((prev) =>
        (prev ?? []).map((x) => (x.id === t.id ? { ...x, revoked: true } : x)),
      );
      setStatus(`Revoked "${t.label || "pairing token"}".`);
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "Couldn't revoke the token.");
    }
  }

  const tokens = parentTokens.data ?? [];

  return (
    <div className="rl-row rl-row-stack">
      <div className="rl-what">
        <p className="rl-name">Paired companions</p>
        <p className="rl-value">
          Your phone or tray app, approving requests without opening the console — tokens are
          shown once and stored hashed
        </p>
        {status && <p className="dev-inline-status" role="status" style={{ marginTop: "0.35rem" }}>{status}</p>}
      </div>
      {tokens.map((t) => (
        <div className="rl-app" key={t.id}>
          <span className="rl-app-name" style={t.revoked ? { color: "var(--fg-faint)", textDecoration: "line-through" } : undefined}>
            {t.label || "Pairing token"}
          </span>
          <span className="rl-app-mins">
            {t.revoked
              ? "revoked"
              : t.last_used_at
                ? `last used ${relTime(t.last_used_at)}`
                : "never used"}
          </span>
          {!t.revoked && (
            <button className="chip-x" aria-label={`Revoke ${t.label || "pairing token"}`} onClick={() => void revoke(t)}>
              ✕
            </button>
          )}
        </div>
      ))}
      <div className="rl-app">
        <input
          className="chip-input"
          style={{ width: "14rem" }}
          placeholder="+ companion, e.g. Mum's phone"
          value={label}
          disabled={minting}
          onChange={(e) => setLabel(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && label.trim() && void mint()}
          aria-label="New companion label"
        />
        {label.trim() && (
          <button className="ch-btn" disabled={minting} onClick={() => void mint()}>
            {minting ? "Creating…" : "Create pairing token"}
          </button>
        )}
      </div>

      <Modal
        open={!!minted}
        onClose={() => setMinted(null)}
        title="Pairing token"
        footer={<Button onClick={() => setMinted(null)}>Done</Button>}
      >
        <p className="text-xs leading-relaxed mb-3" style={{ color: "var(--fg-dim)" }}>
          Copy this now — it's shown only once. Paste it into the companion for{" "}
          <span className="dot text-fg">{minted?.label || "this pairing"}</span>.
        </p>
        {minted && <TokenBlock token={minted.token} />}
      </Modal>
    </div>
  );
}
