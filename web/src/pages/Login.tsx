// ============================================================================
// SIGN IN — two doors, nothing else (docs/AUTH.md).
//
//   Your name → Continue → a 6-digit code shows up on your own computer →
//   type it here.
//   Sign in with a passkey → one tap, no name first.
//   (Sign in with SSO — only when the server has it.)
//
// A fresh server shows "Create your household" instead: your name, then a
// passkey. The setup link the installer printed carries the one-time setup
// code in its fragment (#setup=…); only without it does a code field appear.
// ============================================================================
import { useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useSession } from "../lib/session";
import { takeFromFragment } from "../lib/fragment";
import { ApiError, getAuthConfig } from "../api";
import type { AuthConfig } from "../types";
import { Wordmark } from "../components/Wordmark";
import { PasskeyButton } from "../components/PasskeyButton";
import { TextInput } from "../components/TextInput";
import { Button } from "../components/Button";
import { CodeRing } from "../components/CodeRing";
import { sentence } from "../lib/format";

const SETUP_KEY = "ost-setup";

/** The setup code from the installer's link — kept for this tab only, so a
 * reload mid-setup doesn't lose it. */
function takeSetupToken(): string {
  const fromLink = takeFromFragment("setup");
  try {
    if (fromLink) sessionStorage.setItem(SETUP_KEY, fromLink);
    return fromLink ?? sessionStorage.getItem(SETUP_KEY) ?? "";
  } catch {
    return fromLink ?? "";
  }
}

function forgetSetupToken() {
  try {
    sessionStorage.removeItem(SETUP_KEY);
  } catch {
    /* nothing kept */
  }
}

/** The browser's passkey prompt was dismissed — not an error worth shouting. */
function dismissed(e: unknown): boolean {
  return e instanceof Error && (e.name === "NotAllowedError" || e.name === "AbortError");
}

type Step = "name" | "code";

