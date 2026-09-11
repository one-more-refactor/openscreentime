// First-login second-factor enrollment. Reached only for a hub account (owner/
// parent) that has NO factor yet: rather than dead-ending later at a "type the
// 6 digits" prompt for something that was never set up, we set it up here, once,
// on the way in. It is not a hard lock — the console can never lock you out of
// itself (that promise holds), so there is a quiet "later" — but the default
// path is to enrol.
import { useState } from "react";
import * as api from "../api";
import type { TotpEnrollment } from "../types";
import { QrCode } from "../components/QrCode";
import { CodeRing } from "../components/CodeRing";
import { TokenBlock } from "../components";

export function Enroll2FA({ onDone, who }: { onDone: () => void; who?: string }) {
  const [enrolling, setEnrolling] = useState<TotpEnrollment | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function begin() {
    setBusy(true);
    setError(null);
    try {
      setEnrolling(await api.startTotpEnrollment());
      setCode("");
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't start setup — try again.");
    } finally {
      setBusy(false);
    }
  }

  async function confirm() {
    setBusy(true);
    setError(null);
    try {
      await api.confirmTotpEnrollment(code);
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : "That code didn't match — check the app and try again.");
      setCode("");
    } finally {
      setBusy(false);
    }
  }

  function later() {
    // Session-scoped, so it re-asks next sign-in but never bricks this one.
    try {
      sessionStorage.setItem("ost-2fa-defer", "1");
    } catch {
      /* private mode: just proceed */
    }
    onDone();
  }

  return (
    <div className="enrol2fa">
      <div className="enrol2fa-card">
        <p className="ph-eyebrow">One-time setup</p>
        <h1 className="enrol2fa-h1">Secure your account</h1>
        <p className="enrol2fa-lede">
          You hold the keys to {who ? `${who}'s` : "the"} household. Before you go in, set up a
          second step so only you can change the important things — unlocking a device, adding a
          passkey, pairing a phone. You'll do this once.
        </p>

        {!enrolling ? (
          <>
            <button className="ch-btn ch-btn-yes" disabled={busy} onClick={() => void begin()}>
              {busy ? "Starting…" : "Set up my authenticator"}
            </button>
            {error && <p className="enrol2fa-err" role="alert">{error}</p>}
          </>
        ) : (
          <>
            <p className="enrol2fa-step">
              Add this to an authenticator app — Google Authenticator, Aegis, 1Password, whatever you
              use — then type the 6-digit code it shows.
            </p>
            <div className="tf-enrol">
              <QrCode value={enrolling.otpauth_uri} size={156} label="Scan into your authenticator app" />
              <div className="tf-enrol-text">
                <p className="add-secret-label">Can't scan? Type the secret</p>
                <TokenBlock token={enrolling.secret} />
              </div>
            </div>
            <div className="cr-wrap">
              <CodeRing
                value={code}
                disabled={busy}
                error={!!error}
                aria-label="Code from the app"
                onChange={(v) => {
                  setCode(v);
                  if (error) setError(null);
                }}
                onComplete={() => void confirm()}
              />
              <p className="cr-note" data-error={!!error} role={error ? "alert" : undefined}>
                {busy ? "Checking…" : (error ?? "The 6 digits the app shows")}
              </p>
            </div>
          </>
        )}

        <button type="button" className="enrol2fa-later" onClick={later}>
          I'll set this up later
        </button>
      </div>
    </div>
  );
}
