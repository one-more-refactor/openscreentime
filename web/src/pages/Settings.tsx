// ============================================================================
// SETTINGS — two rooms with very different locks.
//
// The front room is harmless: who you are, how the app looks. It renders
// immediately, because reading is free.
//
// The back room — the computers' unlock codes, passkeys, second factors,
// paired companions — is the set of levers that would let someone take the
// family over. It is not rendered, and its data is NOT EVEN FETCHED, until
// the person confirms it's them: the fetches run only then, and the server
// (docs/AUTH.md) answers them with 428 unless the session holds a live
// confirm window. The client gate is comfort; the server is the lock.
// ============================================================================
import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  ApiError,
  auth,
  confirmTotpEnrollment,
  deletePasskey,
  getAuthConfig,
  getTelegram,
  getTwoFactorStatus,
  listDevices,
  listParentTokens,
  listPasskeys,
  mintParentToken,
  pairTelegram,
  revokeParentToken,
  startTotpEnrollment,
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
  TotpEnrollment,
  TwoFactorStatus,
} from "../types";
import { QrCode } from "../components/QrCode";
import { useAsync } from "../lib/useAsync";
import { useSession } from "../lib/session";
import { useTheme, type ThemeMode } from "../lib/theme";
import { FluentSlider } from "../components/FluentSlider";
import { useConfirm } from "../lib/confirm";
import { Button, Modal, PasskeyButton, TokenBlock } from "../components";
import { CodeRing } from "../components/CodeRing";
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
              {me?.account?.email ?? me?.admin.email ?? "—"} ·{" "}
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
            The computers' unlock codes, your passkeys, second factors and paired companions
            live here. The server only hands them to a session that has proved it's you.
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
      <UnlockCodes />
      <TwoFactor />
      <Telegram />
      <Passkeys />
      <ParentAccess />
    </div>
  );
}

/**
 * The Telegram companion: pair once, then the phone gets alerts, can ok a
 * time request with one tap, and answers the confirm dialog's "send a tap".
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
                ? `Paired${d.username ? ` as @${d.username}` : ""} — alerts, one-tap chore approvals, and confirm checks go to your phone`
                : "Pair your phone: get alerts, ok a chore, and confirm it's you with one tap"}
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

function TwoFactor() {
  const { me } = useSession();
  const twofa = useAsync<TwoFactorStatus>(getTwoFactorStatus, []);
  const [enrolling, setEnrolling] = useState<TotpEnrollment | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);

  async function begin() {
    setBusy(true);
    setStatus(null);
    try {
      setEnrolling(await startTotpEnrollment());
      setCode("");
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "Couldn't start enrollment.");
    } finally {
      setBusy(false);
    }
  }

  async function confirm() {
    setBusy(true);
    setStatus(null);
    try {
      await confirmTotpEnrollment(code);
      setEnrolling(null);
      setStatus("Authenticator connected.");
      twofa.reload();
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "That code didn't match.");
      setCode("");
    } finally {
      setBusy(false);
    }
  }

  const enrolled = twofa.data?.totp_enrolled ?? false;

  return (
    <div className="rl-row">
      <div className="rl-what">
        <p className="rl-name">Second factor</p>
        <p className="rl-value">
          {twofa.loading
            ? "Checking…"
            : enrolled
              ? "Authenticator app connected · email codes as backup"
              : "Email codes only — an authenticator app is stronger"}
        </p>
        {status && <p className="dev-inline-status" role="status" style={{ marginTop: "0.35rem" }}>{status}</p>}
      </div>
      <span className="rl-controls">
        {!enrolled && !twofa.loading && (
          <button className="ch-btn" disabled={busy} onClick={() => void begin()}>
            Connect authenticator
          </button>
        )}
      </span>

      <Modal
        open={!!enrolling}
        onClose={() => setEnrolling(null)}
        title="Connect authenticator"
        footer={
          <Button variant="ghost" onClick={() => setEnrolling(null)} disabled={busy}>
            CANCEL
          </Button>
        }
      >
        <div className="flex flex-col gap-4">
          <p className="text-sm" style={{ color: "var(--fg-dim)" }}>
            Add this secret to your authenticator app (Google Authenticator, Aegis, 1Password …),
            then enter the 6-digit code it shows for{" "}
            <span style={{ color: "var(--fg)" }}>{me?.account?.email ?? "your account"}</span>.
          </p>
          {enrolling && (
            <div className="tf-enrol">
              <QrCode value={enrolling.otpauth_uri} size={148} label="Scan into your authenticator app" />
              <div className="tf-enrol-text">
                <p className="add-secret-label">Can't scan? Type the secret</p>
                <TokenBlock token={enrolling.secret} />
              </div>
            </div>
          )}
          <div className="cr-wrap">
            <CodeRing
              value={code}
              disabled={busy}
              error={!!status && !!enrolling}
              aria-label="Code from the app"
              onChange={(v) => {
                setCode(v);
                if (status) setStatus(null);
              }}
              onComplete={() => void confirm()}
            />
            <p className="cr-note" data-error={!!status && !!enrolling} role={status ? "alert" : undefined}>
              {busy ? "Checking…" : (status ?? "The 6 digits the app shows")}
            </p>
          </div>
        </div>
      </Modal>
    </div>
  );
}

function Passkeys() {
  const { me, refresh } = useSession();
  const authConfig = useAsync<AuthConfig>(getAuthConfig, []);
  const passkeys = useAsync<Passkey[]>(listPasskeys, []);
  const [confirmDelete, setConfirmDelete] = useState<Passkey | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);

  const oidc = authConfig.data?.oidc ?? false;
  const keys = passkeys.data ?? [];
  const lastKey = keys.length <= 1 && !oidc;

  async function add() {
    if (!me) return;
    setStatus(null);
    try {
      await auth.register(me.admin.email, me.admin.display_name);
      await refresh();
      passkeys.reload();
      setStatus("Passkey added.");
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "Passkey registration failed.");
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
          How you sign in — one per device you trust
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
        <PasskeyButton label="+ ADD PASSKEY" onActivate={add} />
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
              {busy ? "REMOVING…" : "REMOVE PASSKEY"}
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
        footer={<Button onClick={() => setMinted(null)}>DONE</Button>}
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
