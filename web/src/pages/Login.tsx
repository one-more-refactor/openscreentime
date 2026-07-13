import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useSession } from "../lib/session";
import { getAuthConfig } from "../api";
import { useAsync } from "../lib/useAsync";
import { isEmail } from "../lib/validate";
import { DotMatrix, PasskeyButton, TextInput, Button } from "../components";
import { LockOverlay } from "../components";
import type { AuthConfig } from "../types";

type Mode = "login" | "register";

const SSO_ERRORS: Record<string, string> = {
  sso_unknown_account:
    "That SSO account doesn't match any admin here. Sign in with a passkey, or ask the existing admin to invite you.",
};

export function Login() {
  const { login, register, mock } = useSession();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const authConfig = useAsync<AuthConfig>(getAuthConfig, []);
  const [mode, setMode] = useState<Mode>("login");
  const [email, setEmail] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [error, setError] = useState<string | null>(() => {
    const code = params.get("error");
    return code ? (SSO_ERRORS[code] ?? "Sign-in failed — try again.") : null;
  });
  const [emailError, setEmailError] = useState<string | null>(null);

  const oidc = authConfig.data?.oidc ?? false;
  const oidcName = authConfig.data?.oidc_name || "SSO";

  async function run() {
    if (!isEmail(email)) {
      setEmailError("Enter a valid email address, e.g. parent@example.com.");
      return;
    }
    setEmailError(null);
    setError(null);
    try {
      if (mode === "login") await login(email.trim());
      else await register(email.trim(), displayName.trim() || email.trim());
      navigate("/devices", { replace: true });
    } catch (e) {
      setError(
        e instanceof Error && e.message
          ? e.message
          : "The passkey ceremony failed — try again.",
      );
    }
  }

  return (
    <div className="min-h-screen grid lg:grid-cols-2">
      {/* Left: brand + form */}
      <div className="flex flex-col justify-center px-8 sm:px-16 py-12 max-w-lg w-full mx-auto">
        <div className="mb-2">
          <DotMatrix text="SENTINEL" dot={4} color="var(--fg)" />
        </div>
        <p className="label mb-10" style={{ color: "var(--fg-faint)" }}>
          ZERO-TRUST DEVICE MANAGEMENT
        </p>

        <div
          className="flex gap-1 mb-6 border rounded p-1 w-fit"
          style={{ borderColor: "var(--line)" }}
        >
          {(["login", "register"] as Mode[]).map((m) => (
            <button
              key={m}
              onClick={() => {
                setMode(m);
                setError(null);
              }}
              className="focusable px-4 py-1.5 rounded text-[0.625rem] font-mono uppercase tracking-label transition-colors"
              style={
                mode === m
                  ? { background: "var(--fg)", color: "var(--bg)" }
                  : { color: "var(--fg-dim)" }
              }
            >
              {m === "login" ? "SIGN IN" : "FIRST ADMIN"}
            </button>
          ))}
        </div>

        <div className="flex flex-col gap-4">
          <TextInput
            label="EMAIL"
            type="email"
            autoComplete="username webauthn"
            value={email}
            onChange={(e) => {
              setEmail(e.target.value);
              if (emailError) setEmailError(null);
            }}
            placeholder="parent@home.lan"
            aria-invalid={!!emailError}
            hint={emailError ?? undefined}
          />
          {mode === "register" && (
            <TextInput
              label="DISPLAY NAME"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="Parent"
            />
          )}

          <div className="mt-2">
            <PasskeyButton
              label={mode === "login" ? "CONTINUE WITH PASSKEY" : "REGISTER PASSKEY"}
              onActivate={run}
              disabled={!email}
            />
          </div>

          {oidc && (
            <a
              href="/api/auth/oidc/start"
              className="focusable w-full flex items-center justify-center gap-3 border rounded px-4 py-3 font-mono uppercase tracking-label text-xs text-fg transition-colors hover:border-fg"
              style={{ borderColor: "var(--line-2)" }}
            >
              <span className="led led-glow-ok" style={{ background: "var(--ok)" }} aria-hidden />
              CONTINUE WITH {oidcName.toUpperCase()}
            </a>
          )}

          {error && (
            <div
              className="flex items-start gap-2 border rounded px-3 py-2"
              style={{ borderColor: "var(--accent)" }}
              role="alert"
            >
              <span className="led led-glow-crit mt-1" style={{ background: "var(--accent)" }} />
              <span className="text-xs" style={{ color: "var(--accent)" }}>
                {error}
              </span>
            </div>
          )}

          {mock && (
            <p className="label" style={{ color: "var(--warn)" }}>
              DESIGN-REVIEW MODE (VITE_USE_MOCK=1) · PASSKEY PROMPTS MAY FAIL
            </p>
          )}

          {mock && (
            <Button
              variant="ghost"
              onClick={() => navigate("/devices", { replace: true })}
            >
              ENTER DESIGN-REVIEW (SKIP AUTH) →
            </Button>
          )}
        </div>

        <p className="label mt-12" style={{ color: "var(--fg-faint)" }}>
          PASSKEY-FIRST · NO PASSWORDS · WEBAUTHN
        </p>
      </div>

      {/* Right: dot-grid panel with the agent lock preview */}
      <div
        className="hidden lg:flex items-center justify-center p-12 dotgrid border-l"
        style={{ borderColor: "var(--line)", background: "var(--surface)" }}
      >
        <div className="w-full max-w-md">
          <p className="label mb-3" style={{ color: "var(--fg-faint)" }}>
            AGENT GUI — HOST INTERRUPTION
          </p>
          <LockOverlay mode="timesup" countdown="00:00" challenge="math" />
        </div>
      </div>
    </div>
  );
}
