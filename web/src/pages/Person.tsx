// ============================================================================
// A PERSON — one page per person, in two parts (docs/PRODUCT.md §2):
//
//   /child/:key        Today: the ring and the time left, anything waiting on
//                      you, Pause and Give 15 / 30 min, where the time went,
//                      their computers, and the keys to them.
//   /child/:key/rules  Rules: the daily limit, allowed hours, bedtime, one
//                      blocklist, earning time — and, quietly at the bottom,
//                      Remove.
//
// The glance and the editor are two jobs, so they are two pages under one
// header. The face shows once, in the header; the ring is the day. Who they
// are (name, face, age) is edited in a small sheet from the header.
//
// Everything comes from the family store (one fetch shared with the rail) plus
// the audit feed for their computers. Lock state is honest: `locked` is what
// the computers report, `lock_pending` a pause still on its way.
// ============================================================================
import { useMemo, useState } from "react";
import { NavLink, useParams } from "react-router-dom";
import * as api from "../api";
import { AGE_BRACKETS, type AgeBracket, type Device, type FamilyChild, type Profile } from "../types";
import { useConfirm, StepUpCancelled } from "../lib/confirm";
import { useToast, type ToastAction } from "../lib/toast";
import { useFamily, familyChanged } from "../lib/family";
import { FACES } from "../lib/avatar";
import { BRACKET_BLURB } from "../lib/brackets";
import { sentence } from "../lib/format";
import { Avatar } from "../components/AvatarRing";
import { Button } from "../components/Button";
import { Modal } from "../components/Modal";
import { TextInput } from "../components/TextInput";
import { PageHead } from "../layout/PageHead";
import { PersonToday } from "./PersonToday";
import { PersonRules } from "./PersonRules";

/** Keeps their own time: the hub sees their minutes, never their rules. */
export function keepsOwnTime(c: FamilyChild): boolean {
  return c.self_managed === true || c.managed === false || c.age_bracket === "adult";
}

/** One of their computers, with the full row the family store holds. */
export type PersonDevice = FamilyChild["devices"][number] & { full: Device | null };

/** What both tabs need about the person, and the one way to change anything. */
export interface PersonCtx {
  child: FamilyChild;
  profile: Profile | null;
  devices: PersonDevice[];
  busy: boolean;
  /** Step up if needed, do it, say so, refresh the family. */
  change: (done: string, fn: () => Promise<unknown>, undo?: ToastAction) => Promise<boolean>;
}

function Tabs({ base, name }: { base: string; name: string }) {
  return (
    <nav className="pp-tabs" aria-label={`${name}'s page`}>
      <NavLink end to={base} className={({ isActive }) => `pp-tab${isActive ? " active" : ""}`}>
        Today
      </NavLink>
      <NavLink to={`${base}/rules`} className={({ isActive }) => `pp-tab${isActive ? " active" : ""}`}>
        Rules
      </NavLink>
    </nav>
  );
}

/** Who they are — name, face, age — in one small sheet. */
function IdentitySheet({
  open,
  child,
  busy,
  onClose,
  onSave,
}: {
  open: boolean;
  child: FamilyChild;
  busy: boolean;
  onClose: () => void;
  onSave: (patch: { display_name?: string; avatar?: string; age_bracket?: AgeBracket }) => void;
}) {
  const [name, setName] = useState(child.name);
  const [face, setFace] = useState<string>(child.avatar ?? "");
  const [bracket, setBracket] = useState<AgeBracket>(child.age_bracket);
  // Start from the person as they are each time the sheet opens.
  const [openedFor, setOpenedFor] = useState<boolean>(false);
  if (open !== openedFor) {
    setOpenedFor(open);
    if (open) {
      setName(child.name);
      setFace(child.avatar ?? "");
      setBracket(child.age_bracket);
    }
  }
  const trimmed = name.trim();
  const patch = {
    ...(trimmed && trimmed !== child.name ? { display_name: trimmed } : {}),
    ...(face !== (child.avatar ?? "") ? { avatar: face } : {}),
    ...(bracket !== child.age_bracket ? { age_bracket: bracket } : {}),
  };
  const dirty = Object.keys(patch).length > 0;

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`Edit ${child.name}`}
      footer={
        <>
          <Button variant="quiet" onClick={onClose}>
            Cancel
          </Button>
          <Button disabled={busy || !dirty || !trimmed} onClick={() => onSave(patch)}>
            Save
          </Button>
        </>
      }
    >
      <div className="stack">
        <TextInput label="Name" value={name} onChange={(e) => setName(e.target.value)} maxLength={40} autoComplete="off" />
        <fieldset className="pp-sheet-group">
          <legend className="label">Face</legend>
          <div className="pills" role="radiogroup" aria-label="Their face">
            <button
              type="button"
              role="radio"
              aria-checked={face === ""}
              className="pill pill-face"
              data-on={face === ""}
              onClick={() => setFace("")}
            >
              {(trimmed || child.name).slice(0, 1).toUpperCase()}
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
        <fieldset className="pp-sheet-group">
          <legend className="label">Age</legend>
          <div className="pills" role="radiogroup" aria-label="Age bracket">
            {AGE_BRACKETS.map((b) => (
              <button
                key={b.key}
                type="button"
                role="radio"
                aria-checked={bracket === b.key}
                className="pill"
                data-on={bracket === b.key}
                onClick={() => setBracket(b.key)}
              >
                {b.label} <span className="pill-range">{b.range}</span>
              </button>
            ))}
          </div>
          <p className="hint pp-sheet-blurb">{BRACKET_BLURB[bracket]}</p>
          {bracket !== child.age_bracket && (
            <p className="hint">Changing the age doesn't rewrite their rules — you change those under Rules.</p>
          )}
        </fieldset>
      </div>
    </Modal>
  );
}

