// ============================================================================
// COMPUTERS — the machinery, kept human.
//
// One card per computer, answering what a parent actually asks: whose is it,
// is it online, is it paused, may it be away right now. From here a computer
// is added (with its install command), renamed, finished setting up, or
// removed. The technical bits — version, hostname, "is it answering?" — sit
// behind a small Details disclosure, next to who's who on it.
//
// Last good data stays on screen: a failed refresh is a banner above the
// list, never a replacement for it, and it clears on the next success.
// ============================================================================
import { useCallback, useEffect, useState } from "react";
import * as api from "../api";
import type { Account, Device, DeviceUser, EnrollTokenResponse, Event } from "../types";
import { useConfirm, StepUpCancelled } from "../lib/confirm";
import { useSession } from "../lib/session";
import { familyChanged } from "../lib/family";
import { useToast } from "../lib/toast";
import { EnrollCommand } from "../components/EnrollCommand";
import { Button } from "../components/Button";
import { Icon } from "../components/Icon";
import { Modal } from "../components/Modal";
import { TextInput, Select } from "../components/TextInput";
import { PageHead } from "../layout/PageHead";
import { ago } from "../lib/format";
import { degradedDevices, degradedSentence } from "../lib/degraded";
import { Moments, standingGaps } from "../components/Moments";

function offlineAllowed(d: Device): boolean {
  return !!d.offline_allowed_until && new Date(d.offline_allowed_until).getTime() > Date.now();
}

/** "2 h 15 min" until the away window closes. */
function leftUntil(iso: string): string {
  const mins = Math.max(0, Math.round((new Date(iso).getTime() - Date.now()) / 60000));
  if (mins < 60) return `${mins} min`;
  const h = Math.floor(mins / 60);
  return mins % 60 ? `${h} h ${mins % 60} min` : `${h} h`;
}

/** Minutes until 07:00 tomorrow — "allow offline until tomorrow morning". */
function untilTomorrowMinutes(): number {
  const t = new Date();
  t.setDate(t.getDate() + 1);
  t.setHours(7, 0, 0, 0);
  return Math.round((t.getTime() - Date.now()) / 60000);
}

type Tone = "ok" | "warn" | "neutral";

/** The one-word state. A pause is only "Paused" once the computer says so. */
function stateOf(d: Device): { word: string; tone: Tone } {
  if (d.lock_pending) return { word: d.locked ? "Resuming…" : "Pausing…", tone: "warn" };
  if (d.status === "pending") return { word: "Not set up yet", tone: "neutral" };
  if (d.locked) return { word: "Paused", tone: "neutral" };
  if (d.status === "offline")
    return offlineAllowed(d) ? { word: "Away, allowed", tone: "neutral" } : { word: "Offline", tone: "warn" };
  return { word: "Online", tone: "ok" };
}

function whoUses(d: Device): string {
  const names = (d.users ?? []).map((u) => u.display_name?.trim() || u.os_username);
  return names.length ? names.join(" · ") : "Nobody yet";
}

// ---- who's who --------------------------------------------------------------

/**
 * Which login on this computer is which person. Every login is its own person
 * unless someone says otherwise here — the installer asked about the owner's;
 * this is where "that one's me" goes. It decides who that login signs in as,
 * so it asks you to confirm.
 */
