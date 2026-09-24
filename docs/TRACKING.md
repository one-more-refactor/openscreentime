# Screen-time tracking — how time is measured and decided

Status: rewritten 2026-09-24 with the fair-measurement work. It replaces the
2026-09-22 read-only audit, whose findings (B1–B8) are listed at the end with
what became of each. Scope: the **decision layer** — how much time a person
has used, and whether they should be stopped right now, and why. *How* a
stop is presented and applied (the overlay, the cgroup freeze, the frozen
set, notifications) is documented in `docs/AGENT.md` and owned separately.

The bar, in the owner's words: the thing has to be fair for everyone. The
number must be right, and a stop must never come as a surprise or for a
wrong reason.

---

## 1. What counts as screen time

`client/src/enforce/activity.rs` — the pure `billable()` behind the
`ActivityProbe` trait.

A minute is billed to a person only while **both** hold:

1. **Their session is the foreground session on a seat.** logind says
   `Active=yes`, `State=active`, a non-empty `Seat`, and a `user*` class.
2. **They are using it.** There was keyboard, mouse, touchpad, touchscreen or
   gamepad input on that seat within the last **5 minutes** (about when a
   desktop would blank the screen), **or** sound is playing.

So these never burn anyone's budget:

| Situation | Why it used to count | Now |
|---|---|---|
| Walked away; the screen locked; the laptop stayed on | Session stays `Active=yes` when locked | No input for 5 min → stops counting |
| Lid closed / suspended | (tick-count accounting) | Billing uses `CLOCK_MONOTONIC`, which stops while suspended |
| systemd ≥ 256 `Class=manager` session (every login has one) | Seatless sessions are always `Active=yes` | Not a seat session |
| A `closing` session kept alive by a leftover process (tmux, nohup) | Same | `State=closing` is excluded |
| Fast user switching: A's session left in the background while B plays | A counted in parallel with B | Background sessions are `State=online`, excluded |
| SSH login | Counted since 0.6 to close `ssh localhost` | Seatless → not screen time (the person at the seat is counted on their own session) |
| Frozen at the lock screen | Fixed in `982c5df` | Still excluded |

**Why these signals.** logind's `IdleHint` was tried and reverted
(`7e4bd34` → `df96882`): the session's owner can set it, so a child could mark
themselves idle forever. The signals used now are read **by root** and
cannot be switched off from the child's account: evdev input nodes
(`/dev/input/event*`) read **non-exclusively** — never grabbed, nothing else
on the machine notices — and ALSA playback substreams
(`/proc/asound/card*/pcm*p/sub*/status`, `state: RUNNING`). Faking either
signal can only make time count *more*.

**Privacy.** The input reader looks at the event *type* only (key / pointer
/ axis), keeps one number per seat — when the last input happened — and
drops everything else. No key codes are stored, logged or sent.

**Where input can't be read** (no `/dev/input`, not root, a container), the
seat falls back to presence — the old behaviour — rather than never
counting, and the status file says `measured: false`.

**Accelerometers** (convertibles) are skipped (`INPUT_PROP_ACCELEROMETER`), so
tilting a tablet isn't "using it". Lid and headphone-jack switches are `EV_SW`
events and never count.

## 2. How much time

`client/src/runner.rs` — `tick_loop`, `billable_elapsed`.

- The enforcement tick runs every 10 s **on its own timer**, independent of
  the WS/poll network loops (`MissedTickBehavior::Delay`). Before, it only ran
  *inside* those loops: offline, a device ticked about once a minute and
  counted 10–25 % of real use, and the watchdog (which watches the heartbeat
  the tick touches) kept restarting a healthy offline agent.
- Each tick bills the **measured awake time** since the previous tick
  (`CLOCK_MONOTONIC`, excludes suspend), capped at **60 s** so a stalled agent
  can't bill a big chunk when it wakes. The first tick after a start bills
  nothing (downtime is unknown — see residuals).
- Seconds are kept; the wire carries seconds (`used_seconds_today`), so the
  console isn't a floored minute behind.

## 3. Which day

`client/src/clock.rs` — `TrustedClock`.

Every decision (day roll, bedtime, allowed hours, override expiry) reads the
**trusted clock**:

- the wall clock while the kernel says it is **NTP-synchronized** (read-only
  `adjtimex`; setting the clock by hand marks it unsynchronized);
- else the **family server's clock**, sent with every usage reply;
- else the last anchor extrapolated by **`CLOCK_BOOTTIME`** (real time since
  boot, including suspend; nobody can set it).

