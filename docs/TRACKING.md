# Screen-time tracking & enforcement core — audit findings and spec

Status: read-only audit, 2026-09-22. Scope: how the on-device Linux agent
(the `client` crate) counts and enforces screen time, plus how that reconciles
with the server ledger. Verified against the code; every claim carries a
`file:line` citation. Nothing in the code was changed.

The three parts:

- **A. How it works today** — mechanism-level, with the exact data flow.
- **B. Ranked correctness bugs and fragilities** — each with a concrete
  inputs → wrong-outcome scenario.
- **C. How it should work** — an implementable spec for a correct core.

---

## A. How it works today

### A.1 The unit that is counted

Screen time is counted as **wall-seconds of "active session" per OS user,
accumulated as a fixed `+10 s` credit on every enforcement tick** — i.e. it is
*tick-cadence accounting*, not a measured monotonic elapsed delta.

- The tick period is `TICK = 10 s` (`client/src/runner.rs:24`), driven by
  `tokio::time::interval(TICK)` (`client/src/runner.rs:2618` for the WS loop,
  `:2690` for the poll loop). No `MissedTickBehavior` is set, so it defaults to
  **Burst** — after any stall the interval fires all missed ticks back-to-back.
- Each tick collects the active users and credits every one of them exactly
  `TICK.as_secs()` seconds, scaled by a dev-only accel factor:
  `for user in &active { self.tracker.add_active(user, TICK.as_secs() as u32, self.ctx.time_accel) }`
  (`client/src/runner.rs:1331-1334`).
- `add_active` adds `real_secs * accel` into a per-user seconds counter
  (`client/src/enforce/screentime.rs:175-178`). `accel` is `1` in production
  (`client/src/config.rs:193,202`), a dev multiplier only.
- The ledger is a `UsageTracker` holding `used_secs: HashMap<String,u32>` and
  `earned_secs: HashMap<String,u32>`, keyed by `os_username`
  (`client/src/enforce/screentime.rs:60-68`). **So counting is per-OS-user.**
- Reported/enforced minutes are `used_secs / 60` (integer floor)
  (`client/src/enforce/screentime.rs:188-193`).

**"Active" is what `loginctl` reports, not "unlocked".** `active_seat_users`
lists every session whose `Active=yes`, seat or not
(`client/src/enforce/screentime.rs:297-348`). Two deliberate design points:

- The session `IdleHint` is **ignored** on purpose, because a managed user can
  set their own idle hint (`client/src/enforce/screentime.rs:290-296`). So the
  screen counts even if the user marks themselves idle — an active session is
  "using the machine" regardless.
- `Remote` (SSH) sessions are **counted**: `Remote` is read but discarded
  (`let _ = remote;`, `client/src/enforce/screentime.rs:339-340`). The comment
  records that `ssh localhost` used to be an unlimited-time loophole and was
  deliberately closed. A tty/VT login is likewise just another `Active` session
  and counts.

**Multiple simultaneous users:** each `Active` user is an independent key in
`used_secs` and each gets its own `+10 s`/tick, so two kids logged in at once
each burn their own budget in parallel — correct. **A user with no graphical
session** but an active SSH/tty session still counts (above). A user with *no*
active session at all is simply absent from `active` that tick and accrues
nothing.

### A.2 Enforcement decision

`evaluate(policy, tracker, user)` (`client/src/enforce/screentime.rs:213-240`)
returns an `Option<LockReason>` from three clock/ledger rules, all in
`Local` time:

1. **Bedtime** (`in_bedtime`, wraps midnight) — `screentime.rs:222-224,250-259`.
2. **Allowed windows** (`within_any_window`) — `screentime.rs:227-229,277-287`.
3. **Daily limit**: `remaining_minutes = daily_limit + earned − used`; lock when
   `<= 0` (`screentime.rs:202-209,230-238`).

The policy `ScreenTime` has only `enabled`, `daily_limit_minutes`, `schedule`,
`bedtime` (`policy/src/lib.rs:356-366`). **There is no per-policy timezone or
reset-hour field.** Wind-down length is derived from the age bracket
(`wind_down_secs`, `policy/src/lib.rs:205-212`), not the policy.

### A.3 The full data flow (tick → tracker → persist → evaluate → freeze → report)

Per `enforcement_tick` (`client/src/runner.rs:1201` onward):