function WhoIsWho({ users, people, onChanged }: { users: DeviceUser[]; people: Account[]; onChanged: () => void }) {
  const { guard } = useConfirm();
  const [status, setStatus] = useState<{ msg: string; error: boolean } | null>(null);

  async function assign(u: DeviceUser, accountId: string) {
    const who = people.find((p) => p.id === accountId)?.display_name ?? "them";
    setStatus(null);
    try {
      await guard(() => api.assignAccount(u.id, accountId));
      setStatus({ msg: `The ${u.os_username} login is ${who} now.`, error: false });
      onChanged();
      familyChanged();
    } catch (e) {
      if (e instanceof StepUpCancelled) return;
      setStatus({ msg: e instanceof Error ? e.message : "That didn't work.", error: true });
    }
  }

  return (
    <div className="cmp-who">
      <p className="label">Who's who</p>
      {users.map((u) => (
        <div className="cmp-who-row" key={u.id}>
          <label className="cmp-who-login" htmlFor={`who-${u.id}`}>
            <code>{u.os_username}</code>
          </label>
          <select
            id={`who-${u.id}`}
            className="field field-select"
            value={u.account_id ?? ""}
            onChange={(e) => void assign(u, e.target.value)}
          >
            {!u.account_id && <option value="">Nobody yet</option>}
            {people.map((p) => (
              <option key={p.id} value={p.id}>
                {p.display_name}
                {p.role !== "member" ? " (parent)" : ""}
              </option>
            ))}
          </select>
        </div>
      ))}
      {status && (
        <p className="hint" data-error={status.error} role="status">
          {status.msg}
        </p>
      )}
    </div>
  );
}

// ---- one computer -------------------------------------------------------------

