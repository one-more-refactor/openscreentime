// ============================================================================
// FAMILY — the home screen, "the wall everyone reads" (brand board § e).
//
// A greeting and one honest verdict sentence; Pause everything; then one card
// per person, sorted by who needs you — their ring, their time left, and a
// request for time answered right there on the card. The parent's own card
// sits on the same wall. Machinery interrupts only when a computer has really
// gone quiet.
//
// The store under it refreshes itself (lib/family.ts), so a new request and
// "Pausing…" → "Paused" arrive without navigating.
// ============================================================================
import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import type { Device, EarnRequest, MeToday } from "../types";
import * as api from "../api";
import { useSession } from "../lib/session";
import { useConfirm, StepUpCancelled } from "../lib/confirm";
import { useToast } from "../lib/toast";
import { AvatarRing } from "../components/AvatarRing";
import { Button, buttonClass } from "../components/Button";
import { Icon } from "../components/Icon";
import { Mark } from "../components/Wordmark";
import { useFamily, familyChanged, minutesLeft, minutesTotal, ringTarget, type FamilyChild } from "../lib/family";
import { unlockedUntil } from "../lib/day";
import { PauseEverything } from "../components/PauseEverything";
import { useCountUp } from "../lib/useCountUp";
import { PageHead } from "../layout/PageHead";
import { ago, duration } from "../lib/format";

const BRACKET_LABEL: Record<FamilyChild["age_bracket"], string> = {
  little: "Little",
  kid: "Kid",
  younger_teen: "Younger teen",
  older_teen: "Older teen",
  adult: "Adult",
};

/** "Kid · Mia's laptop" — the bracket, then where they are. */
function metaLine(c: FamilyChild): string {
  const where =
    c.devices.length === 0
      ? "No computer yet"
      : c.devices.length === 1
        ? c.devices[0].name
        : `${c.devices.length} computers`;
  return `${BRACKET_LABEL[c.age_bracket] ?? c.age_bracket} · ${where}`;
}

/** An adult keeping their own time: the hub sees their minutes, not their rules. */
function keepsOwnTime(c: FamilyChild): boolean {
  return c.self_managed === true || c.managed === false || c.age_bracket === "adult";
}

/** Today's time, in one sentence. The number counts to its value. */
function TimeLine({ child, paused }: { child: FamilyChild; paused: boolean }) {
  const total = minutesTotal(child);
  const left = minutesLeft(child);
  const shown = useCountUp(left ?? child.used_minutes);

  if (paused) return <p className="pc-time">Paused by you</p>;
  if (keepsOwnTime(child)) {
    return (
      <p className="pc-time">
        <b className="num">{duration(shown)}</b> today · keeps their own time
      </p>
    );
  }
  if (total === null || left === null) {
    return (
      <p className="pc-time">
        <b className="num">{duration(shown)}</b> today · no limit set
      </p>
    );
  }
  if (left === 0) {
    return (
      <p className="pc-time" data-tone="stop">
        <b>Time's up</b> for today · {duration(total)} used
      </p>
    );
  }
  const unlocked = unlockedUntil(child.rules);
  if (unlocked) {
    // An unlock code or a grant is what keeps them going: that time, plainly.
    return (
      <p className="pc-time" data-tone={left <= 15 ? "warn" : undefined}>
        <b className="num">{duration(shown)}</b> left · unlocked until {unlocked}
      </p>
    );
  }
  return (
    <p className="pc-time" data-tone={left <= 15 ? "warn" : undefined}>
      <b className="num">{duration(shown)}</b> left of {duration(total)}
      {child.earned_minutes > 0 && <span className="pc-earned"> · {child.earned_minutes} min given</span>}
    </p>
  );
}