export function Login() {
  const { createHousehold, signInWithPasskey, sendCode, enterCode } = useSession();
  const navigate = useNavigate();
  const [params] = useSearchParams();

  const [config, setConfig] = useState<AuthConfig | null>(null);
  const [setupToken, setSetupToken] = useState<string>(takeSetupToken);
  const [askSetupCode, setAskSetupCode] = useState(false);
  const [name, setName] = useState("");
  const [step, setStep] = useState<Step>("name");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(() =>
    params.get("error") ? "That sign-in didn't work. Try again." : null,
  );

  useEffect(() => {
    let alive = true;
    getAuthConfig()
      .then((c) => alive && setConfig(c))
      .catch(
        () =>
          alive &&
          setConfig({ oidc: false, oidc_name: "SSO", needs_setup: false, setup_code_required: false }),
      );
    return () => {
      alive = false;
    };
  }, []);

  const firstRun = config?.needs_setup === true;
  const showSetupCode = firstRun && (askSetupCode || (config?.setup_code_required && !setupToken));

  async function create() {
    if (!name.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await createHousehold(name.trim(), setupToken || undefined);
      forgetSetupToken();
      navigate("/", { replace: true });
    } catch (e) {
      if (dismissed(e)) {
        setError("No passkey was made. Try again when you're ready.");
      } else if (e instanceof ApiError && e.code === "registration_closed") {
        setError("This server already has a household. Sign in instead.");
        setConfig((c) => (c ? { ...c, needs_setup: false } : c));
      } else if (e instanceof ApiError && e.status === 401) {
        setAskSetupCode(true);
        setError("That setup code isn't right. Open the link the installer printed, or type the code.");
      } else {
        setError(e instanceof Error && e.message ? sentence(e.message) : "That didn't work. Try again.");
      }
    } finally {
      setBusy(false);
    }
  }

  async function askForCode() {
    if (!name.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await sendCode(name.trim());
      setCode("");
      setStep("code");
    } catch (e) {
      setError(e instanceof Error && e.message ? sentence(e.message) : "That didn't work. Try again.");
    } finally {
      setBusy(false);
    }
  }

  async function verify(full: string) {
    setBusy(true);
    setError(null);
    try {
      await enterCode(full);
      navigate("/", { replace: true });
    } catch (e) {
      setCode("");
      setError(
        e instanceof ApiError && e.code === "code_expired"
          ? "That code has run out. Ask for a new one."
          : e instanceof Error && e.message
            ? sentence(e.message)
            : "That code didn't match. Try again.",
      );
    } finally {
      setBusy(false);
    }
  }

  async function passkey() {
    setError(null);
    try {
      await signInWithPasskey();
      navigate("/", { replace: true });
    } catch (e) {
      setError(
        dismissed(e)
          ? null
          : e instanceof Error && e.message
            ? sentence(e.message)
            : "That passkey didn't work. Try again.",
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

        {!config ? null : firstRun ? (
          // ---- First run: your name, then a passkey. ----
          <form
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              void create();
            }}
          >
            <p style={{ color: "var(--fg-display)", fontWeight: 500 }}>Create your household</p>
            <TextInput
              label="Your name"
              name="name"
              autoComplete="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoFocus
            />
            {showSetupCode && (
              <TextInput
                label="Setup code"
                name="setup-code"
                autoComplete="off"
                spellCheck={false}
                value={setupToken}
                onChange={(e) => setSetupToken(e.target.value.trim())}
                hint="It's in the link the installer printed."
              />
            )}
            <Button type="submit" className="w-full" disabled={!name.trim() || busy}>
              {busy ? "Waiting for your passkey…" : "Create passkey"}
            </Button>
          </form>
        ) : step === "code" ? (
          // ---- Door one, part two: the code from your computer. ----
          <div className="flex flex-col gap-4">
            <p style={{ color: "var(--fg-display)", fontWeight: 500 }}>
              Enter the code from your computer
            </p>
            <p className="text-sm" style={{ color: "var(--fg-dim)" }}>
              It's in the OpenScreenTime window on your computer.
            </p>
            <div className="cr-wrap">
              <CodeRing
                value={code}
                disabled={busy}
                error={!!error}
                aria-label="The code from your computer"
                onChange={(v) => {
                  setCode(v);
                  if (error) setError(null);
                }}
                onComplete={(full) => void verify(full)}
              />
              <p className="cr-note" data-error={!!error} role={error ? "alert" : "status"}>
                {busy ? "Checking…" : (error ?? "It works for 5 minutes.")}
              </p>
            </div>
            <div className="flex justify-between">
              <Button variant="ghost" size="sm" onClick={() => void askForCode()} disabled={busy}>
                Send a new code
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  setStep("name");
                  setError(null);
                }}
              >
                Back
              </Button>
            </div>
          </div>
        ) : (
          // ---- The two doors. ----
          <div className="flex flex-col gap-4">
            <form
              className="flex flex-col gap-4"
              onSubmit={(e) => {
                e.preventDefault();
                void askForCode();
              }}
            >
              <TextInput
                label="Your name"
                name="username"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                autoComplete="username"
                value={name}
                onChange={(e) => setName(e.target.value)}
                autoFocus
              />
              <Button type="submit" className="w-full" disabled={!name.trim() || busy}>
                Continue
              </Button>
            </form>
            <p className="text-xs text-center" style={{ color: "var(--fg-dim)" }}>
              or
            </p>
            <PasskeyButton label="Sign in with a passkey" onActivate={passkey} />
            {config.oidc && (
              <Button
                variant="ghost"
                className="w-full"
                onClick={() => {
                  window.location.href = "/api/auth/oidc/start";
                }}
              >
                Sign in with {config.oidc_name}
              </Button>
            )}
          </div>
        )}

        {error && step === "name" && (
          <p className="mt-4 text-sm" style={{ color: "var(--accent)" }} role="alert">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