export function Person({ tab }: { tab: "today" | "rules" }) {
  const { key = "" } = useParams();
  const { guard } = useConfirm();
  const { toast } = useToast();
  const fam = useFamily();
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState(false);

  const child = useMemo(() => fam.children.find((c) => c.key === key) ?? null, [fam.children, key]);
  const profile = useMemo(
    () => (child?.profile_id ? fam.profiles.find((p) => p.id === child.profile_id) ?? null : null),
    [fam.profiles, child],
  );
  const devices = useMemo<PersonDevice[]>(
    () => (child?.devices ?? []).map((d) => ({ ...d, full: fam.devices?.find((x) => x.id === d.id) ?? null })),
    [child, fam.devices],
  );

  if (!child) {
    return (
      <div className="page pp">
        <PageHead back={{ to: "/", label: "Family" }} title={fam.loading ? " " : "Nobody here"} />
        {fam.loading ? (
          <p className="meta wait-text">Loading…</p>
        ) : (
          <div className="banner" role="status">
            <p className="banner-main">{fam.error ?? "There's no one with that name in your family."}</p>
            <Button size="sm" variant="quiet" icon="refresh" onClick={() => void fam.reload()}>
              Try again
            </Button>
          </div>
        )}
      </div>
    );
  }

  async function change(done: string, fn: () => Promise<unknown>, undo?: ToastAction): Promise<boolean> {
    setBusy(true);
    try {
      await guard(fn);
      toast(done, "ok", undo);
      return true;
    } catch (e) {
      if (!(e instanceof StepUpCancelled)) {
        toast(sentence(e instanceof Error ? e.message : "That didn't go through. Try again."), "crit");
      }
      return false;
    } finally {
      setBusy(false);
      familyChanged();
    }
  }

  const ctx: PersonCtx = { child, profile, devices, busy, change };
  const base = `/child/${encodeURIComponent(child.key)}`;
  const label = AGE_BRACKETS.find((b) => b.key === child.age_bracket)?.label ?? child.age_bracket;
  const where =
    devices.length === 0 ? "no computer yet" : devices.length === 1 ? devices[0].name : `${devices.length} computers`;

  return (
    <div className="page pp">
      <PageHead
        back={{ to: "/", label: "Family" }}
        lead={<Avatar name={child.name} seed={child.key} avatar={child.avatar} size={56} />}
        title={child.name}
        sub={`${label} · ${where}`}
        actions={
          <Button variant="secondary" size="sm" icon="edit" onClick={() => setEditing(true)}>
            Edit
          </Button>
        }
      />
      <Tabs base={base} name={child.name} />

      {tab === "today" ? <PersonToday ctx={ctx} /> : <PersonRules ctx={ctx} />}

      <IdentitySheet
        open={editing}
        child={child}
        busy={busy}
        onClose={() => setEditing(false)}
        onSave={(patch) => {
          setEditing(false);
          void change(
            patch.display_name ? `${child.name} is ${patch.display_name} now.` : `Saved ${child.name}.`,
            () => api.updateMember(child.account_id, patch),
          );
        }}
      />
    </div>
  );
}