/** A request for time, with its two answers right on the card. */
function Request({ request, name }: { request: EarnRequest; name: string }) {
  const { guard } = useConfirm();
  const { toast } = useToast();
  const [busy, setBusy] = useState(false);
  const reason = request.task_label && request.task_label !== "Asked for more time" ? request.task_label : null;

  async function answer(give: boolean) {
    setBusy(true);
    try {
      await guard(() => (give ? api.approveEarnRequest(request.id) : api.denyEarnRequest(request.id)));
      toast(give ? `Gave ${name} ${request.minutes} more minutes.` : `Told ${name} not now.`);
      familyChanged();
    } catch (e) {
      if (!(e instanceof StepUpCancelled)) {
        toast(e instanceof Error ? e.message : "That didn't go through. Try again.", "crit");
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="pc-req">
      <Icon name="ask" size={18} className="pc-req-ic" />
      <div className="pc-req-text">
        <span>
          Asked for {request.minutes} more minutes · <span className="num">{ago(request.created_at)}</span>
        </span>
        {reason && <span className="pc-req-reason">“{reason}”</span>}
      </div>
      <div className="pc-req-actions">
        <Button size="sm" disabled={busy} onClick={() => void answer(true)}>
          Give {request.minutes} min
        </Button>
        <Button size="sm" variant="quiet" disabled={busy} onClick={() => void answer(false)}>
          Not now
        </Button>
      </div>
    </div>
  );
}

/** A paused person's card offers the way back. */
function ResumeRow({ child }: { child: FamilyChild }) {
  const { guard } = useConfirm();
  const { toast } = useToast();
  const [busy, setBusy] = useState(false);

  async function resume() {
    setBusy(true);
    try {
      await guard(async () => {
        for (const d of child.devices) if (d.locked) await api.unlockDevice(d.id);
      });
      toast(`Resuming ${child.name}. It shows once the computer confirms.`);
      familyChanged();
    } catch (e) {
      if (!(e instanceof StepUpCancelled)) toast(e instanceof Error ? e.message : "Couldn't resume.", "crit");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="pc-req">
      <Icon name="pause" size={18} className="pc-req-ic pc-req-ic-quiet" />
      <div className="pc-req-text">Time keeps for when you resume.</div>
      <div className="pc-req-actions">
        <Button size="sm" variant="secondary" icon="play" disabled={busy} onClick={() => void resume()}>
          Resume
        </Button>
      </div>
    </div>
  );
}

function PersonCard({ child, requests }: { child: FamilyChild; requests: EarnRequest[] }) {
  // `locked` is what the computers report; a pause on its way is shown as
  // exactly that, never as already done.
  const paused = child.locked && child.devices.length > 0;
  const pendingDev = child.devices.find((d) => d.lock_pending);
  // The ring fills toward the time they have today (used + left, so it agrees
  // with the number); someone keeping their own time shows an empty track —
  // their limit is theirs.
  const left = minutesLeft(child);
  const target = keepsOwnTime(child) ? null : ringTarget(child.used_minutes, left);

  return (
    <li className="pc card" data-paused={paused}>
      <div className="pc-hd">
        <AvatarRing
          name={child.name}
          seed={child.key}
          avatar={child.avatar}
          used={child.used_minutes}
          target={target}
          left={left}
          paused={paused}
        />
        <div className="pc-who">
          <Link to={`/child/${encodeURIComponent(child.key)}`} className="pc-name">
            {child.name}
          </Link>
          <p className="pc-meta">{metaLine(child)}</p>
        </div>
        {pendingDev ? (
          <span className="tag tag-warn">{pendingDev.locked ? "Resuming…" : "Pausing…"}</span>
        ) : paused ? (
          <span className="tag">Paused</span>
        ) : left === 0 ? (
          <span className="tag tag-stop">Time's up</span>
        ) : null}
      </div>
      <TimeLine child={child} paused={paused} />
      {requests.map((r) => (
        <Request key={r.id} request={r} name={child.name} />
      ))}
      {paused && !pendingDev && <ResumeRow child={child} />}
    </li>
  );
}

/**
 * The parent, on the same wall — their own day, private to them, opening
 * their own page.
 */
function YouCard() {
  const { me } = useSession();
  const [today, setToday] = useState<MeToday | null>(null);
  useEffect(() => {
    let alive = true;
    api
      .getMeToday()
      .then((t) => alive && setToday(t))
      .catch(() => alive && setToday(null));
    return () => {
      alive = false;
    };
  }, []);
  const name = me?.account?.display_name ?? me?.admin.display_name ?? "You";
  // Your own limit — the one you set on My computer: the ring fills toward
  // the time you have (used + left, a snooze included).
  const limit =
    today?.limit_minutes != null ? ringTarget(today.used_minutes, Math.max(0, today.left_minutes ?? 0)) : null;
  return (
    <li className="pc card pc-you">
      <div className="pc-hd">
        <AvatarRing
          name={name}
          seed={me?.account?.id ?? "you"}
          avatar={me?.account?.avatar}
          used={today?.used_minutes ?? 0}
          target={limit}
          left={today?.left_minutes ?? null}
        />
        <div className="pc-who">
          <Link to="/me" className="pc-name">
            {name} <span className="tag">you</span>
          </Link>
          <p className="pc-meta">
            {today && today.devices.length === 0 ? "No computer of your own yet" : "Your own day · private to you"}
          </p>
        </div>
      </div>
      <p className="pc-time">
        {today ? (
          <>
            <b className="num">{duration(today.used_minutes)}</b>
            {today.limit_minutes != null ? ` of the ${duration(today.limit_minutes)} you set` : " today"}
          </>
        ) : (
          " "
        )}
      </p>
    </li>
  );
}

/** Only when a computer has really gone quiet — never on a healthy day. A
 *  computer inside an allowed-offline window is not trouble. */
function Trouble({ devices }: { devices: Device[] }) {
  const dark = devices.filter(
    (d) =>
      d.status === "offline" &&
      !(d.offline_allowed_until && new Date(d.offline_allowed_until).getTime() > Date.now()),
  );
  if (dark.length === 0) return null;
  return (
    <div className="banner banner-warn fam-trouble">
      <Icon name="offline" size={20} />
      <p className="banner-main">
        {dark.length === 1
          ? `${dark[0].name} was last online ${ago(dark[0].last_seen)}. If it's switched off, that's fine.`
          : `${dark.length} computers haven't been online for a while. If they're switched off, that's fine.`}
      </p>
      <Link to="/computers" className="btn btn-quiet btn-sm">
        See computers
      </Link>
    </div>
  );
}

/**
 * Logins nobody has said who they are yet. Calm — nothing is wrong, and until
 * a parent sorts them their rules enforce nothing on a parent's computer —
 * but only a parent can do it, under Computers → Who's who.
 */
function Unsorted({ devices }: { devices: Device[] }) {
  const waiting = devices.filter((d) => (d.unsorted_logins ?? 0) > 0);
  if (waiting.length === 0) return null;
  const n = waiting[0].unsorted_logins ?? 0;
  return (
    <div className="banner fam-trouble">
      <Icon name="person" size={20} />
      <p className="banner-main">
        {waiting.length === 1
          ? `${waiting[0].name} has ${n === 1 ? "a login" : `${n} logins`} nobody's sorted yet.`
          : `${waiting.length} computers have logins nobody's sorted yet.`}
      </p>
      <Link to="/computers" className="btn btn-quiet btn-sm">
        Who's who
      </Link>
    </div>
  );
}

/**
 * Nobody else in the household yet — it's just you. Say so honestly and offer
 * both ways on: keep time for yourself (your own computer, then My computer),
 * or look after someone else. Your own card sits beside it, so the wall is
 * never empty.
 */
function JustYou({ haveMine }: { haveMine: boolean }) {
  return (
    <li className="fr card">
      <Mark size={48} />
      <h2 className="fr-title">It's just you so far.</h2>
      {haveMine ? (
        <p className="fr-lede">
          Your computer is set up. Your own limit, focus hours and the sites you block for yourself live on{" "}
          <Link to="/me" className="link">
            My computer
          </Link>
          . When there's someone else to look after, add them here.
        </p>
      ) : (
        <ol className="fr-steps fr-doors">
          <li>
            <b>Keeping time for yourself?</b> Add your own computer, then set your own limit and focus hours on My
            computer.
          </li>
          <li>
            <b>Looking after someone?</b> Add each person — a name and a birthday; their age sets sensible rules —
            and then their computer.
          </li>
        </ol>
      )}
      <div className="fr-actions">
        {haveMine ? (
          <>
            <Link to="/add" className={buttonClass("primary")}>
              <Icon name="add" size={18} />
              Add a person
            </Link>
            <Link to="/me" className={buttonClass("secondary")}>
              <Icon name="laptop" size={18} />
              My computer
            </Link>
          </>
        ) : (
          <>
            <Link to="/computers?add=mine" className={buttonClass("primary")}>
              <Icon name="laptop" size={18} />
              Add my computer
            </Link>
            <Link to="/add" className={buttonClass("secondary")}>
              <Icon name="add" size={18} />
              Add a person
            </Link>
          </>
        )}
      </div>
    </li>
  );
}

/** Waiting: the real layout drawn in outline, so nothing jumps when it lands. */
function FamilyWaiting() {
  return (
    <ul className="people" aria-busy="true" aria-label="Loading the family">
      {[0, 1].map((i) => (
        <li key={i} className="pc card">
          <div className="pc-hd">
            <span className="wait" style={{ width: 64, height: 64, borderRadius: "50%" }} />
            <div className="pc-who">
              <span className="wait" style={{ width: "40%", height: 18, marginBottom: 8 }} />
              <span className="wait" style={{ width: "60%", height: 13 }} />
            </div>
          </div>
          <span className="wait" style={{ width: "55%", height: 16, marginTop: 16 }} />
        </li>
      ))}
    </ul>
  );
}

export function Family() {
  const { devices, children, requests, error, loading, refreshing, reload } = useFamily();
  const { me } = useSession();
  const [sweeping, setSweeping] = useState(false);

  const hour = new Date().getHours();
  const greeting = hour < 11 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening";

  // Every computer that could be paused; one still being set up has no agent.
  const pausable = (devices ?? []).filter((d) => d.status !== "pending");
  const allPaused = pausable.length > 0 && pausable.every((d) => d.locked);

  const requestsFor = (c: FamilyChild) => {
    const ids = new Set(c.devices.map((d) => d.device_user_id));
    return requests.filter((r) => ids.has(r.device_user_id));
  };

  // Who needs you floats to the top: a request waiting, then paused, then out
  // of time; everyone fine settles below. Stable within a tier, so the wall
  // doesn't reshuffle on every refresh.
  const needsScore = (c: FamilyChild): number => {
    if (c.pending_requests > 0) return 0;
    if (c.locked && c.devices.length > 0) return 1;
    if (minutesLeft(c) === 0) return 2;
    return 3;
  };
  const sorted = [...children].sort((a, b) => needsScore(a) - needsScore(b));

  // The three-second answer to "is everyone OK?".
  const verdict = (): string => {
    if (children.length === 0) return "It's just you here so far.";
    const asking = children.filter((c) => c.pending_requests > 0);
    const paused = children.filter((c) => c.locked && c.devices.length > 0);
    const spent = children.filter((c) => !c.locked && minutesLeft(c) === 0);
    const parts: string[] = [];
    if (paused.length) parts.push(paused.length === 1 ? `${paused[0].name} is paused.` : `${paused.length} people are paused.`);
    if (spent.length) parts.push(spent.length === 1 ? `${spent[0].name} is out of time.` : `${spent.length} people are out of time.`);
    if (!paused.length && !spent.length) parts.push("Everyone is within their time.");
    if (asking.length)
      parts.push(asking.length === 1 ? `${asking[0].name} asked for more.` : `${asking.length} people asked for more.`);
    return parts.join(" ");
  };

  const hasData = devices !== null;

  return (
    <div className="page fam" data-sweeping={sweeping}>
      {refreshing && <span className="refresh-bar" aria-hidden="true" />}
      <PageHead
        eyebrow="Family"
        title={greeting}
        sub={loading && !hasData ? " " : verdict()}
        actions={
          // Just you so far: the card below carries the ways on, and one
          // primary action per screen is plenty.
          children.length > 0 ? (
            <Link to="/add" className={buttonClass("primary")}>
              <Icon name="add" size={18} />
              Add a person
            </Link>
          ) : undefined
        }
      />

      {error && (
        <div className="banner banner-stop fam-banner" role="alert">
          <Icon name="warning" size={20} />
          <p className="banner-main">
            {hasData ? "Couldn't refresh — this is how things looked a moment ago." : "Couldn't load your family."}
          </p>
          <Button size="sm" variant="quiet" icon="refresh" onClick={() => void reload()}>
            Try again
          </Button>
        </div>
      )}

      {/* A household pause needs a household: alone, your own computer's
          limit is on My computer. */}
      {pausable.length > 0 && children.length > 0 && (
        <PauseEverything devices={pausable} allPaused={allPaused} onSweep={setSweeping} onDone={reload} />
      )}

      {devices && <Trouble devices={devices} />}
      {devices && <Unsorted devices={devices} />}

      {loading && !hasData ? (
        <FamilyWaiting />
      ) : children.length === 0 ? (
        // Invite first-run setup only when the family is really empty — not
        // when the first load failed (the banner above says that).
        error ? null : (
          <ul className="people">
            <YouCard />
            <JustYou haveMine={(devices ?? []).some((d) => !!d.owner_account_id && d.owner_account_id === me?.account?.id)} />
          </ul>
        )
      ) : (
        <ul className="people">
          {sorted.map((c) => (
            <PersonCard key={c.key} child={c} requests={requestsFor(c)} />
          ))}
          <YouCard />
        </ul>
      )}
    </div>
  );
}
