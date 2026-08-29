// ============================================================================
// LOGIN / FIRST-RUN REGISTRATION — the front door, kept small on purpose.
//
// Fresh install (no account yet): a single passkey-only registration — pick a
// username, create your passkey. That is the ONLY option; there is no email,
// no password, no code, and nothing third-party.
//
// Otherwise (login): type your username and your own computer asks "is this
// you?" — one tap on its notification signs this browser in (the client-code /
// number-match flow). Beneath it, a small "Log in with passkey" for phones and
// unmanaged browsers. SSO stays available when the server has it configured.
// ============================================================================
import { useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useSession } from "../lib/session";
import { ApiError, getAuthConfig } from "../api";
import type { AuthConfig } from "../types";
import { Wordmark, PasskeyButton, TextInput, Button } from "../components";

type Phase = "idle" | "waiting";

const USERNAME_RE = /^[a-z0-9._-]{3,32}$/;

export function Login() {
  const { login, register, deviceLogin, mock } = useSession();
  const navigate = useNavigate();
  const [params] = useSearchParams();

  const [config, setConfig] = useState<AuthConfig | null>(null);
  const [username, setUsername] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [userError, setUserError] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [matchCode, setMatchCode] = useState("");
  const [error, setError] = useState<string | null>(() =>
    params.get("error") ? "Sign-in failed — try again." : null,
  );

  useEffect(() => {
    let alive = true;
    getAuthConfig()
      .then((c) => alive && setConfig(c))
      .catch(() => alive && setConfig({ oidc: false, oidc_name: "SSO", needs_setup: false }));
    return () => {
      alive = false;
    };
  }, []);

  const registering = config?.needs_setup === true;

  function validUsername(): string | null {
    const u = username.trim().toLowerCase();
    if (!USERNAME_RE.test(u)) {
      setUserError("3–32 characters: a–z, 0–9, dot, underscore, hyphen.");
      return null;
    }
    setUserError(null);
    return u;
  }

  // LOGIN: type your username → your own computer approves (number match).
  async function runDeviceLogin() {
    const who = username.trim();
    if (!who) return;
    setError(null);
    setPhase("waiting");
    try {
      await deviceLogin(who, setMatchCode);
      navigate("/", { replace: true });
    } catch (e) {
      setError(e instanceof Error ? e.message : "That didn't work — try again.");
      setPhase("idle");
      setMatchCode("");
    }
  }

  // LOGIN fallback: passkey for this username.
  async function runPasskeyLogin() {
    const who = username.trim();
    if (!who) {
      setUserError("Enter your username first.");
      return;
    }
    setError(null);
    try {
      await login(who);
      navigate("/", { replace: true });
    } catch (e) {
      setError(
        e instanceof Error && e.message ? e.message : "The passkey didn't match — try again.",
      );
    }
  }

  // FIRST-RUN: passkey-only account creation.
  async function runRegister() {
    const u = validUsername();
    if (!u) return;
    setError(null);
    try {
      await register(u, displayName.trim() || undefined);
      navigate("/", { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.code === "registration_closed") {
        setError("An account already exists on this server — sign in instead.");
        return;
      }
      if (e instanceof ApiError && e.status === 409) {
        setUserError("That username is taken — pick another.");
        return;
      }
      setError(
        e instanceof Error && e.message ? e.message : "Creating your passkey failed — try again.",
      );
    }
  }

  return (
    <div className="min-h-screen flex items-center justify-center px-6">
      <div className="w-full max-w-sm">
        <div className="mb-2">
          <Wordmark size={2} />
        </div>
        <p className="mb-10 text-sm" style={{ color: "var(--fg-dim)" }}>
          Screen time for the whole family.
        </p>

        {phase === "waiting" ? (
          <div className="flex flex-col gap-4" role="status" aria-live="polite">
            <p style={{ color: "var(--fg-display)", fontWeight: 500 }}>Check your computer.</p>
            <p className="text-sm" style={{ color: "var(--fg-dim)" }}>
              A notification on your computer is showing three numbers. Tap the one that matches
              this:
            </p>
            <p
              style={{
                fontSize: "2.4rem",
                fontWeight: 600,
                letterSpacing: "0.3em",
                color: "var(--fg-display)",
                fontVariantNumeric: "tabular-nums",
                textAlign: "center",
              }}
            >
              {matchCode}
            </p>
            <span className="login-wait-bar" aria-hidden="true" />
            <Button variant="ghost" onClick={() => window.location.reload()}>
              Cancel
            </Button>
          </div>
        ) : registering ? (
          // ---- First-run registration: passkey only, the only option. ----
          <div className="flex flex-col gap-4">
            <p style={{ color: "var(--fg-display)", fontWeight: 500 }}>Create the first account.</p>
            <TextInput
              label="Username"
              value={username}
              autoComplete="username webauthn"
              onChange={(e) => {
                setUsername(e.target.value);
                if (userError) setUserError(null);
              }}
              onKeyDown={(e) => e.key === "Enter" && void runRegister()}
              placeholder="e.g. dad"
              aria-invalid={!!userError}
              hint={userError ?? "This is how you'll sign in. No email, ever."}
            />
            <TextInput
              label="Display name (optional)"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="Parent"
            />
            <PasskeyButton label="Create account" onActivate={runRegister} disabled={!username.trim()} />
          </div>
        ) : (
          // ---- Login: username → your computer approves; passkey beneath. ----
          <div className="flex flex-col gap-4">
            <TextInput
              label="Username"
              value={username}
              autoComplete="username webauthn"
              onChange={(e) => {
                setUsername(e.target.value);
                if (userError) setUserError(null);
              }}
              onKeyDown={(e) => e.key === "Enter" && void runDeviceLogin()}
              placeholder="e.g. dad"
              aria-invalid={!!userError}
              hint={userError ?? undefined}
            />
            <Button onClick={() => void runDeviceLogin()} disabled={!username.trim()}>
              Continue
            </Button>
            <p className="text-xs" style={{ color: "var(--fg-faint)" }}>
              Your own computer approves the sign-in — nothing to type, nothing to remember.
            </p>
            <button
              type="button"
              className="focusable text-xs"
              style={{
                color: "var(--fg-faint)",
                background: "none",
                border: "none",
                cursor: "pointer",
                padding: "0.25rem 0",
                textAlign: "left",
              }}
              onClick={() => void runPasskeyLogin()}
            >
              Log in with passkey →
            </button>
            {config?.oidc && (
              <button
                type="button"
                className="focusable text-xs"
                style={{
                  color: "var(--fg-faint)",
                  background: "none",
                  border: "none",
                  cursor: "pointer",
                  padding: "0.25rem 0",
                  textAlign: "left",
                }}
                onClick={() => {
                  window.location.href = "/api/auth/oidc/start";
                }}
              >
                Sign in with {config.oidc_name} →
              </button>
            )}
          </div>
        )}

        {error && (
          <div
            className="mt-4 flex items-start gap-2 border rounded px-3 py-2"
            style={{ borderColor: "var(--accent)" }}
            role="alert"
          >
            <span className="text-xs" style={{ color: "var(--accent)" }}>
              {error}
            </span>
          </div>
        )}

        {mock && (
          <Button variant="ghost" onClick={() => navigate("/devices", { replace: true })}>
            Enter design review (skip auth) →
          </Button>
        )}
      </div>
    </div>
  );
}