1. **Clock-skew probe.** Compare `Utc::now()` to the wall-clock this tick was
   expected to land on (armed as `now + TICK` last tick); a large deviation is a
   tamper event (`runner.rs:1209-1215`). `expected_wall` starts each *process* as
   `None`, so a set-back *while the agent is off* is invisible to this probe
   (handled instead by the persisted `saved_at`, `runner.rs:196-198,530-534`).
2. **Day ceiling.** If on a local network, set the tracker ceiling to
   `local(last_contact_wall) + 1 day`; if fully offline, `None`
   (`runner.rs:1307-1313`). `clock_ahead_of_ceiling` drives a once-per-episode
   warning (`runner.rs:1314-1326`, `screentime.rs:144-146`).
3. **Account active users.** `active = active_seat_users(exec)`; each gets
   `add_active(+10 s)` (`runner.rs:1329-1334`). `add_active` first calls
   `roll_day` (`screentime.rs:176,148-159`).
4. **Attribution sampler** runs for active, **non-frozen** users
   (`runner.rs:1338-1344`) — note this path *does* exclude frozen users, but the
   budget counter in step 3 does **not**.
5. **Persist the ledger every tick** via atomic rename (`runner.rs:1370-1374`,
   `screentime.rs:92-106`), to `/var/lib/openscreentime/usage_ledger.json`
   (`screentime.rs:21-23`, `paths.rs:17,23`).
6. **Per-user freeze/unfreeze.** For every policy user, `evaluate` →
   `decide_freeze` (`runner.rs:1499-1582`, `:2501-2520`). A new lock arms a
   `FREEZE_GRACE` (60 s, or the bracket wind-down) save-your-work countdown, then
   freezes the cgroup (`screen_time_lockout`, `runner.rs:1846-1938`;
   `FREEZE_GRACE` `runner.rs:29`).
7. **Freeze mechanism.** `freeze_user` writes `1`/`0` to
   `/sys/fs/cgroup/user.slice/user-<uid>.slice/cgroup.freeze`
   (`screentime.rs:379-408`). Screen-time freezes are soft (`hard=false`): if the
   freezer is unavailable it logs and no-ops rather than killing the session
   (`screentime.rs:399-405`). `freezer_usable` / `is_frozen` report the real
   kernel state (`screentime.rs:354-371`).
8. **Persist freeze state** (`freeze_state.json`: frozen set, challenge-grant
   counters, tamper lockdown, `saved_at`) every tick (`runner.rs:1592-1594,
   1829-1844`).
9. **Report.** `usage_snapshot` builds `UsageReport { os_username,
   used_minutes_today }` for every policy user (`runner.rs:1191-1199`,
   `protocol.rs:90-94`). Sent as a WS `Heartbeat` frame every `WS_HEARTBEAT`
   30 s (`runner.rs:2632-2641`) or in the HTTP poll heartbeat
   (`runner.rs:2702-2705`).

### A.4 Server reconciliation

- The server upserts each `UsageReport` into `screen_time_ledger` keyed by
  `day = CURRENT_DATE`, `used_seconds = GREATEST(existing, reported*60)`
  (`server/src/agent.rs:321-335`). The `GREATEST` clamp makes the server total
  **monotonic within a day**: a client that resets/restarts and reports a lower
  number cannot lower the recorded total.
- Independently, if a heartbeat reports more than `USAGE_REGRESSION_SECS = 300`
  below the recorded total, the server emits a `critical` `evasion`
  (`usage_regression`) event (`server/src/agent.rs:264,294-318`).
- **The server never pushes used-time back to the client.** The command set is
  `lock, unlock, apply_policy, set_tamper_level, credit_time, deny_earn,
  login_approve, ping` (`client/src/protocol.rs:13-29`); none carries a usage
  correction. The **client ledger is authoritative for enforcement**; the server
  ledger is display/audit/cross-device aggregation only.
- Cross-device aggregation (`server/src/family.rs:62-69,174`) sums
  `used_seconds` across a person's devices for *today* (`l.day = CURRENT_DATE`)
  for the console — but nothing feeds that combined number back to any device.
- Earn-time is server-authoritative: approval enqueues a `credit_time` command;
  the client calls `add_earned` (minutes×60 into `earned_secs`) and re-saves
  (`runner.rs:2366-2369`, `screentime.rs:183-186`).

