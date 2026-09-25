// ============================================================================
// A person's today (the first half of the person page): the ring and the time
// left, anything waiting on you answered right here, Pause and Give 15 / 30
// min, where the time went, their computers, and the keys.
// ============================================================================
import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import * as api from "../api";
import type { EarnRequest, Event } from "../types";
import { useFamily, minutesLeft, minutesTotal, ringTarget } from "../lib/family";
import { ago, duration, durationShort } from "../lib/format";
import { stopSentence } from "../lib/day";
import { useCountUp } from "../lib/useCountUp";
import { Ring, RingNumber } from "../components/Ring";
import { Button } from "../components/Button";
import { Icon } from "../components/Icon";
import { WhereTheTime } from "../components/WhereTheTime";
import { Moments } from "../components/Moments";
import { UnlockCodePanel } from "../components/UnlockCodePanel";
import { keepsOwnTime, type PersonCtx, type PersonDevice } from "./Person";

const HERO = 152;

/** The ring and the number the page exists to answer, then the verbs. */
function Hero({ ctx, requests }: { ctx: PersonCtx; requests: EarnRequest[] }) {
  const { child, devices, busy, change } = ctx;
  const name = child.name;
  const own = keepsOwnTime(child);
  const total = own ? null : minutesTotal(child);
  const left = own ? null : minutesLeft(child);
  const used = child.used_minutes;
  const paused = child.locked && devices.length > 0;
  const pending = devices.find((d) => d.lock_pending);
  const shown = useCountUp(left ?? used);
  // The ring fills toward used + left: the same number, so an unlock code's
  // time shows as time, not as a full red ring.
  const target = total === null ? null : ringTarget(used, left);
  const frac = target === null ? null : target > 0 ? used / target : 1;
  const live = devices.filter((d) => d.status !== "pending");

  // The ring's number: what is left, the thing a person asks; with no limit,
  // what was used.
  const inner =
    total === null ? (
      <RingNumber size={HERO} value={durationShort(shown)} unit={used < 60 ? "min today" : "today"} />
    ) : (
      <RingNumber size={HERO} value={durationShort(shown)} unit={shown >= 60 && shown % 60 === 0 ? "left" : "min left"} />
    );

  const next = paused
    ? "Paused by you. Their time keeps until you resume."
    : own
      ? `${name} keeps their own time — their limits and sites are theirs to set.`
      : stopSentence(child.rules, "they");

  function pause(resume: boolean) {
    void change(
      resume ? `Resuming ${name}. It shows once the computer confirms.` : `Pausing ${name}. It shows once the computer confirms.`,
      async () => {
        for (const d of live) {
          if (resume) await api.unlockDevice(d.id);
          else if (!d.locked) await api.lockDevice(d.id);
        }
      },
      resume
        ? undefined
        : {
            label: "Undo",
            run: () =>
              void change(`Resuming ${name}.`, async () => {
                for (const d of live) await api.unlockDevice(d.id);
              }),
          },
    );
  }

  function give(minutes: number) {
    // One budget per person across every computer: the grant lands on the
    // computer they asked from (it answers the ask there, and tells them),
    // else the one most likely to hear it first; the server sums the day.
    const asked = requests[0] && live.find((d) => d.device_user_id === requests[0].device_user_id);
    const target =
      asked ?? [...live].sort((a, b) => Number(b.status === "online") - Number(a.status === "online"))[0];
    if (!target) return;
    void change(`Gave ${name} ${minutes} more minutes today.`, () => api.creditTime(target.device_user_id, minutes));
  }

  function answer(r: EarnRequest, yes: boolean) {
    void change(
      yes ? `Gave ${name} ${r.minutes} more minutes.` : `Told ${name} not now.`,
      () => (yes ? api.approveEarnRequest(r.id) : api.denyEarnRequest(r.id)),
    );
  }

  return (
    <section className="pp-hero card" aria-label="Today">
      <Ring
        size={HERO}
        used={frac}
        minutesLeft={left}
        paused={paused}
        on="card"
        label={
          total === null
            ? `${duration(used)} used today, no limit set`
            : `${duration(used)} used of ${duration(total)}, ${duration(left ?? 0)} left`
        }
      >
        {inner}
      </Ring>

      <div className="pp-hero-main">
        <p className="pp-big num">
          {duration(used)} used today{" "}
          <small>
            {total !== null
              ? `of ${duration(total)}${child.earned_minutes > 0 ? ` · ${child.earned_minutes} min given` : ""}`
              : own
                ? ""
                : "· no limit set"}
          </small>
        </p>
        {next && <p className="pp-next">{next}</p>}

        {requests.map((r) => {
          const reason = r.task_label && r.task_label !== "Asked for more time" ? r.task_label : null;
          return (
            <div key={r.id} className="pp-req" role="group" aria-label="A request for time">
              <Icon name="ask" size={18} className="pp-req-ic" />
              <p className="pp-req-text">
                <span>
                  Asked for {r.minutes} more minutes · <span className="num">{ago(r.created_at)}</span>
                </span>
                {reason && <span className="pp-req-reason">“{reason}”</span>}
              </p>
              <span className="pp-req-actions">
                <Button size="sm" disabled={busy} onClick={() => answer(r, true)}>
                  Give {r.minutes} min
                </Button>
                <Button size="sm" variant="quiet" disabled={busy} onClick={() => answer(r, false)}>
                  Not now
                </Button>
              </span>
            </div>
          );
        })}

        {live.length > 0 && (
          <div className="pp-actions">
            {pending ? (
              <Button variant="secondary" icon={pending.locked ? "play" : "pause"} disabled>
                {pending.locked ? "Resuming…" : "Pausing…"}
              </Button>
            ) : paused ? (
              <Button variant="secondary" icon="play" disabled={busy} onClick={() => pause(true)}>
                Resume
              </Button>
            ) : (
              <Button variant="secondary" icon="pause" disabled={busy} onClick={() => pause(false)}>
                Pause
              </Button>
            )}
            {!own && (
              <>
                <Button variant="secondary" icon="give-time" disabled={busy} onClick={() => give(15)}>
                  Give 15 min
                </Button>
                <Button variant="secondary" disabled={busy} onClick={() => give(30)}>
                  Give 30 min
                </Button>
              </>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

/** Their computers: what each one really reports. */
function Computers({ devices, name, events }: { devices: PersonDevice[]; name: string; events: Event[] }) {
  return (
    <section className="section">
      <div className="section-head">
        <h2 className="h2">Their computers</h2>
      </div>
      <div className="card">
        {devices.length === 0 ? (
          <div className="row">
            <Icon name="laptop" size={20} className="row-ic" />
            <p className="row-main row-sub">No computer yet — set one up and {name}'s day shows here.</p>
            <Link to="/computers" className="btn btn-secondary btn-sm">
              Add a computer
            </Link>
          </div>
        ) : (
          <ul className="rows pp-devices">
            {devices.map((d) => (
              <li key={d.id} className="row" data-state={d.status}>
                <span className="pp-dev-ic">
                  <Icon name={d.status === "offline" ? "offline" : "laptop"} size={20} />
                </span>
                <div className="row-main">
                  <p className="row-title">{d.name}</p>
                  <p className="row-sub">
                    {d.status === "online"
                      ? "Online"
                      : d.status === "pending"
                        ? "Not set up yet"
                        : `Offline — last online ${ago(d.full?.last_seen)}. It keeps today's rules.`}
                  </p>
                </div>
                {d.lock_pending ? (
                  <span className="tag tag-warn">{d.locked ? "Resuming…" : "Pausing…"}</span>
                ) : d.locked ? (
                  <span className="tag">Paused</span>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </div>
      <Moments
        events={events}
        computers={devices.length > 1 ? Object.fromEntries(devices.map((d) => [d.id, d.name])) : undefined}
      />
    </section>
  );
}

/** The keys: each computer's unlock code and its recovery codes, behind a
 * confirm. One reveal per computer — the panel's own. */
function Keys({ devices, name }: { devices: PersonDevice[]; name: string }) {
  const ready = devices.filter((d) => d.full && d.status !== "pending");
  return (
    <section className="section">
      <div className="section-head">
        <h2 className="h2">Keys</h2>
      </div>
      <div className="card card-pad pp-keys">
        <p className="lede">
          The unlock code opens {name}'s computer when it's stopped — type it on the computer. Recovery codes are the
          spare keys for when your phone is out of reach. Both ask you to confirm it's you.
        </p>
        {ready.length === 0 ? (
          <p className="meta">A code appears once their computer is set up.</p>
        ) : (
          ready.map((d) => <UnlockCodePanel key={d.id} device={d.full!} />)
        )}
      </div>
    </section>
  );
}

export function PersonToday({ ctx }: { ctx: PersonCtx }) {
  const { child, devices, busy, change } = ctx;
  const fam = useFamily();
  const [events, setEvents] = useState<Event[]>([]);
  const duIds = new Set(devices.map((d) => d.device_user_id));
  const requests = fam.requests.filter((r) => duIds.has(r.device_user_id));
  const own = keepsOwnTime(child);

  // The moments that mattered on their computers. Events are per computer, so
  // on a shared one they include a sibling's — keep this person's own and the
  // computer-wide ones (a tamper has no login). Someone keeping their own time
  // has no moments here: a parent sees their minutes, and that's all (the
  // server leaves their events out too).
  const idsKey = devices
    .map((d) => d.id)
    .sort()
    .join(",");
  useEffect(() => {
    if (!idsKey || own) {
      setEvents([]);
      return;
    }
    let alive = true;
    void Promise.all(idsKey.split(",").map((id) => api.listEvents({ device_id: id, limit: 30 }).catch(() => [])))
      .then((per) => {
        if (alive) setEvents(per.flat().sort((a, b) => b.created_at.localeCompare(a.created_at)));
      });
    return () => {
      alive = false;
    };
  }, [idsKey, own]);

  return (
    <>
      {child.blocked && (
        <div className="banner banner-warn pp-banner" role="status">
          <Icon name="lock" size={20} />
          <p className="banner-main">
            {child.name}'s account still has an old block on it — they can open their page but can't ask for anything.
            Pause is the one way to stop screens now.
          </p>
          <Button
            size="sm"
            variant="secondary"
            disabled={busy}
            onClick={() => void change(`Lifted the block on ${child.name}.`, () => api.unblockMember(child.account_id))}
          >
            Lift the block
          </Button>
        </div>
      )}

      <Hero ctx={ctx} requests={requests} />

      {own ? (
        <section className="section">
          <div className="section-head">
            <h2 className="h2">Where the time went</h2>
          </div>
          <p className="card card-pad meta">
            Adults keep the details of their day to themselves. You see their minutes, and that's all.
          </p>
        </section>
      ) : (
        <WhereTheTime accountId={child.account_id} who="they" offsetSecs={child.utc_offset_secs} />
      )}

      <Computers
        devices={devices}
        name={child.name}
        events={own ? [] : events.filter((e) => e.device_user_id === null || duIds.has(e.device_user_id))}
      />
      <Keys devices={devices} name={child.name} />
    </>
  );
}
