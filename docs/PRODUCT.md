# OpenScreenTime — the product

What the product is, how it behaves, and the words it uses. The look is
[`brand/board.html`](../brand/board.html); this doc owns the behaviour and
the vocabulary. Cited files are under `web/src/` and `client/src/` unless
noted.

## 1. The house clock

OpenScreenTime is the clock on the kitchen wall, not a cop at the door. A
**self-hosted screen-time clock for a whole household** — children, teens,
and adults keeping time only for themselves. One parent (the **hub**) runs
the household from a web console; everyone else has one page of their own
(`/me`), a calm window on their computer, and — when the day's time is spent
— a plain, full-screen stop.

*Set it once. It keeps time.* A wall clock has five properties, and each is a
promise:

| | Promise |
|---|---|
| **Fair** | Everyone reads the same clock. A person sees what a parent sees about them, and the parent's own time hangs on the same wall. Only real use is counted. |
| **Set it once** | You hang it and it runs: the server starts at boot, backs up, updates itself; the computers update from it. Nothing to babysit. |
| **Quiet** | It doesn't nag. It speaks only when a person is needed: a request, a pause, a stop, a computer that went dark. No streaks, no recaps. |
| **Honest** | It says the time plainly. When time is up, it says so — calm, kind, firm, in that order. It never shows a wish as a fact. |
| **Yours too** | It works for one person: the clock on your own desk, for the limits you set yourself. |

**Allow by default.** The network is open for everyone. What a parent blocks
— a category, an app, a site — is blocked for real. Presets pre-block only
what is never age-appropriate: adult content and gambling for everyone
under 18, dating for under-12s, and web proxies (the usual way around a block).

## 2. People

Everyone has an account. Each OS login on a computer is one person
("Who's who" links logins to people). A person's **age bracket** sets their
starting rules and how much of their day a parent sees
(`policy/src/lib.rs` `AgeBracket`, `server/src/presets.rs`,
`server/src/usage.rs`).

| Bracket | Starts with | Can ask for more time | A parent sees |
|---|---|---|---|
| **Little** 0–6 | 45 min a day, 08:00–19:00, bedtime 19:00 | No | minutes, apps, sites |
| **Kid** 6–12 | 1 h a day, school days 07:00–20:00, bedtime 20:00, tasks to earn time | Yes ("Ask for 15 more minutes") | minutes, apps, sites |
| **Younger teen** 12–16 | 2 h 30 a day, bedtime 22:00, a task to earn time | Yes (15 min, 30 min or 1 h) | minutes, apps, sites |
| **Older teen** 16–18 | no limit; a parent can set one | Yes (15 min, 30 min or 1 h) | minutes, apps — not sites |
| **Adult** 18+ | nothing; keeps their own time | No | minutes only — not apps, sites or their own rules |

An adult keeps their own time on **My computer** (`/me`): a daily limit they
set (a hard stop, warned at 15, 5 and 1 minute), focus hours, and sites they
block for themselves (`/api/me/rules`). The hub can't read or change those
rules.

**Jobs — the parent:** glance (is everyone OK today?), answer a request,
pause now (one person, or every screen), set a person's rules, keep the keys
(unlock and recovery codes).