function ComputerCard({
  device,
  people,
  onChanged,
}: {
  device: Device;
  people: Account[];
  onChanged: () => void;
}) {
  const d = device;
  const { guard } = useConfirm();
  const { toast } = useToast();
  const [busy, setBusy] = useState(false);
  const [installToken, setInstallToken] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [newName, setNewName] = useState(d.name);
  const [removing, setRemoving] = useState(false);
  const [check, setCheck] = useState<{ msg: string; ok: boolean } | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const state = stateOf(d);
  const pending = d.status === "pending";
  const away = offlineAllowed(d);

  // The computer's own moments — a pause, a gap, a lost connection: its
  // events with no login (a person's are told on their page). Asked again
  // when what it says about itself changes.
  const [moments, setMoments] = useState<Event[]>([]);
  const said = `${d.status}|${d.locked}|${(d.last_state?.gaps ?? []).join(",")}`;
  useEffect(() => {
    if (pending) return;
    let alive = true;
    api
      .listEvents({ device_id: d.id, limit: 30 })
      .then((evs) => alive && setMoments(evs.filter((e) => e.device_user_id === null)))
      .catch(() => {
        /* the card stands without them */
      });
    return () => {
      alive = false;
    };
  }, [d.id, pending, said]);

  async function run(done: string, fn: () => Promise<unknown>, undo?: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await guard(fn);
      toast(done, "ok", undo ? { label: "Undo", run: () => void run("Undone.", undo) } : undefined);
      setPicking(false);
      onChanged();
      familyChanged();
    } catch (e) {
      if (e instanceof StepUpCancelled) return;
      setError(e instanceof Error ? e.message : "That didn't work. If the computer is off, it catches up when it's back.");
    } finally {
      setBusy(false);
    }
  }

  // A computer that never joined: a fresh install command for THIS computer
  // (the old one lasts a day). Minting one is standing access, so it asks.
  async function showInstall() {
    setBusy(true);
    setError(null);
    try {
      setInstallToken((await guard(() => api.regenEnrollToken(d.id))).enroll_token);
    } catch (e) {
      if (e instanceof StepUpCancelled) return;
      setError(e instanceof Error ? e.message : "Couldn't make a new install command.");
    } finally {
      setBusy(false);
    }
  }

  async function rename() {
    const name = newName.trim();
    if (!name || name === d.name) {
      setRenaming(false);
      return;
    }
    await run(`Renamed to ${name}.`, () => api.updateDevice(d.id, { name }));
    setRenaming(false);
  }

  async function remove() {
    setRemoving(false);
    await run(`${d.name} was removed.`, () => api.deleteDevice(d.id));
  }

  // "Is it answering?" — a quiet round trip; it changes nothing on the computer.
  async function checkOn() {
    setChecking(true);
    setCheck(null);
    try {
      const r = await api.pingDevice(d.id);
      if (r.ok) {
        const secs = r.latency_ms != null ? Math.max(1, Math.round(r.latency_ms / 1000)) : null;
        setCheck({ msg: secs != null ? `It answered in ${secs} s.` : "It answered.", ok: true });
      } else {
        setCheck({ msg: "No answer. It may be off or offline.", ok: false });
      }
    } catch (e) {
      setCheck({ msg: e instanceof Error ? e.message : "Couldn't reach it.", ok: false });
    } finally {
      setChecking(false);
    }
  }

  return (
    <li className="cmp card" data-state={pending ? "pending" : d.locked ? "paused" : d.status}>
      <div className="cmp-hd">
        <span className="cmp-ic" aria-hidden="true">
          <Icon name="laptop" size={22} />
        </span>
        <div className="cmp-who-main">
          {renaming ? (
            <form
              className="cmp-rename"
              onSubmit={(e) => {
                e.preventDefault();
                void rename();
              }}
            >
              <TextInput
                aria-label="Computer name"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                autoFocus
                maxLength={60}
              />
              <Button type="submit" size="sm" disabled={busy || !newName.trim()}>
                Save
              </Button>
              <Button
                size="sm"
                variant="quiet"
                onClick={() => {
                  setRenaming(false);
                  setNewName(d.name);
                }}
              >
                Cancel
              </Button>
            </form>
          ) : (
            <h2 className="cmp-name">{d.name}</h2>
          )}
          <p className="cmp-meta">{whoUses(d)}</p>
        </div>
        <span className={`tag${state.tone === "ok" ? " tag-ok" : state.tone === "warn" ? " tag-warn" : ""}`}>
          {state.word}
        </span>
      </div>

      {pending ? (
        installToken ? (
          <div className="cmp-install">
            <p className="cmp-line">Open a Terminal on that computer, paste this in, and press Enter.</p>
            <EnrollCommand token={installToken} />
          </div>
        ) : (
          <div className="cmp-row">
            <p className="cmp-line">Set up, but it hasn't joined yet.</p>
            <Button size="sm" variant="secondary" disabled={busy} onClick={() => void showInstall()}>
              Finish setting it up
            </Button>
          </div>
        )
      ) : (
        <>
          <p className="cmp-line">
            {d.status === "online" ? "Online now" : `Last online ${ago(d.last_seen)}`}
            {away && d.offline_allowed_until && <> · may be away for {leftUntil(d.offline_allowed_until)} more</>}
            {(d.pending_commands?.length ?? 0) > 0 && " · changes on their way"}
          </p>
          {degradedDevices([d]).length > 0 && (
            <p className="hint cmp-degraded" data-error="true" role="status">
              <Icon name="warning" size={16} /> {degradedSentence(d)}
            </p>
          )}
          <Moments events={moments} standing={standingGaps([d])} max={3} bare />

          <div className="cmp-actions">
            {d.locked ? (
              <Button
                size="sm"
                variant="secondary"
                icon="play"
                disabled={busy || d.lock_pending}
                onClick={() => void run("Resuming — it shows once the computer confirms.", () => api.unlockDevice(d.id))}
              >
                Resume
              </Button>
            ) : (
              <Button
                size="sm"
                variant="secondary"
                icon="pause"
                disabled={busy || d.lock_pending}
                onClick={() =>
                  void run(
                    "Pausing — it shows once the computer confirms.",
                    () => api.lockDevice(d.id),
                    () => api.unlockDevice(d.id),
                  )
                }
              >
                Pause
              </Button>
            )}
            {away ? (
              <Button
                size="sm"
                variant="quiet"
                disabled={busy}
                onClick={() => void run("It's expected online again.", () => api.setOfflineWindow(d.id, null))}
              >
                End away time
              </Button>
            ) : picking ? (
              <span className="cmp-away" role="group" aria-label="Allow it to be offline for">
                <span className="meta">Away for</span>
                <Button size="sm" variant="quiet" disabled={busy} onClick={() => void run("It may be away for 1 hour.", () => api.setOfflineWindow(d.id, 60))}>
                  1 h
                </Button>
                <Button size="sm" variant="quiet" disabled={busy} onClick={() => void run("It may be away for 4 hours.", () => api.setOfflineWindow(d.id, 240))}>
                  4 h
                </Button>
                <Button
                  size="sm"
                  variant="quiet"
                  disabled={busy}
                  onClick={() => void run("It may be away until tomorrow morning.", () => api.setOfflineWindow(d.id, untilTomorrowMinutes()))}
                >
                  Until tomorrow
                </Button>
                <button type="button" className="btn-icon btn-icon-sm" aria-label="Cancel" onClick={() => setPicking(false)}>
                  <Icon name="close" size={16} />
                </button>
              </span>
            ) : (
              <Button size="sm" variant="quiet" disabled={busy} onClick={() => setPicking(true)}>
                Allow offline…
              </Button>
            )}
          </div>
        </>
      )}

      {error && (
        <p className="hint" data-error="true" role="alert">
          {error}
        </p>
      )}

      <details className="disclosure cmp-details">
        <summary>
          <Icon name="chevron-right" size={16} />
          Details
        </summary>
        <div className="cmp-details-body">
          {!pending && (d.users?.length ?? 0) > 0 && people.length > 0 && (
            <WhoIsWho users={d.users ?? []} people={people} onChanged={onChanged} />
          )}
          {!pending && (
            <dl className="cmp-facts">
              <div>
                <dt>Version</dt>
                <dd>{d.agent_version || "—"}</dd>
              </div>
              <div>
                <dt>Hostname</dt>
                <dd>{d.hostname || "—"}</dd>
              </div>
              <div>
                <dt>Last online</dt>
                <dd>{d.last_seen ? new Date(d.last_seen).toLocaleString() : "never"}</dd>
              </div>
            </dl>
          )}
          <div className="cmp-manage">
            {!pending && (
              <Button size="sm" variant="secondary" icon="refresh" disabled={checking} onClick={() => void checkOn()}>
                {checking ? "Checking…" : "Is it answering?"}
              </Button>
            )}
            <Button size="sm" variant="quiet" icon="edit" disabled={busy} onClick={() => setRenaming(true)}>
              Rename
            </Button>
            <Button size="sm" variant="danger" icon="remove" disabled={busy} onClick={() => setRemoving(true)}>
              Remove
            </Button>
          </div>
          {check && (
            <p className="hint" data-error={!check.ok} role="status">
              {check.msg}
            </p>
          )}
        </div>
      </details>

      <Modal
        open={removing}
        onClose={() => setRemoving(false)}
        title={`Remove ${d.name}?`}
        danger
        footer={
          <>
            <Button variant="quiet" onClick={() => setRemoving(false)}>
              Cancel
            </Button>
            <Button variant="danger-solid" disabled={busy} onClick={() => void remove()}>
              Remove
            </Button>
          </>
        }
      >
        <p className="dialog-lede">
          OpenScreenTime stops looking after it: no more limits there, and its unlock code stops. The
          next time it's online it takes OpenScreenTime off itself — anyone paused or out of time there
          gets their screen back. The logins on it stay as they are. To manage it again, set it up
          again.
        </p>
      </Modal>
    </li>
  );
}

