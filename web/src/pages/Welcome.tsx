// ============================================================================
// WELCOME — the one screen a first-run SSO sign-in lands on before the account
// exists. The identity is already verified (that's why we're here), but the
// name is yours to pick: the same courtesy the passkey path gets, instead of a
// username quietly derived from your email. Choose it, and the account is
// created and you're in.
// ============================================================================
import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { ApiError, finishOidcSetup, getOidcSetup, type OidcSetup } from "../api";
import { Wordmark, TextInput, Button } from "../components";

const USERNAME_RE = /^[a-z0-9._-]{3,32}$/;

/** The setup code the first-run page kept from the installer's `#setup=`
 * link (Login.tsx) — the server wants it for an SSO first run too. */
function keptSetupCode(): string | undefined {
  try {
    return sessionStorage.getItem("ost-setup") || undefined;
  } catch {
    return undefined;
  }
}

export function Welcome() {
  const [params] = useSearchParams();
  const token = params.get("setup") ?? "";

  const [setup, setSetup] = useState<OidcSetup | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [username, setUsername] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [userError, setUserError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!token) {
      setLoadError("This link is missing its setup code. Sign in again to start over.");
      return;
    }
    let alive = true;
    getOidcSetup(token)
      .then((s) => {
        if (!alive) return;
        setSetup(s);
        setUsername(s.suggested_username);
        setDisplayName(s.suggested_name);
      })
      .catch(() => {
        if (alive) setLoadError("This setup link has expired. Sign in again to start over.");
      });
    return () => {
      alive = false;
    };
  }, [token]);

  async function submit() {
    const u = username.trim().toLowerCase();
    if (!USERNAME_RE.test(u)) {
      setUserError("Use 3 to 32 lower-case letters, numbers, dots, dashes or underscores.");
      return;
    }
    setUserError(null);
    setError(null);
    setBusy(true);
    try {
      await finishOidcSetup(token, u, displayName.trim() || undefined, keptSetupCode());
      // The session cookie is set — reload into the console fresh so the
      // session provider picks up the new sign-in.
      window.location.assign("/");
    } catch (e) {
      setBusy(false);
      if (e instanceof ApiError && e.status === 409) {
        setUserError("That name is taken. Pick another.");
        return;
      }
      setError(
        e instanceof Error && e.message
          ? e.message
          : "Could not finish setting up. Try once more.",
      );
    }
  }

  return (
    <div className="signin">
      <div className="signin-box">
        <Wordmark size={1.625} className="signin-lockup" />

        {loadError ? (
          <div className="signin-form">
            <h1 className="signin-title">Let's start over</h1>
            <p className="signin-sub">{loadError}</p>
            <Button block onClick={() => window.location.assign("/login")}>
              Back to sign in
            </Button>
          </div>
        ) : !setup ? (
          <p className="signin-sub wait-text">Setting things up…</p>
        ) : (
          <form
            className="signin-form"
            onSubmit={(e) => {
              e.preventDefault();
              void submit();
            }}
          >
            <h1 className="signin-title">Welcome — pick your name</h1>
            <p className="signin-sub">
              Signed in as {setup.email}. This is the name you'll sign in with from now on.
            </p>

            <TextInput
              label="Username"
              name="username"
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
              autoComplete="username"
              value={username}
              onChange={(e) => {
                setUsername(e.target.value);
                if (userError) setUserError(null);
              }}
              placeholder="e.g. dad"
              error={userError}
              hint="Lower-case letters and numbers, 3 to 32 of them. Dots, dashes and underscores work too."
            />

            <TextInput
              label="Your name, as the family sees it"
              name="name"
              autoComplete="name"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="Parent"
            />

            <Button type="submit" block disabled={busy || !username.trim()}>
              {busy ? "Creating your account…" : "Create account"}
            </Button>
          </form>
        )}

        {error && (
          <p className="hint signin-error" data-error="true" role="alert">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