**Jobs — everyone else:** see time left today, ask for more time, know the
shape of the day (what's blocked, when screens are off), trust the deal
(what a parent can and can't see — [`TRANSPARENCY.md`](TRANSPARENCY.md)).

## 3. The console

A parent has a left rail and nothing else: the lockup, **Family · Computers ·
Settings · Me**, then **Today** — every person's ring and time left — and
sign-out (`layout/Shell.tsx`). Below 1024 px the rail is a drawer. A member
session has exactly one page, `/me`; the server refuses every other route and
the web redirects there (`App.tsx` `MemberGate`).

| Route | Screen | What it's for |
|---|---|---|
| `/login` | Sign in | Two doors: your name → a 6-digit code on your own computer, or a passkey. SSO when configured. First run: "Create your household" from the setup link. ([`AUTH.md`](AUTH.md)) |
| `/welcome` | SSO first run | Pick your name. |
| `/` | **Family** | The day at a glance: a verdict sentence, **Pause everything** (press and hold), one card per person sorted by who needs you (asking, paused, out of time, everyone else), the parent's own card last, and notices only when something is really wrong (a computer offline outside its allowed window, a login nobody has sorted). A household of one says "It's just you so far." |
| `/child/:key` | A person — **Today** | The ring and time left, requests with *Give N min* / *Not now*, **Pause** / **Resume**, **Give 15 min** / **Give 30 min**, where the time went (as their age allows), moments from the last 48 hours, their computers, and the keys. |
| `/child/:key/rules` | A person — **Rules** | Daily limit (0–8 h; 0 = no limit), when screens can be on (school days, weekend) and bedtime, what's blocked (categories, apps, sites, safe search), earning time, and **Remove** (type the name to confirm). |
| `/computers` | **Computers** | One card per computer: Online / Offline / Away, allowed / Paused / Not set up yet. Pause, Resume, **Allow offline…**, and under Details: Who's who, "Is it answering?", Rename, Remove. **Add a computer** gives the install line. |
| `/add` | Add a person | Who they are first (name, face, birthday → bracket), their computer next (the install line and their unlock code). Linux only, and it says so. |
| `/settings` | **Settings** | You, Appearance (light / dark / match my system), and Security behind "Confirm it's you": passkeys, unlock codes, the phone (Telegram), paired companions. |
| `/me` | **Me** | A person's own day: the ring, Ask for more time, their rules, their week, where the time went, their computers, and "What can a parent see?". For an adult: My computer. |

`/devices` and `/family` redirect to `/computers` and `/`.

On the computer (`client/src/`): the **app window** (`ost app`), the
**companion** that warns before a stop (`tray.rs`, notifications even
without a tray), and the **lock**, on its own screen (`lock/`). The device's
design is [`DESIGN-CLIENT.md`](DESIGN-CLIENT.md) and its words
[`BRAND-CLIENT.md`](BRAND-CLIENT.md).

## 4. Behaviour

**The stop.** Warnings at 15, 5 and 1 minute before any stop (limit,
bedtime, the end of allowed hours, a planned pause). At zero the lock takes
the screen and the person's apps are paused, never closed. A stop nobody saw
coming (a rule just changed) gets a save-your-work countdown first. A
parent's pause is immediate. Ways back: **Ask for more time**, the unlock code
typed at the lock (30 minutes), or a parent giving time or resuming from the
console.

**The rules.** One function decides everything, on the computer and in the
console alike (`policy/src/rules.rs`): a limit of 0 is no limit; a day with
no window is any time; a window ending 00:00 runs to midnight, one ending
before it starts runs past midnight; an empty window or a whole-day bedtime
is refused on save and ignored on the computer — never a 24/7 lockout. A
daily limit is one budget across all of a person's computers. Every parent
action (give time, the code at the lock, Resume) writes one override that
beats the limit, bedtime and hours until it ends; a pause beats an override.

**Fair measurement.** A minute counts only while the person's session is the
one on screen and someone used a key, the mouse or had sound in the last five
minutes. The day follows a clock the person can't move, and only moves
forward ([`TRACKING.md`](TRACKING.md)).

**Never brick.** A computer that can't show a lock freezes nobody. An
unsorted login on a parent's own computer enforces nothing until someone
sorts it. Offline, a computer keeps today's rules; offline lockdown is off
unless a parent turns it on, and the unlock code always opens it.

**Honest states.** Never show a wish as a fact: a pause in flight is
"Pausing…" until the computer confirms; a queued action says it lands when
the computer is back. A healthy day shows no red. "No limit set" is neutral,
never "0 of 0". A computer not set up yet is "Not set up yet", never
"offline".

**Confirm only what's irreversible or a key.** Reversible actions (pause,
give time, change a rule) just happen and say the result, with **Undo** where
the inverse is one call. Irreversible: removing a person (type the name) and
Pause everything (press and hold). The keys — unlock and recovery codes,
passkeys, pairing, Who's who, a new install line — ask "Confirm it's you"
(a passkey or a code on your own computer, good for 15 minutes).

## 5. Words

One word per concept, on the console and on the computer alike.

| Concept | The word | Not |
|---|---|---|
| Stop someone's screens now, reversibly | **Pause** / Paused / Pausing… | lock, freeze, lockdown, restricted |
| Deny an app, category or site | **Block** / blocked | "block" for a person |
| The day's time ran out | **Time's up** | lockout, violation |
| A managed human | **person**, or their name | member, user, kid (as the generic) |
| The parent's daily cap | **limit** | budget, allowance, quota |
| Remaining time | **time left** | remaining |
| The 6-digit code that opens a stopped computer and `sudo` | **unlock code** | parent code, PIN, backup code |
| The one-time spare keys | **recovery code** | — |
| Asking a parent | **Ask for more time** | request more time, earn more |
| Connected to the server | **Online** | connected |
| Take someone out of the household | **Remove** | block account |
| The managed machine, in prose | **computer** | device, machine |

Rules on top of the table:

- **Sentence case** for everything a person reads. No ALL-CAPS labels. The
  wordmark is `OpenScreenTime`, always.
- **Verb-first buttons**, one primary action per screen: "Pause", "Give 15
  min", "Resume everything". Not "Submit".
- **Two ways to say time** (`lib/format.ts`): `duration` ("45 min", "1 h 30
  min") and `ago` ("just now", "20 min ago", "2 days ago"); `durationShort`
  ("1 h 12") only inside a ring or a tight row.
- **The ring means one thing**: time used today, filling clockwise from the
  tick at twelve. The number beside it may say time left, because that's
  what a person asks. Never a spinner, a countdown or a hold indicator.
- **Console and computer say the same thing** for the same event: a parent
  taps Pause and the lock says "Paused by a parent"; the limit runs out and
  both say "Time's up"; the code the console shows is the one the lock asks
  for, and both call it the unlock code.
