// ============================================================================
// ADD A PERSON — who they are first, their computer second.
//
// Step 1 is the person: a name, a face if you like, and a birthday. The
// birthday picks the age bracket (how much they decide for themselves, how
// firm the stops are) and the bracket picks the starting rules; a parent can
// override the bracket without lying about the date.
//
// Step 2 is their computer: the one-line install, and next to it the unlock
// code for that computer — read here whenever it's needed. A person who will
// use a computer that is already set up skips step 2 (Computers → Details →
// Who's who links their login).
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
import { useConfirm, StepUpCancelled } from "../lib/confirm";
import { UnlockCodePanel } from "../components/UnlockCodePanel";
import { EnrollCommand } from "../components/EnrollCommand";
import { TextInput } from "../components/TextInput";
import { Button } from "../components/Button";
import { Icon } from "../components/Icon";
import { PageHead } from "../layout/PageHead";
import { familyChanged } from "../lib/family";
import { FACES } from "../lib/avatar";

const BRACKET_BLURB: Record<AgeBracket, string> = {
  little: "You decide everything. A firm daily limit and the simplest stop.",
  kid: "A firm limit and stop, but they can ask you for more time.",
  younger_teen: "Limits plus their own goals, and two minutes' warning before the stop.",
  older_teen: "Mostly up to them. You see how it goes and can still set a limit.",
  adult: "Keeping time for themselves. Nothing is enforced; they can block things for themselves.",
};

