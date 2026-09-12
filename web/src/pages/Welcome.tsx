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
      setUserError("3–32 characters: a–z, 0–9, dot, underscore, hyphen.");
      return;
    }
    setUserError(null);
    setError(null);
    setBusy(true);
    try {
      await finishOidcSetup(token, u, displayName.trim() || undefined);
      // The session cookie is set — reload into the console fresh so the
      // session provider picks up the new sign-in.
      window.location.assign("/");
    } catch (e) {
      setBusy(false);
      if (e instanceof ApiError && e.status === 409) {
        setUserError("That username is taken — pick another.");
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
    <div className="min-h-screen flex items-center justify-center px-6">
      <div className="w-full max-w-sm">
        <div className="mb-2">
          <Wordmark size={2} />
        </div>

        {loadError ? (
          <>
            <p className="mb-6 text-sm" style={{ color: "var(--fg-dim)" }}>
              {loadError}
            </p>
            <Button onClick={() => window.location.assign("/login")}>Back to sign in</Button>
          </>
        ) : !setup ? (
          <p className="mt-6 text-sm" style={{ color: "var(--fg-dim)" }}>
            Setting things up…
          </p>
        ) : (
          <form
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              void submit();
            }}
          >
            <div>
              <p style={{ color: "var(--fg-display)", fontWeight: 500 }}>
                Welcome — pick your name.
              </p>
              <p className="mt-1 text-xs" style={{ color: "var(--fg-dim)" }}>
                Signed in as {setup.email}. This is the name you'll sign in with from now on.
              </p>
            </div>

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
              aria-invalid={!!userError}
              hint={userError ?? "3–32 characters: a–z, 0–9, dot, underscore, hyphen."}
            />

            <TextInput
              label="Display name (optional)"
              name="name"
              autoComplete="name"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="Parent"
            />

            <Button type="submit" disabled={busy || !username.trim()}>
              {busy ? "Creating your account…" : "Create account"}
            </Button>
          </form>
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
      </div>
    </div>
  );
}
