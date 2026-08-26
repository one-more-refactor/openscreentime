import { useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useSession } from "../lib/session";
import { ApiError, getAuthConfig } from "../api";
import { useAsync } from "../lib/useAsync";
import { isEmail } from "../lib/validate";
import { Wordmark, PasskeyButton, TextInput, Button } from "../components";
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
      navigate("/", { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.code === "registration_closed") {
        setError(
          "Registration is closed on this server — an admin already exists. " +
            "Sign in instead, or ask the existing admin for access.",
        );
        return;
      }
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
          <Wordmark size={2} />
        </div>
        <p className="mb-10 text-sm" style={{ color: "var(--fg-dim)" }}>
          Screen time for the whole family.
        </p>

        <div className="seg mb-6">
          {(["login", "register"] as Mode[]).map((m) => (
            <button
              key={m}
              type="button"
              onClick={() => {
                setMode(m);
                setError(null);
              }}
              className="focusable seg-btn"
              data-on={mode === m}
            >
              {m === "login" ? "Sign in" : "First parent"}
            </button>
          ))}
        </div>

        <div className="flex flex-col gap-4">
          <TextInput
            label="Email"
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
              label="Display name"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="Parent"
            />
          )}

          <div className="mt-2">
            <PasskeyButton
              label={mode === "login" ? "Continue with passkey" : "Create your passkey"}
              onActivate={run}
              disabled={!email}
            />
          </div>

          {oidc && (
            <a
              href="/api/auth/oidc/start"
              className="focusable w-full flex items-center justify-center gap-3 border rounded px-4 py-3 text-sm transition-colors hover:border-fg"
              style={{ borderColor: "var(--line-2)", color: "var(--fg)" }}
            >
              <span className="led led-glow-ok" style={{ background: "var(--ok)" }} aria-hidden />
              Continue with {oidcName}
            </a>
          )}

          {/* The default way in on a managed machine: the installed client
              vouches for whoever is logged into the computer — parent and
              child alike. Trust lives at sign-in; there is nothing to arm
              once you are inside. */}
          <div
            className="rounded border px-4 py-3 text-sm"
            style={{ borderColor: "var(--line)", color: "var(--fg-dim)" }}
          >
            <p style={{ color: "var(--fg)", marginBottom: "0.25rem" }}>
              On a computer that runs OpenScreenTime?
            </p>
            Run <code>ost login</code> there — the installed client signs you in
            as you, parent or child. No password, no phone.
          </div>

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

        <p className="mt-12 text-xs" style={{ color: "var(--fg-faint)" }}>
          Trust lives at sign-in: a passkey, your own computer, or SSO. Never a password.
        </p>
      </div>

      {/* Right: the ring, not the lock — the first thing anyone sees of this
          product is their day at a glance, not a punishment screen. */}
      <div
        className="hidden lg:flex items-center justify-center p-12 dotgrid border-l"
        style={{ borderColor: "var(--line)", background: "var(--surface)" }}
      >
        <div className="w-full max-w-md flex flex-col items-center">
          <HeroRing />
          <p className="mt-6 text-sm" style={{ color: "var(--fg-dim)" }}>
            Everyone's day, at a glance.
          </p>
        </div>
      </div>
    </div>
  );
}

/** The activity ring the console is built around, drawing itself on arrival. */
function HeroRing() {
  const size = 240;
  const stroke = 12;
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  const pct = 0.7; // a good day, most of it still ahead
  const [drawn, setDrawn] = useState(false);
  useEffect(() => {
    const t = requestAnimationFrame(() => setDrawn(true));
    return () => cancelAnimationFrame(t);
  }, []);
  return (
    <div style={{ width: size, height: size, position: "relative" }} aria-hidden="true">
      <svg viewBox={`0 0 ${size} ${size}`} width={size} height={size}>
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          strokeWidth={stroke}
          fill="none"
          stroke="var(--line)"
        />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          strokeWidth={stroke}
          fill="none"
          stroke="var(--ok)"
          strokeLinecap="round"
          strokeDasharray={c}
          strokeDashoffset={drawn ? c * (1 - pct) : c}
          transform={`rotate(-90 ${size / 2} ${size / 2})`}
          style={{ transition: "stroke-dashoffset 900ms var(--ease)" }}
        />
      </svg>
      <div
        style={{
          position: "absolute",
          inset: 0,
          display: "grid",
          placeItems: "center",
          textAlign: "center",
        }}
      >
        <div>
          <p style={{ fontSize: "2.6rem", fontWeight: 600, color: "var(--fg-display)", lineHeight: 1 }}>
            1h 24
          </p>
          <p className="text-xs" style={{ color: "var(--fg-dim)", marginTop: "0.35rem" }}>
            left today
          </p>
        </div>
      </div>
    </div>
  );
}