export function AddChild() {
  const { guard } = useConfirm();
  const [name, setName] = useState("");
  const [face, setFace] = useState<string | null>(null);
  const [birthdate, setBirthdate] = useState("");
  const [override, setOverride] = useState<AgeBracket | null>(null);
  const [theme, setTheme] = useState<Theme | null>(null);
  const [newComputer, setNewComputer] = useState(true);
  const [member, setMember] = useState<Account | null>(null);
  const [enroll, setEnroll] = useState<EnrollTokenResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const navigate = useNavigate();

  const derived = useMemo(() => (birthdate ? bracketForBirthdate(birthdate) : null), [birthdate]);
  const bracket: AgeBracket = override ?? derived ?? "kid";
  const autoTheme = defaultThemeFor(bracket);
  const first = name.trim();

  async function create(e: React.FormEvent) {
    e.preventDefault();
    if (!first || busy) return;
    setBusy(true);
    setError(null);
    try {
      // The person first — with their bracket's starting rules — then their
      // computer, carrying their name and linked to them, so their login on
      // it lands on their own page.
      const { m, dev } = await guard(async () => {
        const m = await api.createMember({
          display_name: first,
          birthdate: birthdate || null,
          age_bracket: bracket,
          theme,
        });
        // The face is a person-detail, not part of creating them — set right
        // after, so they arrive already looking like themselves.
        if (face) await api.updateMember(m.id, { avatar: face });
        const dev = newComputer ? await api.createDevice(`${first}'s computer`, m.id) : null;
        return { m, dev };
      });
      familyChanged();
      if (!dev) {
        navigate(`/child/${encodeURIComponent(m.id)}`);
        return;
      }
      setMember(m);
      setEnroll(dev);
    } catch (err) {
      if (err instanceof StepUpCancelled) return;
      setError(err instanceof Error ? err.message : "Couldn't add them. Try again.");
    } finally {
      setBusy(false);
    }
  }

  if (enroll) {
    return (
      <div className="page add">
        <PageHead
          back={{ to: "/", label: "Family" }}
          title={`Set up ${first}'s computer`}
          sub="The command works once. The unlock code you can always come back for."
        />

        <div className="add-two">
          <section className="card card-pad add-step">
            <p className="add-n">1</p>
            <h2 className="h2">Install it</h2>
            <p className="lede">Open a Terminal on their computer, paste this in, and press Enter.</p>
            <EnrollCommand token={enroll.enroll_token} />
          </section>

          <section className="card card-pad add-step">
            <p className="add-n">2</p>
            <h2 className="h2">Your unlock code</h2>
            <p className="lede">
              On {first}'s computer it unlocks the screen and gives time back. It changes every 30
              seconds and works even with no internet.
            </p>
            <UnlockCodePanel device={enroll.device} autoShow variant="step" />
          </section>
        </div>

        <div className="banner add-note">
          <Icon name="key" size={20} />
          <div className="banner-main">
            <p>
              <b>Make recovery codes while you think of it.</b> They're the spare keys for when your
              phone is out of reach. You'll find them later in Settings.
            </p>
          </div>
        </div>

        <div className="add-done">
          <Button onClick={() => navigate(member ? `/child/${encodeURIComponent(member.id)}` : "/")}>Done</Button>
          <Link to="/" className="btn btn-quiet">
            Finish later
          </Link>
        </div>
      </div>
    );
  }

  return (
    <div className="page page-narrow add">
      <PageHead
        back={{ to: "/", label: "Family" }}
        title="Add a person"
        sub="Who they are first; their computer next. Linux computers only, for now."
      />

      <form onSubmit={create} className="card card-pad add-form">
        <TextInput
          label="Their name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="e.g. Robin"
          autoFocus
          autoComplete="off"
          maxLength={40}
        />

        <fieldset className="add-group">
          <legend className="label">
            A face <span className="add-optional">optional, they can change it</span>
          </legend>
          <div className="pills" role="radiogroup" aria-label="Their face">
            <button
              type="button"
              role="radio"
              aria-checked={face === null}
              className="pill pill-face"
              data-on={face === null}
              onClick={() => setFace(null)}
            >
              {first ? first[0].toUpperCase() : "Aa"}
            </button>
            {FACES.map((f) => (
              <button
                key={f}
                type="button"
                role="radio"
                aria-checked={face === f}
                className="pill pill-face"
                data-on={face === f}
                aria-label={`Use ${f} as their face`}
                onClick={() => setFace(f)}
              >
                {f}
              </button>
            ))}
          </div>
        </fieldset>

        <TextInput
          label="Their birthday"
          type="date"
          value={birthdate}
          max={new Date().toISOString().slice(0, 10)}
          onChange={(e) => {
            setBirthdate(e.target.value);
            setOverride(null);
          }}
          hint="Optional. It picks the age bracket below."
          className="add-date"
        />

        <fieldset className="add-group">
          <legend className="label">
            Age bracket
            {derived && !override && <span className="add-optional">from their birthday</span>}
            {override && <span className="add-optional">chosen by you</span>}
          </legend>
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
          <p className="hint add-blurb">{BRACKET_BLURB[bracket]}</p>
        </fieldset>

        <fieldset className="add-group">
          <legend className="label">Their computer</legend>
          <div className="pills" role="radiogroup" aria-label="Their computer">
            <button
              type="button"
              role="radio"
              aria-checked={newComputer}
              className="pill"
              data-on={newComputer}
              onClick={() => setNewComputer(true)}
            >
              <Icon name="laptop" size={16} />
              Set one up next
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={!newComputer}
              className="pill"
              data-on={!newComputer}
              onClick={() => setNewComputer(false)}
            >
              One that's already here
            </button>
          </div>
          {!newComputer && (
            <p className="hint">Afterwards, link their login under Computers → Details → Who's who.</p>
          )}
        </fieldset>

        <details className="disclosure">
          <summary>
            <Icon name="chevron-right" size={16} />
            How their own page looks
          </summary>
          <p className="hint add-look-hint">
            They see their own page on their computer. For this age it's{" "}
            {THEMES.find((t) => t.key === autoTheme)?.label.toLowerCase()} unless you choose.
          </p>
          <div className="pills">
            <button type="button" className="pill" data-on={theme === null} onClick={() => setTheme(null)}>
              For their age
            </button>
            {THEMES.map((t) => (
              <button
                key={t.key}
                type="button"
                className="pill"
                data-on={theme === t.key}
                title={t.blurb}
                onClick={() => setTheme(t.key)}
              >
                {t.label}
              </button>
            ))}
          </div>
        </details>

        {error && (
          <p className="hint" data-error="true" role="alert">
            {error}
          </p>
        )}
        <div>
          <Button type="submit" disabled={busy || !first}>
            {busy ? "Adding…" : newComputer ? "Continue" : `Add ${first || "them"}`}
          </Button>
        </div>
      </form>
    </div>
  );
}
