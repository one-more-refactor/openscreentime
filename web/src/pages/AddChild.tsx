// ============================================================================
// ADD A CHILD — what used to be "enroll a device".
//
// The old flow asked you to name a machine, then handed you a token. That is
// backwards: you are not adding a laptop to a fleet, you are setting up a
// person who happens to use a laptop. The device is an implementation detail of
// that, and the wording here says so throughout.
//
// The install one-liner still exists, because a Linux agent has to be installed
// somehow. It is the last step, framed as "run this on their computer", not as
// the point of the exercise.
// ============================================================================
import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import * as api from "../api";
import type { EnrollTokenResponse } from "../types";

export function AddChild() {
  const [name, setName] = useState("");
  const [enroll, setEnroll] = useState<EnrollTokenResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const navigate = useNavigate();

  const origin = window.location.origin;
  const oneLiner = enroll
    ? `curl -fsSL ${origin}/install.sh | sudo SENTINEL_TOKEN=${enroll.enroll_token} sh -s -- --server ${origin}`
    : "";

  async function create(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      // One device per child to begin with; more can be added later from their
      // page. The device carries the child's name because that is how a parent
      // will look for it, not "thinkpad-x220".
      setEnroll(await api.createDevice(`${name.trim()}'s computer`));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not set that up");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="ch-wrap">
      <Link to="/" className="ch-back">← Family</Link>

      {!enroll ? (
        <>
          <header className="ch-head-simple">
            <h1 className="ch-name">Add a child</h1>
            <p className="ch-meta">
              You'll set up their computer next. This works on Linux computers only —
              Windows, Mac, phones and tablets are not supported yet.
            </p>
          </header>

          <form onSubmit={create} className="add-form">
            <label className="add-label" htmlFor="child-name">
              What's their name?
            </label>
            <input
              id="child-name"
              className="add-input"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Vali"
              autoFocus
              autoComplete="off"
            />
            <button className="ch-btn ch-btn-yes add-submit" disabled={busy || !name.trim()}>
              {busy ? "Setting up…" : "Continue"}
            </button>
            {error && <p className="fam-error">{error}</p>}
          </form>
        </>
      ) : (
        <>
          <header className="ch-head-simple">
            <h1 className="ch-name">Set up {name}'s computer</h1>
            <p className="ch-meta">
              Open a Terminal on their computer, paste this in, and press Enter.
            </p>
          </header>

          <pre className="add-code">{oneLiner}</pre>
          <button
            className="ch-btn"
            onClick={() => void navigator.clipboard?.writeText(oneLiner)}
          >
            Copy
          </button>

          <div className="add-note">
            <p>
              <strong>It will show an 8-digit recovery PIN on screen.</strong> Write it
              down — it appears once, and it is how you get back in if that
              computer ever locks you out.
            </p>
            <p className="ch-meta">
              This command works for 24 hours and only once.
            </p>
          </div>

          <button className="ch-btn ch-btn-yes add-submit" onClick={() => navigate("/")}>
            Done
          </button>
        </>
      )}
    </div>
  );
}
