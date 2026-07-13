import { useEffect, useState } from "react";
import { listProfiles, updateProfile, createProfile, deleteProfile } from "../api";
import type { Policy, Profile } from "../types";
import { useAsync } from "../lib/useAsync";
import { useToast, errMsg } from "../lib/toast";
import { PageHeader } from "../layout/Shell";
import { Button, ErrorPanel, Modal, Panel, PolicyEditor, StatusLed } from "../components";
import { Empty, Loading } from "./Devices";
import { minutesToHm } from "../lib/format";

const KIND_LABEL: Record<Profile["kind"], string> = {
  kids: "KIDS",
  teen: "TEEN",
  default: "DEFAULT",
  custom: "CUSTOM",
};

export function Profiles() {
  const profiles = useAsync<Profile[]>(listProfiles, []);
  const { toast } = useToast();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draft, setDraft] = useState<Policy | null>(null);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [duplicating, setDuplicating] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<Profile | null>(null);
  const [deleting, setDeleting] = useState(false);

  const list = profiles.data ?? [];
  const selected = list.find((p) => p.id === selectedId) ?? null;

  // Select first profile once loaded.
  useEffect(() => {
    if (!selectedId && list.length) {
      setSelectedId(list[0].id);
      setDraft(structuredClone(list[0].policy));
      setDirty(false);
    }
  }, [list, selectedId]);

  function select(p: Profile) {
    setSelectedId(p.id);
    setDraft(structuredClone(p.policy));
    setDirty(false);
  }

  async function save() {
    if (!selected || !draft) return;
    setSaving(true);
    try {
      const updated = await updateProfile(selected.id, draft);
      profiles.setData((prev) =>
        (prev ?? []).map((x) => (x.id === selected.id ? updated : x)),
      );
      setDirty(false);
      toast("Policy saved.", "ok");
    } catch (e) {
      toast(errMsg(e, "Couldn't save the policy — your edits are still here, try again."));
    } finally {
      setSaving(false);
    }
  }

  async function duplicate(p: Profile) {
    setDuplicating(true);
    try {
      const created = await createProfile(`${p.name} Copy`, structuredClone(p.policy));
      profiles.setData((prev) => [...(prev ?? []), created]);
      select(created);
    } catch (e) {
      toast(errMsg(e, `Couldn't duplicate ${p.name} — try again.`));
    } finally {
      setDuplicating(false);
    }
  }

  async function remove(p: Profile) {
    if (p.is_preset) return;
    setDeleting(true);
    try {
      await deleteProfile(p.id);
      profiles.setData((prev) => (prev ?? []).filter((x) => x.id !== p.id));
      setSelectedId(null);
      setConfirmDelete(null);
      toast(`${p.name} deleted.`, "ok");
    } catch (e) {
      toast(errMsg(e, `Couldn't delete ${p.name} — it may still be assigned to a user.`));
    } finally {
      setDeleting(false);
    }
  }

  if (profiles.loading) return <Loading />;
  if (profiles.error)
    return (
      <ErrorPanel
        title="Couldn't load profiles"
        detail={profiles.error}
        onRetry={profiles.reload}
      />
    );

  return (
    <>
      <PageHeader
        title="PROFILES"
        actions={
          selected && (
            <>
              <Button variant="ghost" disabled={duplicating} onClick={() => void duplicate(selected)}>
                {duplicating ? "DUPLICATING…" : "DUPLICATE"}
              </Button>
              {!selected.is_preset && (
                <Button variant="danger" onClick={() => setConfirmDelete(selected)}>
                  DELETE
                </Button>
              )}
              <Button variant="primary" disabled={!dirty || saving} onClick={() => void save()}>
                {saving ? "SAVING…" : dirty ? "SAVE POLICY" : "SAVED"}
              </Button>
            </>
          )
        }
      />

      <div className="grid lg:grid-cols-[16rem_1fr] gap-6 items-start">
        {/* Profile list */}
        <div className="flex flex-col gap-2">
          {list.map((p, i) => {
            const active = p.id === selectedId;
            return (
              <button
                key={p.id}
                onClick={() => select(p)}
                className="focusable relative text-left border rounded p-3 transition-colors"
                style={{
                  borderColor: active ? "var(--fg)" : "var(--line)",
                  background: active ? "var(--surface-2)" : "var(--surface)",
                }}
              >
                <span className="tick tick-tl" />
                <span className="tick tick-tr" />
                <span className="tick tick-bl" />
                <span className="tick tick-br" />
                <div className="flex items-center justify-between gap-2">
                  <span className="dot text-xs text-fg">{p.name.toUpperCase()}</span>
                  <span className="ref">PR-{String(i + 1).padStart(2, "0")}</span>
                </div>
                <p className="label mt-2" style={{ color: "var(--fg-faint)" }}>
                  {KIND_LABEL[p.kind]}
                  {" · "}
                  {p.policy.screen_time.enabled
                    ? `${minutesToHm(p.policy.screen_time.daily_limit_minutes)}/DAY`
                    : "NO TIME LIMIT"}
                  {" · "}
                  {p.policy.dns.allowlist.length} ALLOWED
                </p>
              </button>
            );
          })}
          {list.length === 0 && <Empty label="NO PROFILES" />}
        </div>

        {/* Editor */}
        <div>
          {selected && draft ? (
            <div className="flex flex-col gap-4">
              <Panel
                title={`EDITING · ${selected.name.toUpperCase()}`}
                refCode="POL-01"
                aside={
                  <StatusLed
                    tone={dirty ? "warn" : "ok"}
                    label={dirty ? "UNSAVED" : "IN SYNC"}
                  />
                }
              >
                <PolicyEditor
                  value={draft}
                  onChange={(next) => {
                    setDraft(next);
                    setDirty(true);
                  }}
                />
              </Panel>
            </div>
          ) : (
            <Panel dots refCode="POL-00">
              <Empty label="SELECT A PROFILE" />
            </Panel>
          )}
        </div>
      </div>

      {/* Delete confirm */}
      <Modal
        open={!!confirmDelete}
        onClose={() => setConfirmDelete(null)}
        title="DELETE PROFILE"
        danger
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmDelete(null)}>
              CANCEL
            </Button>
            <Button
              variant="danger"
              disabled={deleting}
              onClick={() => confirmDelete && void remove(confirmDelete)}
            >
              {deleting ? "DELETING…" : "DELETE PROFILE"}
            </Button>
          </>
        }
      >
        <p className="text-xs leading-relaxed" style={{ color: "var(--fg-dim)" }}>
          This deletes <span className="dot text-fg">{confirmDelete?.name}</span> permanently.
          Users assigned to it must be moved to another profile first.
        </p>
      </Modal>
    </>
  );
}