// ---- adding a computer --------------------------------------------------------

type Owner = { kind: "person"; id: string; name: string } | { kind: "shared" };

function AddComputer({
  open,
  onClose,
  people,
  mine,
  onAdded,
}: {
  open: boolean;
  onClose: () => void;
  people: Account[];
  /** Pre-select "mine" (the parent's own computer). */
  mine: boolean;
  onAdded: () => void;
}) {
  const { me } = useSession();
  const { guard } = useConfirm();
  const [ownerId, setOwnerId] = useState<string>("");
  const [name, setName] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [added, setAdded] = useState<EnrollTokenResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const owners: Owner[] = [
    ...(me?.account ? [{ kind: "person" as const, id: me.account.id, name: me.account.display_name }] : []),
    ...people
      .filter((p) => p.id !== me?.account?.id)
      .map((p) => ({ kind: "person" as const, id: p.id, name: p.display_name })),
    { kind: "shared" },
  ];
  const owner = owners.find((o) => (o.kind === "shared" ? ownerId === "shared" : o.id === ownerId));
  const suggested =
    owner?.kind === "person" ? `${owner.name}'s computer` : owner?.kind === "shared" ? "Family computer" : "";

  useEffect(() => {
    if (!open) return;
    setAdded(null);
    setError(null);
    setNameTouched(false);
    setOwnerId(mine && me?.account ? me.account.id : "");
    setName("");
  }, [open, mine, me?.account]);

  useEffect(() => {
    if (!nameTouched) setName(suggested);
  }, [suggested, nameTouched]);

  async function add(e: React.FormEvent) {
    e.preventDefault();
    if (!owner || !name.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const res = await guard(() =>
        api.createDevice(name.trim(), owner.kind === "person" ? owner.id : undefined),
      );
      setAdded(res);
      onAdded();
      familyChanged();
    } catch (err) {
      if (err instanceof StepUpCancelled) return;
      setError(err instanceof Error ? err.message : "Couldn't set that up.");
    } finally {
      setBusy(false);
    }
  }

  const isMine = owner?.kind === "person" && owner.id === me?.account?.id;

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={added ? `Set up ${added.device.name}` : "Add a computer"}
      footer={
        added ? (
          <Button onClick={onClose}>Done</Button>
        ) : (
          <>
            <Button variant="quiet" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" form="add-computer" disabled={busy || !owner || !name.trim()}>
              {busy ? "Setting up…" : "Continue"}
            </Button>
          </>
        )
      }
    >
      {added ? (
        <div className="stack">
          <p className="dialog-lede">
            Open a Terminal on {isMine ? "your computer" : "that computer"}, paste this in, and press
            Enter. It shows up here within a minute.
            {isMine && " After that, you can sign in with your name and a code that shows up there."}
          </p>
          <EnrollCommand token={added.enroll_token} />
        </div>
      ) : (
        <form id="add-computer" className="stack" onSubmit={add}>
          <Select label="Whose computer is it?" value={ownerId} onChange={(e) => setOwnerId(e.target.value)}>
            <option value="" disabled>
              Choose…
            </option>
            {owners.map((o) =>
              o.kind === "shared" ? (
                <option key="shared" value="shared">
                  Shared — several people use it
                </option>
              ) : (
                <option key={o.id} value={o.id}>
                  {o.id === me?.account?.id ? `Mine (${o.name})` : o.name}
                </option>
              ),
            )}
          </Select>
          <TextInput
            label="What should it be called?"
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              setNameTouched(true);
            }}
            placeholder="e.g. Living room PC"
            maxLength={60}
            hint="Linux computers only, for now."
          />
          {error && (
            <p className="hint" data-error="true" role="alert">
              {error}
            </p>
          )}
        </form>
      )}
    </Modal>
  );
}

