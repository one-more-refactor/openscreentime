// ============================================================================
// LOGIN — the front door, kept small on purpose (CONTRACT-0.6 §2).
//
// The primary way in: type your name, and your own computer asks "is this
// you?" — one click on its notification signs this browser in. No email, no
// password, no code. The PKCE verifier stays in this tab; the approval is
// worthless anywhere else.
//
// Everything else — the first parent's registration, the passkey fallback for
// phones and unmanaged browsers — lives behind one quiet disclosure. SSO is
// deliberately absent from the UI for now (the server still speaks it).
// ============================================================================
import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useSession } from "../lib/session";
import { ApiError } from "../api";
import { isEmail } from "../lib/validate";
import { Wordmark, PasskeyButton, TextInput, Button } from "../components";

type Phase = "idle" | "waiting";

export function Login() {
  const { login, register, deviceLogin, mock } = useSession();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [name, setName] = useState("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [matchCode, setMatchCode] = useState("");
  const [error, setError] = useState<string | null>(() =>
    params.get("error") ? "Sign-in failed — try again." : null,
  );

  // The fallback drawer: passkey sign-in, or the first parent registering.
  const [fallbackOpen, setFallbackOpen] = useState(false);
  const [fallbackMode, setFallbackMode] = useState<"login" | "register">("login");
  const [email, setEmail] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [emailError, setEmailError] = useState<string | null>(null);

  async function runDeviceLogin() {
    const who = name.trim();
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

  async function runPasskey() {
    if (!isEmail(email)) {
      setEmailError("Enter the email on the account, e.g. parent@example.com.");
      return;
    }
    setEmailError(null);
    setError(null);
    try {
      if (fallbackMode === "login") await login(email.trim());
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
        e instanceof Error && e.message ? e.message : "The passkey ceremony failed — try again.",
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
            <p style={{ color: "var(--fg-display)", fontWeight: 500 }}>
              Check your computer.
            </p>
            <p className="text-sm" style={{ color: "var(--fg-dim)" }}>
              A notification is asking whether this is you. Approve it only if
              it shows this code:
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
        ) : (
          <div className="flex flex-col gap-4">
            <TextInput
              label="Your name"
              value={name}
              autoComplete="username"
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && void runDeviceLogin()}
              placeholder="e.g. Mia"
            />
            <Button onClick={() => void runDeviceLogin()} disabled={!name.trim()}>
              Continue
            </Button>
            <p className="text-xs" style={{ color: "var(--fg-faint)" }}>
              Your own computer approves the sign-in — nothing to type, nothing
              to remember.
            </p>
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
          <Button
            variant="ghost"
            onClick={() => navigate("/devices", { replace: true })}
          >
            Enter design review (skip auth) →
          </Button>
        )}

        {phase === "idle" && (
          <div className="mt-10">
            <button
              type="button"
              className="focusable text-xs"
              style={{ color: "var(--fg-faint)", background: "none", border: "none", cursor: "pointer", padding: 0 }}
              onClick={() => setFallbackOpen((o) => !o)}
              aria-expanded={fallbackOpen}
            >
              {fallbackOpen ? "▾" : "▸"} No computer nearby? Passkey &amp; first-time setup
            </button>

            {fallbackOpen && (
              <div className="mt-4 flex flex-col gap-4">
                <div className="seg">
                  {(["login", "register"] as const).map((m) => (
                    <button
                      key={m}
                      type="button"
                      onClick={() => {
                        setFallbackMode(m);
                        setError(null);
                      }}
                      className="focusable seg-btn"
                      data-on={fallbackMode === m}
                    >
                      {m === "login" ? "Passkey" : "First parent"}
                    </button>
                  ))}
                </div>
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
                {fallbackMode === "register" && (
                  <TextInput
                    label="Display name"
                    value={displayName}
                    onChange={(e) => setDisplayName(e.target.value)}
                    placeholder="Parent"
                  />
                )}
                <PasskeyButton
                  label={fallbackMode === "login" ? "Continue with passkey" : "Create your passkey"}
                  onActivate={runPasskey}
                  disabled={!email}
                />
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