### A.5 Offline behavior

- Counting continues offline unchanged — it never depends on the server.
- Fail-closed grace: after `OST_OFFLINE_GRACE_SECS` (default 900 s) with no
  contact, re-assert the last-known network policy every tick
  (`runner.rs:49-59,876-925`).
- Hard-lockdown: after `offline_lockdown_days` days with no contact **while on a
  local network** and only if an offline unlock credential exists, freeze all
  users like an admin lock (`runner.rs:795-867`). The floor is 3 days
  (`MIN_OFFLINE_LOCKDOWN_DAYS`, `runner.rs:799-806`); parent PIN always unlocks.
- The ledger, freeze set, and last-contact wall-clock all persist across reboots
  (`screentime.rs:21`, `runner.rs:172-243`), so a power-cycle resumes rather than
  resets.

---

## B. Ranked correctness bugs and fragilities

### B1 (highest). Frozen users keep accruing used-time — earn-time grants are silently eaten

`add_active` is called for **every** `Active` user (`runner.rs:1331-1334`), and
freezing a cgroup does **not** change logind's `Active` state, so a frozen user
stays in `active_seat_users` (`screentime.rs:297-348`) and keeps getting
`+10 s`/tick. The attribution sampler deliberately excludes frozen users
(`runner.rs:1338-1344`), but the budget counter does not — the exclusion was
applied in one place and missed in the other.

- **Scenario:** Kid hits 60/60 and is frozen at the lock screen. They sit there
  20 minutes; `used` climbs to ~80 min. Parent approves +15 min earn-time →
  budget = 60 + 15 = 75. `remaining = 75 − 80 = −5` → `evaluate` still returns
  `DailyLimit`, the freeze never lifts, the parent sees "granted" but the child
  stays locked. The grant was consumed by time spent *frozen*.
- **Also:** reported `used_minutes_today` grows without bound while frozen
  (e.g. "180/60"), corrupting the console total and the cross-device sum.
- Evidence: `runner.rs:1331-1334`; freeze does not clear activity
  (`screentime.rs:379-408`); `evaluate` `screentime.rs:230-238`.

### B2. Recorded/displayed "today" is UTC-day while enforcement is local-day (verified)

The client resets its counter at **local** midnight (`effective_today` uses
`Local::now()`, `screentime.rs:119-125,148-159`) and enforces on the local day.
The server writes the ledger keyed by `CURRENT_DATE` — the Postgres server date,
which is **UTC** with the stock deployment (no `TZ`/`SET TIME ZONE` anywhere in
`server/`, `deploy/`, `compose.yaml`, `Containerfile`). `usage.rs:125-131`
explicitly documents this divergence as a known limitation. Freiburg is UTC+1/+2,
so the console's "today" flips at 01:00/02:00 local, not local midnight.

- **Consequence 1 (data loss via the clamp).** At 00:30 local (= 22:30 UTC), the
  client has already rolled to the new local day and reports a small
  `used_minutes_today`. The server upserts into **yesterday's** UTC row, where
  `GREATEST(yesterday_total, small)` swallows the new-day usage entirely
  (`server/src/agent.rs:321-335`). For the ~2 h window after local midnight, new
  usage is not recorded and does not appear on the console.
- **Consequence 2 (wrong-day attribution / cross-device sum).** `family.rs`'s
  per-child daily total (`l.day = CURRENT_DATE`, `family.rs:68-69`) buckets late
  local-night usage onto the wrong UTC day, so the console and the enforced
  device disagree near midnight.
- **Clarification of the memory note.** The *enforced* limit resets at **local**
  midnight (correct, in code). What is UTC-wrong is the **recorded/displayed**
  day and the cross-device sum. The one way the enforced reset lands at the wrong
  wall-hour is if the *device's* timezone is left at UTC (misconfig), since the
  code trusts `Local`.
- Evidence: `screentime.rs:119-125`; `server/src/agent.rs:286,325-329`;
  `server/src/family.rs:68-69`; `server/src/usage.rs:125-131`.

### B3. Accounting is tick-count, not measured elapsed — suspend/stall inflates used-time

Each tick adds a fixed `+10 s` (`runner.rs:1333`), and the interval is Burst
(default, no override at `runner.rs:2618,2690`). After the process is unscheduled
— **suspend/resume**, heavy load, or a stop-the-world stall — tokio replays every
missed tick immediately, each crediting another `+10 s`.

