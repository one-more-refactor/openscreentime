import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useSession } from "../lib/session";
import { PasskeyButton, TextInput, Button } from "../components";
import { LockOverlay } from "../components";

type Mode = "login" | "register";

export function Login() {
  const { login, register, mock } = useSession();
  const navigate = useNavigate();
  const [mode, setMode] = useState<Mode>("login");
  const [email, setEmail] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function run() {
    setError(null);
    try {
      if (mode === "login") await login(email);
      else await register(email, displayName || email);
      navigate("/devices", { replace: true });
    } catch (e) {
      setError(
        e instanceof Error ? e.message : "PASSKEY CEREMONY FAILED — TRY AGAIN",
      );
    }
  }

  return (
    <div className="min-h-screen grid lg:grid-cols-2">
      {/* Left: brand + form */}
      <div className="flex flex-col justify-center px-8 sm:px-16 py-12 max-w-lg w-full mx-auto">
        <div className="flex items-center gap-3 mb-2">
          <span className="led led-glow-crit led-pulse" style={{ background: "var(--accent)" }} />
          <span className="wordmark text-2xl text-fg">SENTINEL</span>
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
            onChange={(e) => setEmail(e.target.value)}
            placeholder="parent@home.lan"
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

          {error && (
            <div
              className="flex items-center gap-2 border rounded px-3 py-2"
              style={{ borderColor: "var(--accent)" }}
            >
              <span className="led led-glow-crit" style={{ background: "var(--accent)" }} />
              <span className="text-xs" style={{ color: "var(--accent)" }}>
                {error}
              </span>
            </div>
          )}

          {mock && (
            <p className="label" style={{ color: "var(--warn)" }}>
              BACKEND OFFLINE · ANY PASSKEY PROMPT MAY FAIL — DESIGN-REVIEW MODE
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
          PASSKEY-ONLY · NO PASSWORDS · WEBAUTHN
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
