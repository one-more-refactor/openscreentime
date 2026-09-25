// ============================================================================
// SIGN IN — two doors, nothing else (docs/AUTH.md, brand board § d).
//
//   Your name → Continue → a 6-digit code shows up on your own computer →
//   type it here, in six boxes.
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
import { takeToken } from "../lib/fragment";
import { ApiError, getAuthConfig, usingMock } from "../api";
import type { AuthConfig } from "../types";
import { Wordmark } from "../components/Wordmark";
import { PasskeyButton } from "../components/PasskeyButton";
import { TextInput } from "../components/TextInput";
import { Button } from "../components/Button";
import { CodeBoxes } from "../components/CodeBoxes";
import { sentence } from "../lib/format";

const SETUP_KEY = "ost-setup";

/** The setup code from the installer's link — taken from the address bar
 * before the redirect to /login (lib/fragment.ts), then kept for this tab
 * only, so a reload mid-setup doesn't lose it. */
function takeSetupToken(): string {
  const fromLink = takeToken("setup");
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

/** "4:52" */
function clock(secs: number): string {
  const s = Math.max(0, secs);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** After this long without a code, the page says what a code needs. The
 * server answers every name the same way (a name with no computer that may
 * show a code gets a code that never comes), so the page can't know which it
 * is — it can only say, gently, what makes a code come, and offer the other
 * door. */
export const NO_CODE_HINT_MS = 30_000;

type Step = "name" | "code";

export function Login() {
  const { createHousehold, signInWithPasskey, sendCode, enterCode } = useSession();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  // Design review only (VITE_USE_MOCK=1): ?mock=code / ?mock=firstrun open
  // those states directly. Compiled out of a real build.
  const review = usingMock ? params.get("mock") : null;

  const [config, setConfig] = useState<AuthConfig | null>(null);
  const [setupToken, setSetupToken] = useState<string>(takeSetupToken);
  const [askSetupCode, setAskSetupCode] = useState(false);
  const [name, setName] = useState("");
  const [step, setStep] = useState<Step>("name");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [expiresAt, setExpiresAt] = useState<number | null>(null);
  const [sentAt, setSentAt] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [error, setError] = useState<string | null>(() =>
    params.get("error") ? "That sign-in didn't work. Try again." : null,
  );

  useEffect(() => {
    let alive = true;
    getAuthConfig()
      .then((c) => alive && setConfig(review === "firstrun" ? { ...c, needs_setup: true } : c))
      .catch(
        () =>
          alive &&
          setConfig({ oidc: false, oidc_name: "SSO", needs_setup: false, setup_code_required: false }),
      );
    return () => {
      alive = false;
    };
  }, [review]);

  useEffect(() => {
    if (review !== "code") return;
    setName("philip");
    void sendCode("philip").then((secs) => {
      setExpiresAt(Date.now() + secs * 1000);
      setSentAt(Date.now());
      setStep("code");
    });
  }, [review, sendCode]);

  // The code's clock, in plain sight.
  useEffect(() => {
    if (step !== "code" || !expiresAt) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [step, expiresAt]);

  const firstRun = config?.needs_setup === true;
  const showSetupCode = firstRun && (askSetupCode || (config?.setup_code_required && !setupToken));
  const secsLeft = expiresAt ? Math.round((expiresAt - now) / 1000) : null;
  const expired = secsLeft !== null && secsLeft <= 0;
  // Still waiting, nothing typed, a while on: say what a code needs.
  const noCodeYet = !expired && !busy && !error && code === "" && sentAt !== null && now - sentAt >= NO_CODE_HINT_MS;

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
      const secs = await sendCode(name.trim());
      setExpiresAt(Date.now() + secs * 1000);
      setSentAt(Date.now());
      setNow(Date.now());
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
          ? "That code has expired. Send a new code, or use a passkey."
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

  function backToDoors() {
    setStep("name");
    setError(null);
    setExpiresAt(null);
    setSentAt(null);
  }

  return (
    <div className="signin">
      <div className="signin-box">
        <Wordmark size={1.625} className="signin-lockup" />

        {!config ? null : firstRun ? (
          // ---- First run: your name, then a passkey. ----
          <form
            className="signin-form"
            onSubmit={(e) => {
              e.preventDefault();
              void create();
            }}
          >
            <h1 className="signin-title">Create your household</h1>
            <p className="signin-sub">Your name, then a passkey on this device. No password, anywhere.</p>
            <TextInput
              label="Your name"
              name="name"
              autoComplete="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Philip"
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
            <Button type="submit" block icon="passkey" disabled={!name.trim() || busy}>
              {busy ? "Waiting for your passkey…" : "Create passkey"}
            </Button>
          </form>
        ) : step === "code" ? (
          // ---- Door one, part two: the code from your computer. ----
          <div className="signin-form">
            <h1 className="signin-title">Check your computer</h1>
            <p className="signin-sub">
              Enter the code from your computer. It's in the OpenScreenTime window there.
            </p>
            <CodeBoxes
              value={code}
              disabled={busy || expired}
              error={!!error}
              aria-label="The code from your computer"
              onChange={(v) => {
                setCode(v);
                if (error) setError(null);
              }}
              onComplete={(full) => void verify(full)}
            />
            {error ? (
              <p className="hint" data-error="true" role="alert">
                {error}
              </p>
            ) : (
              <p className="hint num" role="status">
                {busy
                  ? "Checking…"
                  : expired
                    ? "That code has expired."
                    : secsLeft !== null
                      ? `Expires in ${clock(secsLeft)}`
                      : "It works for 5 minutes."}
              </p>
            )}
            {noCodeYet && (
              <p className="hint signin-nocode">
                No code? Your computer must be on, and it must be your own login — or{" "}
                <button type="button" className="link" onClick={() => void passkey()}>
                  use a passkey
                </button>
                .
              </p>
            )}
            {expired ? (
              // The code ran out (or never came): the two ways on, as doors.
              <>
                <Button block icon="refresh" onClick={() => void askForCode()} disabled={busy}>
                  Send a new code
                </Button>
                <PasskeyButton label="Sign in with a passkey" onActivate={passkey} />
                <div className="signin-row">
                  <Button size="sm" variant="quiet" onClick={backToDoors}>
                    Cancel
                  </Button>
                </div>
              </>
            ) : (
              <>
                <div className="signin-row">
                  <Button size="sm" variant="secondary" icon="refresh" onClick={() => void askForCode()} disabled={busy}>
                    Send a new code
                  </Button>
                  <Button size="sm" variant="quiet" onClick={backToDoors}>
                    Cancel
                  </Button>
                </div>
                <p className="signin-foot">
                  Not at your computer?{" "}
                  <button type="button" className="link" onClick={() => void passkey()}>
                    Sign in with a passkey
                  </button>{" "}
                  instead.
                </p>
              </>
            )}
          </div>
        ) : (
          // ---- The two doors. ----
          <div className="signin-form">
            <h1 className="signin-title">Sign in</h1>
            <p className="signin-sub">Your own computer approves you. Nothing to remember.</p>
            <form
              className="signin-form"
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
                placeholder="e.g. philip"
                autoFocus
              />
              <Button type="submit" block disabled={!name.trim() || busy}>
                Continue
              </Button>
            </form>
            <p className="signin-or">or</p>
            <PasskeyButton label="Sign in with a passkey" onActivate={passkey} />
            {config.oidc && (
              <Button
                variant="quiet"
                block
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
          <p className="hint signin-error" data-error="true" role="alert">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