- **Scenario:** Laptop suspended 2 h in the afternoon (same local day). On resume,
  the ticker bursts ~720 missed ticks; each adds 10 s → ~120 min added to `used`
  at once, even though the machine was asleep. The child is charged for suspend
  time. (An overnight suspend that crosses local midnight instead resets via
  `roll_day`, so this bites same-day suspends and long stalls.)
- Secondary: the first post-resume tick also trips the clock-skew tamper event
  (`runner.rs:1209-1215`), a false tamper signal.
- Because time is never reconciled to a monotonic clock, steady-state also drifts
  slightly: a tick whose processing takes >10 s wall still books exactly 10 s.
- Evidence: `runner.rs:1333,2618,2690`; `screentime.rs:175-178`.

### B4. Clock-set-forward while fully offline mints a fresh daily budget

The forward-jump defense (`set_day_ceiling`/`effective_today`,
`screentime.rs:119-141`) only clamps the accounting day when the device is on a
local network (`ceiling` is `None` when `local_net_up` is false,
`runner.rs:1307-1312`). The comment concedes this ("an honest week away still
rolls").

- **Scenario:** Kid disables Wi-Fi / unplugs Ethernet, sets the clock to tomorrow.
  `local_net_up` is false → no ceiling → `roll_day` advances and `used_secs`
  clears → a full fresh `daily_limit`. The offline hard-lockdown cannot counter
  this for at least 3 days and only fires *with* a local network
  (`runner.rs:795-847`).
- Evidence: `runner.rs:1307-1312`; `screentime.rs:119-141`.

### B5. Per-device budgets, never a per-child budget

`daily_limit_minutes` is enforced against a single device's local `used_secs`.
Nothing on-device knows another device's usage; the cross-device sum is
display-only (`family.rs:62-69,174`), and no command pushes it down
(`protocol.rs:13-29`).

- **Scenario:** A child with a laptop and a desktop, each with a 60-min limit,
  gets 60 + 60 = 120 min/day. The `family.rs` header comment ("one child whose
  day is the sum of both devices") describes an intent the enforcement path does
  not implement.
- Evidence: `screentime.rs:202-209`; `server/src/family.rs:4,174`;
  `client/src/protocol.rs:13-29`.

### B6. Restart during counting under-counts (accepted, but real)

The ledger persists every tick and reloads on start (`runner.rs:1370-1374,564`;
`screentime.rs:80-106`), so there is **no double-count** on restart, and the
round-trip is unit-tested (`screentime.rs:495-508`). But any wall between a crash
and the next successful save+respawn is un-accounted: the child uses the machine
for free during the agent's downtime. Persistence is at most `TICK` behind, so
worst case ~10 s of loss per crash plus the whole downtime window.

- Evidence: `runner.rs:1370-1374`; `screentime.rs:92-106`.

### B7. Reporting granularity truncates to whole minutes

`used_minutes = used_secs / 60` floors (`screentime.rs:188-193`), and the wire
type is integer minutes (`protocol.rs:91-94`), re-expanded server-side as `×60`
(`server/src/agent.rs:279,333`). Up to 59 s per report never reaches the server;
harmless for enforcement (client keeps seconds) but the server total lags the
device by up to a minute per user. Minor.

### B8. `set_day_ceiling` clamps the day but never clears — a stale future day can freeze budget open

When a forward-RTC offline boot has already rolled `self.day` into the future and
the server ceiling later pulls it back, `set_day_ceiling` moves `self.day` back to
the ceiling **without clearing** `used_secs` (`screentime.rs:130-141`, by design:
"the budget that was spent stays spent"). This is correct for not *granting* free
time, but combined with B4 it means the tamper posture is: forward-offline mints a
day; coming back online does not *reclaim* the minted budget, only stops further
minting. Worth stating explicitly in any rewrite. Low severity on its own.

---

## C. How it should work (spec for a correct, robust core)

### C.1 Unit and clock

- **Count measured elapsed active time from a monotonic clock**, not a tick
  count. Each tick, add `min(now_monotonic − last_tick_monotonic, MAX_STEP)` per
  active user, where `MAX_STEP` (e.g. 2×`TICK`) caps any single step so a
  suspend/stall burst cannot inflate the total. Set
  `interval.set_missed_tick_behavior(Skip)` so missed ticks are dropped, not
  replayed. This fixes B3 directly.
- Keep seconds internally; keep per-OS-user keying. Keep the `IdleHint`-ignored
  and SSH-counts decisions (they are correct anti-cheat choices).

### C.2 Exclude non-counting sessions

- **A frozen user must not accrue.** In the accounting loop, skip users in the
  frozen set (mirror the attribution filter at `runner.rs:1338-1344`). Fixes B1.
- Optionally also skip sessions the compositor reports as locked (screensaver),
  independent of the freezer, so an idle-locked screen doesn't burn budget —
  but do this via a source that the user cannot self-assert (not `IdleHint`).

### C.3 Day boundary — one timezone, end to end

- Introduce a **device timezone** (IANA name, e.g. `Europe/Berlin`) carried in
  the policy bundle, and make **both** the on-device reset **and** the server
  ledger `day` use it. Concretely: the client already rolls on `Local`; store the
  device's UTC offset/zone server-side and compute the ledger `day` as
  `(now AT TIME ZONE device_zone)::date` instead of `CURRENT_DATE`
  (`server/src/agent.rs:286,326`; `server/src/family.rs:69`;
  `server/src/usage.rs` already flags this). Fixes B2 and removes the near-midnight
  `GREATEST` data loss.
- Reset-hour should be the family's local midnight (configurable later if needed);
  never UTC, never the server host's zone.

### C.4 Tamper resistance (clock)

- Keep the backward-jump defense (forward-only `roll_day`) and the persisted
  `saved_at` set-back-while-off detector (`runner.rs:196-198,530-534`) — both good.
- **Close the offline forward-jump hole (B4).** Anchor the accounting day to a
  monotonic "seconds since a trusted epoch" counter that only advances, plus the
  last server-confirmed date, rather than to wall-clock alone. When genuinely
  offline, allow a new day to roll only after ~24 h of *monotonic* time has
  elapsed since the last roll — not merely because the wall-clock date changed.
  This lets an honest week away roll one day per real day while defeating "unplug
  and jump the RTC".
- Preserve "spent stays spent" on clamp (B8), but document that minted-then-clawed
  budget is not reclaimed; prefer the monotonic anchor above so it is never minted.

### C.5 Restart / accuracy safety

- Keep per-tick atomic persistence (already correct, B6). Additionally persist the
  **monotonic anchor and a boot id** so a restart can tell "same boot, resume the
  monotonic delta" from "new boot, fall back to wall-clock + saved_at check".
- Keep seconds precision on the wire (send `used_seconds_today`, not minutes) to
  remove B7, or accept the ≤59 s lag as immaterial.

### C.6 Enforcement correctness

- Freeze must stop accrual (C.2). Wind-down/grace already does not double-count
  (the user is genuinely active during grace); keep it.
- Keep the kernel-truth reporting (`is_frozen`/`freezer_usable`, the
  `screen_time_no_freezer` degraded gap) — it is the right "never a green lie"
  posture (`screentime.rs:354-371`, `runner.rs:669-675`).

### C.7 Offline & server sync

- Keep the client as the enforcement authority and the server `GREATEST` clamp +
  `usage_regression` evasion event as the audit backstop
  (`server/src/agent.rs:294-335`) — this is sound once the day key is fixed (C.3).
- **Decide per-child vs per-device explicitly (B5).** If a per-child daily cap is
  the product intent, the server must push a per-device *remaining* budget (a new
  command) computed from the cross-device ledger, and the client must subtract it;
  otherwise document that limits are per-device and correct the `family.rs`
  comment. Do not leave the intent/implementation gap silent.
- Counting-while-offline is already correct; keep it.

### C.8 Summary of the target model

Per active, non-frozen OS user, accumulate **monotonic elapsed active seconds**,
capped per step, into a per-user seconds ledger that: resets at the family's
**local** midnight (one timezone used by device and server alike); rolls forward
only on real elapsed time (monotonic anchor), never on a bare wall-clock jump;
persists atomically every tick with a boot id; is floored to the wire only for
display; and is reconciled server-side by a monotonic max plus an independent
regression alarm. Freezing stops accrual. Budgets are per-device unless a
server-pushed per-child remaining is added.
