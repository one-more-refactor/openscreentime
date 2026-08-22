// ============================================================================
// ADD A CHILD — a person first, their computer second.
//
// Step 1 is who they are: a name and a birthdate. The birthdate decides the
// age bracket (how much they decide for themselves, how hard the stops are)
// and the bracket picks the starting rules; a parent can override the bracket
// — a mature eleven-year-old, a late bloomer — without lying about the date.
//
// Step 2 is their computer: the one-line install, and next to it the parent
// code for that machine — an authenticator-app secret the device verifies
// offline. Scan it now; it is the key to that computer from here on.
// ============================================================================
import { useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import * as api from "../api";
import {
  AGE_BRACKETS,
  THEMES,
  bracketForBirthdate,
  defaultThemeFor,
  type Account,
  type AgeBracket,
  type EnrollTokenResponse,
  type Theme,
} from "../types";
import { QrCode } from "../components/QrCode";
import { familyChanged } from "../lib/family";

const BRACKET_BLURB: Record<AgeBracket, string> = {
  little: "You decide everything. Hard daily limit, the simplest stop, no asking for time.",
  kid: "Hard limit, hard stop — but they can ask you for time and earn it with tasks.",
  younger_teen: "Limits plus their own goals; a two-minute wind-down before the stop.",
  older_teen: "Mostly self-set. You see how it goes and can still cap it.",
  adult: "Private self-tracking. Nobody enforces anything; they can block things for themselves.",
};

export function AddChild() {
  const [name, setName] = useState("");
  const [birthdate, setBirthdate] = useState("");
  const [override, setOverride] = useState<AgeBracket | null>(null);
  const [theme, setTheme] = useState<Theme | null>(null);
  const [member, setMember] = useState<Account | null>(null);
  const [enroll, setEnroll] = useState<EnrollTokenResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const navigate = useNavigate();

  const derived = useMemo(() => (birthdate ? bracketForBirthdate(birthdate) : null), [birthdate]);
  const bracket: AgeBracket = override ?? derived ?? "kid";
  const autoTheme = defaultThemeFor(bracket);

  const origin = window.location.origin;
  const oneLiner = enroll
    ? `curl -fsSL ${origin}/install.sh | sudo OST_TOKEN=${enroll.enroll_token} sh -s -- --server ${origin}`
    : "";

  async function create(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      // The person first — with their bracket's starting rules — then the
      // computer, carrying their name so a parent can find it later, and
      // linked to them so whoever logs in on it lands on their own page.
      const m = await api.createMember({
        display_name: name.trim(),
        birthdate: birthdate || null,
        age_bracket: bracket,
        theme,
      });
      setMember(m);
      setEnroll(await api.createDevice(`${name.trim()}'s computer`, m.id));
      familyChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not set that up");
    } finally {
      setBusy(false);
    }
  }

  function done() {
    navigate(member ? `/child/${encodeURIComponent(member.id)}` : "/");
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

          <form onSubmit={create} className="add-form add-form-wide">
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

            <label className="add-label" htmlFor="child-birthdate" style={{ marginTop: "0.6rem" }}>
              When were they born? <span className="add-optional">optional — it picks the age bracket</span>
            </label>
            <input
              id="child-birthdate"
              className="add-input"
              type="date"
              value={birthdate}
              max={new Date().toISOString().slice(0, 10)}
              onChange={(e) => {
                setBirthdate(e.target.value);
                setOverride(null);
              }}
            />

            <p className="add-label" style={{ marginTop: "0.6rem" }}>
              Age bracket
              {derived && !override && <span className="add-optional">from their birthdate</span>}
              {override && <span className="add-optional">chosen by you</span>}
            </p>
            <div className="add-brackets" role="radiogroup" aria-label="Age bracket">
              {AGE_BRACKETS.map((b) => (
                <button
                  key={b.key}
                  type="button"
                  role="radio"
                  aria-checked={b.key === bracket}
                  className="add-bracket"
                  data-on={b.key === bracket}
                  onClick={() => setOverride(b.key === derived ? null : b.key)}
                >
                  <span className="add-bracket-label">{b.label}</span>
                  <span className="add-bracket-range">{b.range}</span>
                </button>
              ))}
            </div>
            <p className="add-bracket-blurb">{BRACKET_BLURB[bracket]}</p>

            <details className="add-more">
              <summary>How their own page looks</summary>
              <p className="rl-value" style={{ margin: "0.4rem 0 0.6rem" }}>
                When they open OpenScreenTime on their computer they see their own page. Auto picks{" "}
                <strong>{THEMES.find((t) => t.key === autoTheme)?.label}</strong> for this bracket.
              </p>
              <div className="pills">
                <button type="button" className="pill no-code" data-on={theme === null} onClick={() => setTheme(null)}>
                  Auto
                </button>
                {THEMES.map((t) => (
                  <button
                    key={t.key}
                    type="button"
                    className="pill no-code"
                    data-on={theme === t.key}
                    title={t.blurb}
                    onClick={() => setTheme(t.key)}
                  >
                    {t.label}
                  </button>
                ))}
              </div>
            </details>

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
            <p className="ch-meta">Two things, both on this screen only once.</p>
          </header>

          <div className="add-two">
            <section className="add-col">
              <h2 className="ch-h2">1 · Install it</h2>
              <p className="add-step-text">
                Open a Terminal on their computer, paste this in, and press Enter.
              </p>
              <pre className="add-code">{oneLiner}</pre>
              <button className="ch-btn no-code" onClick={() => void navigator.clipboard?.writeText(oneLiner)}>
                Copy command
              </button>
              <p className="ch-meta" style={{ marginTop: "0.75rem" }}>
                This command works for 24 hours and only once.
              </p>
            </section>

            <section className="add-col">
              <h2 className="ch-h2">2 · Scan the parent code</h2>
              <p className="add-step-text">
                Scan this into your authenticator app (Google Authenticator, Aegis, 1Password …).
                It's the parent key for <strong>this computer</strong>: the 6-digit code it shows
                unlocks the screen, reopens time, and lets you use <code>sudo</code> there — even
                with no internet.
              </p>
              {enroll.parent_code ? (
                <div className="add-qr">
                  <QrCode value={enroll.parent_code.otpauth_uri} label="Parent code QR" />
                  <div className="add-qr-text">
                    <p className="add-secret-label">Can't scan? Type this secret instead</p>
                    <code className="add-secret">{enroll.parent_code.secret}</code>
                    <button
                      className="ch-btn no-code"
                      onClick={() => void navigator.clipboard?.writeText(enroll.parent_code?.secret ?? "")}
                    >
                      Copy secret
                    </button>
                  </div>
                </div>
              ) : (
                <p className="fam-quiet">
                  This server didn't hand out a parent code yet — you can show it later from Settings →
                  Security &amp; access.
                </p>
              )}
            </section>
          </div>

          <div className="add-note">
            <p>
              <strong>The install also prints an 8-digit backup code on screen.</strong> Write it
              down — it appears once, and it is the spare key if your phone is ever out of reach.
            </p>
            <p className="ch-meta">
              You can show the parent code again, or replace it, under Settings → Security &amp; access.
            </p>
          </div>

          <button className="ch-btn ch-btn-yes add-submit" onClick={done}>
            Done
          </button>
        </>
      )}
    </div>
  );
}
