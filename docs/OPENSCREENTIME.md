# OpenScreenTime — Product & Design Brief

> The north star for the Sentinel → **OpenScreenTime** rebrand + rebuild.
> Decisions below were made with the operator, not guessed. When something
> here conflicts with older docs (they still say "Sentinel"), this wins.

## What it is

Not "parental controls." **A screen-time app for the whole family** — and for
people with no kids at all. Everyone tracks their own device usage; a parent is
the hub for anyone they manage. It earns daily use by being genuinely useful and
glanceable, **not by nagging**. Where it can, it removes friction from the
healthy choice instead of policing the unhealthy one.

"Sentinel" — intimidating, surveillance-flavored — is gone from everything
user-facing.

## Principles (the feeling)

1. **Glanceable over logged.** Every important state is *visualized* at a glance
   (rings, color, badges, the pause state). The text activity feed is the **last
   resort** for detail — never the primary way you understand what's happening.
2. **Friction on the good path is the enemy.** Good apps are pre-allowed by
   default; on setup we *offer to set healthy sites as the browser home / pins /
   default search*. Make the low-effort path the good one.
3. **It practices what it preaches.** **Silent unless you're needed** — no
   engagement-bait notifications, not even a daily recap. A notification fires
   only when a human must act: a time request, a pause, a tamper.
4. **Warm, not loud.** Apple-calm base (clean type, whitespace, restraint) +
   real, fluid motion + the *encouragement* of streaks and rings. No mascot, no
   confetti storms.
5. **Communicate through feeling and through it just working.** Not nicer words.

## People & roles

- **Hub = the parent** (or a solo adult managing only themselves). Everything
  routes through the hub. Family members do **not** interact peer-to-peer — no
  sibling-to-sibling nudges. A child requests → the parent answers.
- **Age brackets** — autonomy scales with age. Enforcement, *when it happens*, is
  always a **plain hard stop** (no euphemism, no softening the words):

  | Bracket | Autonomy | Enforcement |
  |---|---|---|
  | **0–6 Little** | Curated good-by-default allowlist only. Parent does everything. No request UI. | Hard daily limit, hard stop. Biggest, simplest lock button. |
  | **6–12 Kid** | Can send time requests and earn time via tasks. Default-good allowlist; parent widens. | Hard limit, hard stop. |
  | **12–16 Younger teen** | Goals + limit, rich own stats. Requests to parent. More categories open by default. | Hard stop with a brief wind-down countdown, then stop. |
  | **16–18 Older teen** | Mostly self-set goals with parent visibility; parent can still cap. | Hard stop only where the parent enforces a cap. |
  | **Adult 18+** | Fully private self-tracking. No parent, no external enforcement. Can be a hub for others. | Self-imposed only (self-set focus / limits). |

## Home = the family pulse

**Rings on top, story below.**

- **Ring grid:** each managed person is a live activity ring (today vs goal).
  Instant read of who's fine / who's over / who's paused. Tap → that person.
- **Compact timeline below:** the day's real *moments* (goal hit, paused, over
  limit, tamper) — only the ones that matter, visualized, not a raw log dump.
- **Pause Everything** control up top (the "lockout options to the top" + "one
  tap pauses all" asks, in one place).

## Per-person page

Order, top to bottom:

1. **Control first** — Pause, limits, the hard-stop status. (Moved to top per request.)
2. Today's ring + earned / limit.
3. Requests waiting (for the parent).
4. Where they use it (devices).
5. Recent activity — the last-resort detail. Sentence-case, legible sans, **no
   ALL-CAPS mono**.

## Healthy alternatives = defaults, not a feed

- Ship a curated, age-bracketed set of "good" apps/sites **pre-added to
  allowlists by default**.
- On setup / device add: **offer to set healthy defaults** — browser homepage,
  pinned tabs, default search — behind one confirm.
- No nag feed, no in-the-moment interruptions. The good thing is simply already
  there and easier to reach.

## One tap = Pause Everything

A single prominent control freezes every managed screen in the house *now*; tap
again to resume. Front and center on home. Named scenes (Dinner / Bedtime /
Focus) can come later — the hero is the instant pause.

## Auth & user management (the operator's "first off")

**Everyone has an account** — parents, teens, adults — and logs in with their
*device as the voucher*. Age bracket + role decide what they see and can do; an
adult's self-tracking is private to them.

**Two ways in (user's choice):**
- **Passkey** (WebAuthn) — works from any browser.
- **Device-voucher autologin** — the installed OpenScreenTime client on a
  machine silently authenticates the browser session on that machine. Open the
  console on a device you own and you're already in.

**Reading is frictionless; changing needs a second factor.** Any mutation —
grant time, change a limit, pause, edit settings, add a device — requires
**step-up 2FA**. Second factor (v1): an **authenticator app (TOTP)** *or* an
**emailed token**. No SMS/phone, no QR device-pairing in v1 — both scrapped.

**The server validates everything.** The client/session is never trusted for
authorization; every mutation and every second-factor check is verified
server-side. Sessions ride rotating tokens (7-day window).

**Could-be-an-app:** the console is built to be wrappable (Tauri / PWA) so the
same gorgeous site can ship as the desktop/phone app later. Not built in v1, but
nothing in the design should preclude it.

This replaces the current enroll-token/heartbeat model and folds in the server
hardening (atomic enroll, hashed tokens, no device self-forging its state).

## Enforcement = hard stop, stated plainly, and finally correct

The new enforcement model is where the red-team screen-time fixes land:

- Read the kernel freeze state back **every tick**; stop treating "not evaluated"
  as "within policy" (kills the VT-flip and `echo 0 > cgroup.freeze` bypasses).
- Day roll + budget anchored to a fixed timezone + monotonic time (kills the
  clock/timezone budget reset).
- **Fail loudly** if the freezer isn't available — no silent no-op while the UI
  says "frozen."
- **Brick-safe:** a host that can't enforce does not self-lock the family out;
  `daily_limit = 0` means **zero, not unlimited**; a malformed schedule can't
  mean a 24/7 lockout.
- **Plain words:** when it stops, it says it stopped.

## Type & motion

- Drop ALL-CAPS mono for content; clean, legible sans, sentence case. Keep a
  tight display face only for the big numbers.
- Motion everywhere but purposeful: ring fills, number count-ups, page
  transitions, the pause sweeping across the grid. Warm, smooth, never
  bouncy-loud.

## Build order

1. **Rebrand** Sentinel → OpenScreenTime across the web console (name, copy,
   metadata). Safe, visible, first.
2. **Auth / user-management rework** — everyone has an account; sign in via
   passkey *or* device-voucher autologin; rotating 7-day tokens; step-up 2FA
   (TOTP or emailed token) on every change; server validates everything. (The
   "first off," the meaty core.)
3. **Home = family pulse** (rings + timeline + Pause Everything).
4. **Per-person page** reorder (control to top) + legible type + motion pass.
5. **Age brackets** end-to-end.
6. **Healthy-defaults engine.**
7. **Enforcement correctness + hard-stop** (client), landing the red-team fixes.