So: the day rolls at **local midnight**, **forward only** (a clock set back
never wipes the counters), **never earlier than real elapsed time allows**
(a clock set forward by hand doesn't roll it), and **without the server**.
Offline for days, each real midnight gives a fresh budget. The old ceiling
("last server contact + 1 day") is gone — during a server outage it locked
kids out every morning with yesterday's "60 of 60".

On a new boot the anchor is the wall clock, but never earlier than the last
trusted time persisted before shutdown (a clock set back while powered off
buys nothing). The anchor lives in the ledger, so a restart keeps it.

## 4. The rules

`policy/src/rules.rs` — `evaluate()`, one pure function used by the agent
(to enforce) and the server (for the console). Semantics:

- Daily limit `0` (or screen time disabled) = **no limit**, never "0 left of 0".
- A day **without** an allowed-hours window is **any time**. (It used to mean
  locked all day — while the console said "any time".)
- A window ending `00:00` runs **to midnight**; `00:00 – 00:00` is all day.
- A window whose end is before its start **crosses midnight** (Friday
  20:00 – 01:00 allows Saturday 00:00 – 01:00 too; the tail never restricts
  Saturday on its own).
- Bedtime is every day and may cross midnight.
- An **empty or unreadable** window, or a **whole-day bedtime**, is ignored —
  never a 24/7 lockout. The server refuses them on save with a plain message
  (`validate_screen_time`); the web editor checks the same shared vectors
  (`policy/tests/schedule-vectors.json`) before saving.

It answers *allowed?*, *why* (`limit` / `bedtime` / `outside_hours` /
`paused`), and **when the next stop lands** — the first of: the budget
running out (assuming continuous use), bedtime, the end of the allowed
window, the end of an override. So "minutes left" at 19:55 before a 20:00
bedtime is **5**, not the 40 the budget alone would say.

## 5. One budget per person

A daily limit is **one budget per person across all their computers**
(`server/src/ledger.rs`).

- Each usage report carries the device's own seconds, **its local day** and
  UTC offset. The server files it under that day (not Postgres' UTC
  `CURRENT_DATE`) and answers with what the same person used and was granted
  on their other logins that day.
- The device enforces its own use plus that. Offline, the last answer for
  today keeps applying (persisted, tagged with its day).
- The console's "today" is each device's own local day, and its "left" is
  computed the same way as the device's (seconds, rounded up), so the two
  agree up to the 30 s report interval.
- Grants are credited to the device's local day and carry that day in the
  `credit_time` command, so a grant delivered after midnight is not credited
  to the wrong day.

The **usage-regression ("evasion") check** compares within the same device
day only, so a legitimate local-midnight reset can never trip it; it is
skipped for agents too old to say which day they mean; and it fires **once
per device user per day**, not on every heartbeat. (It used to fire a
critical "clock games" alert about every 30 s per child from local midnight
until UTC midnight.)

## 6. Parent overrides

One persisted override per user (`UsageTracker::overrides`), written by every
parent action, beating limit, bedtime and allowed hours, surviving restarts,
expiring on the trusted clock:

| Action | Effect |
|---|---|
| Approved request / console "+N min" (`credit_time`) | N minutes on today's budget **and** an override for N minutes. Idempotent by command id: a redelivery after a lost ack is acked `duplicate`. |
| Code at the lock screen | 30 minutes, plus every device-level lock cleared. |
| `ost unlock --minutes N` | N minutes for everyone on the machine (it used to clear the lock and re-stop the child ~70 s later). |
| Console Resume | Clears the pause; a plain Resume gives 30 minutes to whoever a rule is stopping right then (explicit `minutes` / `until: "end_of_day"` also accepted). |

A **pause beats an override** — the later, stronger parent action.

## 7. What the device publishes

`status.<user>.json` carries the verdict — `allowed`, `reason`,
`minutes_left`, `stop_at`, `resume_at`, `next_warning_at` (10 and 2 minutes
before the stop), `override_until`, `counting`, `measured` — documented
field by field in `docs/AGENT.md` → "The verdict". The warnings and the lock
screen are built on these.

## 8. Known residuals (honest limits)

- **A muted video watched without touching anything stops counting after
  5 minutes.** No input and no sound is indistinguishable from "walked
  away". Accepted: under-counting a silent film beats billing every dinner.
- **Agent downtime isn't billed.** The first tick after a (re)start bills
  nothing; a crash, update or reboot loses at most the downtime. The ledger is
  persisted every tick, so nothing already counted is lost.
- **A clock moved while the machine is powered off** (BIOS/RTC) is believed on
  the next boot if it moved *forward* (a set-back is floored by the persisted
  high-water mark). It only borrows tomorrow's budget — the forward-only day
  pays it back — and NTP or the server corrects the clock at the next contact.
- **A wrong RTC with no NTP and no server** (e.g. a dual-boot machine keeping
  local time in the RTC, offline) shifts the midnight roll by the RTC's error
  until either source is reachable.
- **Other computers' use is as fresh as their last report** (every 30 s while
  connected). Two devices used at the same moment can overshoot the shared
  budget by up to about a report interval each; a device offline all day
  reports when it comes back, after the fact.
- **The console's "when screens stop" doesn't know a code typed at the
  device** (a local override); it says what the rules say.
- **Multi-seat machines**: sound is not per seat; playing audio counts for
  every present seat.
- **Input devices that chatter** (a gamepad with a noisy stick outside the
  driver's fuzz) keep a seat "in use". Rare; the kernel's fuzz filter drops
  most jitter.

## 9. The 2026-09-22 audit, finding by finding

| # | Finding | Now |
|---|---|---|
| B1 | Frozen users kept accruing | Fixed (`982c5df`), kept. |
| B2 | Ledger keyed by the UTC day | Fixed: filed under the device-local day (§5). The audit overstated the loss for UTC+ zones; the real harms were the post-midnight display lag and the nightly false evasion alerts — both gone. |
| B3 | "Suspend replays missed ticks" | The audit was wrong about suspend (`Instant` is `CLOCK_MONOTONIC`, it doesn't advance asleep); the real bug was the opposite, under-counting offline. Billing is now measured and capped, on its own timer (§2). |
| B4 | Clock set forward offline mints a fresh day | Fixed: the trusted clock (§3). |
| B5 | Per-device budgets | Decided: **per person** across computers (§5). |
| B6 | Downtime under-counts | Accepted residual (§8). |
| B7 | Minute truncation on the wire | Fixed: seconds on the wire. |
| B8 | A clamped future day isn't reclaimed | Moot: the day is never minted early any more. |