// ---- the page -----------------------------------------------------------------

// Last good list, kept across visits so coming back shows the cards at once
// and refreshes underneath.
let lastDevices: Device[] | null = null;

export function Computers() {
  const { me } = useSession();
  const [devices, setDevices] = useState<Device[] | null>(lastDevices);
  const [error, setError] = useState<string | null>(null);
  const [people, setPeople] = useState<Account[]>([]);
  // `?add=mine` (from "Add my computer" on the family page or My computer)
  // opens "Add a computer" with the parent's own chosen. Design review only:
  // ?mock=add opens it for a screenshot.
  const [adding, setAdding] = useState<null | "any" | "mine">(() => {
    const q = new URLSearchParams(window.location.search);
    if (q.get("add") === "mine") return "mine";
    return api.usingMock && q.get("mock") === "add" ? "any" : null;
  });

  useEffect(() => {
    let alive = true;
    api
      .listMembers()
      .then((m) => alive && setPeople(m))
      .catch(() => {
        /* no picker without the list — the cards still work */
      });
    return () => {
      alive = false;
    };
  }, []);

  const load = useCallback(async () => {
    try {
      // A fresh array: the mock hands back the same reference every time.
      const next = [...(await api.listDevices())];
      lastDevices = next;
      setDevices(next);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't load the computers.");
    }
  }, []);

  useEffect(() => {
    void load();
    // "Last online" and the away countdown drift by the minute.
    const t = setInterval(() => void load(), 30_000);
    return () => clearInterval(t);
  }, [load]);

  const list = devices ?? [];
  const paused = list.filter((d) => d.locked);
  const dark = list.filter((d) => d.status === "offline" && !offlineAllowed(d));
  const degraded = degradedDevices(list);
  const verdict = !devices
    ? " "
    : list.length === 0
      ? "No computers yet."
      : paused.length > 0
        ? paused.length === 1
          ? `${paused[0].name} is paused.`
          : `${paused.length} computers are paused.`
        : dark.length > 0
          ? dark.length === 1
            ? `${dark[0].name} hasn't been online for a while.`
            : `${dark.length} computers haven't been online for a while.`
          : degraded.length > 0
            ? degraded.length === 1
              ? `${degraded[0].name} can't apply all of its rules.`
              : `${degraded.length} computers can't apply all of their rules.`
            : "Every computer is doing what it should.";
  const haveMine = list.some((d) => !!d.owner_account_id && d.owner_account_id === me?.account?.id);

  return (
    <div className="page cmps">
      <PageHead
        title="Computers"
        sub={verdict}
        actions={
          <Button icon="add" onClick={() => setAdding("any")}>
            Add a computer
          </Button>
        }
      />

      {error && (
        <div className="banner banner-stop cmps-banner" role="alert">
          <Icon name="warning" size={20} />
          <p className="banner-main">
            {devices ? "Couldn't refresh — this is how things looked a moment ago." : "Couldn't load the computers."}
          </p>
          <Button size="sm" variant="quiet" icon="refresh" onClick={() => void load()}>
            Try again
          </Button>
        </div>
      )}

      {devices && !haveMine && me?.account && (
        <div className="banner cmps-banner">
          <Icon name="laptop" size={20} />
          <p className="banner-main">Add your own computer, and you can sign in with a code that shows up on it.</p>
          <Button size="sm" variant="secondary" onClick={() => setAdding("mine")}>
            Add my computer
          </Button>
        </div>
      )}

      {!devices && !error ? (
        <ul className="cmp-list" aria-busy="true" aria-label="Loading the computers">
          {[0, 1].map((i) => (
            <li key={i} className="cmp card">
              <div className="cmp-hd">
                <span className="wait" style={{ width: 44, height: 44, borderRadius: "50%" }} />
                <div className="cmp-who-main">
                  <span className="wait" style={{ width: "45%", height: 18, marginBottom: 8 }} />
                  <span className="wait" style={{ width: "30%", height: 13 }} />
                </div>
              </div>
            </li>
          ))}
        </ul>
      ) : devices && list.length === 0 ? (
        <div className="cmps-empty">
          <Icon name="laptop" size={32} />
          <p>No computers yet. Add one, and it shows up here within a minute.</p>
          <Button icon="add" onClick={() => setAdding("any")}>
            Add a computer
          </Button>
        </div>
      ) : (
        <ul className="cmp-list">
          {list.map((d) => (
            <ComputerCard key={d.id} device={d} people={people} onChanged={() => void load()} />
          ))}
        </ul>
      )}

      <AddComputer
        open={adding !== null}
        mine={adding === "mine"}
        onClose={() => setAdding(null)}
        people={people}
        onAdded={() => void load()}
      />
    </div>
  );
}
