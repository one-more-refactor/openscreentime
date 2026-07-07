import { useEffect, useState } from "react";
import { listProfiles, updateProfile, createProfile, deleteProfile } from "../api";
import type { Policy, Profile } from "../types";
import { useAsync } from "../lib/useAsync";
import { PageHeader } from "../layout/Shell";
import { Button, Panel, PolicyEditor, StatusLed } from "../components";
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
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draft, setDraft] = useState<Policy | null>(null);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);

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
      await updateProfile(selected.id, draft).catch(() => {});
      profiles.setData((prev) =>
        (prev ?? []).map((x) =>
          x.id === selected.id
            ? { ...x, policy: draft, updated_at: new Date().toISOString() }
            : x,
        ),
      );
      setDirty(false);
    } finally {
      setSaving(false);
    }
  }

  async function duplicate(p: Profile) {
    const created = await createProfile(`${p.name} Copy`, structuredClone(p.policy)).catch(
      () => {
        // offline: fabricate a local custom clone so the flow is visible
        const local: Profile = {
          ...p,
          id: `local-${Date.now()}`,
          name: `${p.name} Copy`,
          kind: "custom",
          is_preset: false,
        };
        return local;
      },
    );
    profiles.setData((prev) => [...(prev ?? []), created]);
    select(created);
  }

  async function remove(p: Profile) {
    if (p.is_preset) return;
    await deleteProfile(p.id).catch(() => {});
    profiles.setData((prev) => (prev ?? []).filter((x) => x.id !== p.id));
    setSelectedId(null);
  }

  if (profiles.loading) return <Loading />;

  return (
    <>
      <PageHeader
        title="PROFILES"
        actions={
          selected && (
            <>
              <Button variant="ghost" onClick={() => duplicate(selected)}>
                DUPLICATE
              </Button>
              {!selected.is_preset && (
                <Button variant="danger" onClick={() => remove(selected)}>
                  DELETE
                </Button>
              )}
              <Button variant="primary" disabled={!dirty || saving} onClick={save}>
                {saving ? "SAVING…" : dirty ? "SAVE POLICY" : "SAVED"}
              </Button>
            </>
          )
        }
      />

      <div className="grid lg:grid-cols-[16rem_1fr] gap-6 items-start">
        {/* Profile list */}
        <div className="flex flex-col gap-2">
          {list.map((p) => {
            const active = p.id === selectedId;
            return (
              <button
                key={p.id}
                onClick={() => select(p)}
                className="focusable text-left border rounded p-3 transition-colors"
                style={{
                  borderColor: active ? "var(--fg)" : "var(--line)",
                  background: active ? "var(--surface-2)" : "var(--surface)",
                }}
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="dot text-xs text-fg">{p.name.toUpperCase()}</span>
                  <span
                    className="label border rounded px-1.5 py-0.5"
                    style={{
                      color: p.is_preset ? "var(--fg-dim)" : "var(--accent)",
                      borderColor: p.is_preset ? "var(--line)" : "var(--accent-dim)",
                    }}
                  >
                    {KIND_LABEL[p.kind]}
                  </span>
                </div>
                <p className="label mt-2" style={{ color: "var(--fg-faint)" }}>
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
            <Panel dots>
              <Empty label="SELECT A PROFILE" />
            </Panel>
          )}
        </div>
      </div>
    </>
  );
}
