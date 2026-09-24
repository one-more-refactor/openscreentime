# OpenScreenTime — Brand & voice of the on-device client

> The identity and copy system for the app a person sees **on the managed
> computer itself**: the app window (`client/src/app.rs`), the full-screen stop
> (`client/src/lockout.rs`), the first-run intro cards (`client/src/intro.rs`),
> and the notifications + tray (`client/src/tray.rs`).
>
> This is the most emotionally loaded surface in the product — it is the thing
> that tells a child *your time is up*. It builds **on** the product design
> language (`docs/DESIGN.md`: "time is a ring you fill"; green healthy / amber
> transition / red the one stop; Figtree; sentence case), obeys the product
> vocabulary (`docs/PRODUCT.md` §4: one word per concept), and keeps the honesty
> contract (`docs/OPENSCREENTIME.md`: "when it stops, it says it stopped";
> `docs/TRANSPARENCY.md`: what it can and can't see). Where this doc gives
> strings and `DESIGN.md`/`PRODUCT.md` give the rule, the rule wins; this doc is
> the words that fill it.
>
> **Scope note for engineers:** several strings here still live as ALL-CAPS in
> code (`tray.rs` "TIME LEFT"/"PAUSED"/"REQUEST MORE TIME", the `render_ascii`
> banner in `lockout.rs`, the `SOLVE`/`WAIT`/`ENTER UNLOCK CODE` challenge
> prompts, `App.tsx` gates). Every string below is the replacement. This doc
> defines the words; it does not edit the code.

---

## 1. What this app is — the identity in one sentence

**OpenScreenTime on your computer is a calm clock you own — it shows you the time
you have, warns you kindly before it runs out, and says so plainly when it does.
It is not a warden watching you; it is the honest face of a deal your family
made.**

Three things follow from that sentence, and everything in this doc is downstream
of them:

- **It belongs to the person using the machine, not to the parent.** The app
  window, the tray, the intro, the transparency footer — their whole job is to
  answer *the child's* questions ("how long do I have?", "what can they see?",
  "how do I ask for more?"), not to give a parent a console. The parent's console
  is the web app. On the device, the child is the user.
- **It is a clock, not an alarm.** A full ring is *normal* (`DESIGN.md` §1). The
  app is quiet almost all the time and only speaks when a human needs to act
  (`OPENSCREENTIME.md`: "silent unless a human is needed"). It never re-engages,
  never nags, never gamifies attention back onto the screen.
- **It is honest to a fault.** It never softens the stop into a euphemism, and it
  never overstates what it sees. The stop is firm; the tone is kind; the facts
  are exact. When it stops, it says it stopped (`OPENSCREENTIME.md` §Enforcement).

---

## 2. Voice & tone principles

### 2.1 The five principles

1. **Calm, kind, and firm — in that order, never traded against each other.**
   The stop is real and we don't hide it, but the words around it are warm. "That's
   it for today" is both true and gentle. We never make the limit sound negotiable
   to soften it, and we never make it sound punitive to enforce it. Firmness lives
   in the *fact* ("Time's up for today"); kindness lives in *how we say it* and in
   *what comes next* ("Ask a parent, or earn a few more minutes").

2. **Plain words, no euphemism, no cop.** When it stops, it says *stop* — not
   "session suspended", not "access restricted", not "you've been locked out for
   violating." A child understands "your time is up for today." We never dress the
   stop as a system event or a punishment. (`OPENSCREENTIME.md`: "Plain words:
   when it stops, it says it stopped." `PRODUCT.md` §4: retire "lockout",
   "restricted", "frozen" from anything a person reads.)

3. **Honest about power, both directions.** We say exactly what a parent did
   ("A parent paused this computer") and exactly what the app can and can't see
   ("It cannot see your screen, your messages, what you type, or where you
   browse"). We never imply more control than exists (no fake "we're always
   watching"), and never less (no hiding that a limit is enforced). The
   transparency footer is always in view in the app window; it is not fine print.

4. **Speak to a person, not a device.** Sentence case, one humanist sans
   (Figtree), second person ("your time", "you're back"). Numbers mean something a
   person can act on — "45 min", "2 min left" — never raw fields, never
   "TIME LEFT: 90 MIN", never an LED or a status code. No ALL-CAPS anywhere; caps
   are the surveillance voice the brand fled (`DESIGN.md` §7, `DESIGN-AUDIT.md`).

5. **Quiet by default; one thing to do.** Every screen has at most one action, and
   most screens have none — they just answer a question. The app window's only
   verb is "Ask for more time." The stop screen offers one way forward. We do not
   fill calm moments with tips, streak-bait, or upsell. Silence is a feature.

### 2.2 How tone scales across age brackets

Same honesty, same facts, different words. The bracket is known on-device
(`AgeBracket` drives `lock_copy` in `runner.rs`), so copy can key off it. The
rule: **younger = fewer words, warmer, no jargon, no autonomy it doesn't have;
older = plainer, more data, more agency, less hand-holding.** The *fact* never
changes across brackets — only the framing.

| Bracket (from `OPENSCREENTIME.md`) | Voice | What changes |
|---|---|---|
| **0–6 Little** | Warmest, shortest, concrete. No numbers a 6-year-old can't read, no "request", no jargon. A grown-up handles everything. | "All done for today. Time to do something else!" No "ask for more" UI (`OPENSCREENTIME.md`: no request UI for Little). No wind-down math. |
| **6–12 Kid** | Warm and encouraging, simple sentences, second person. Can ask for more and earn time. | "That's it for today. Want more? Ask a parent, or earn a few minutes." Full "Ask for more time." Wind-down is gentle: "2 minutes left — good time to save." |
| **12–16 Younger teen** | Calm, matter-of-fact, respectful. Own stats, goals, a real wind-down countdown. Less cheer. | "Time's up for today — 90 of 90 minutes used. Screen pauses in 60s — save your work." Shows the numbers. |
| **16–18 Older teen** | Plain and peer-level. Mostly self-set goals; a stop only where a parent capped. Zero cheerleading, zero condescension. | "Time's up — you've hit today's limit. 60s to save." Treats them as an adult who chose most of this. |
| **Adult 18+** | Neutral, self-tracking. No parent, no external enforcement — only self-set limits. | "You've reached the focus limit you set. Ending in 60s." Framed as *their own* limit, never imposed. |

Worked example — the daily-limit stop, one fact across five brackets:

- **Little:** "All done for today. Nice work — go play!"
- **Kid:** "That's it for today. You used all 90 minutes. Ask a parent, or earn a few more."
- **Younger teen:** "Time's up for today — 90 of 90 minutes used. Screen pauses in 60s, so save your work."
- **Older teen:** "Time's up — that's today's limit (90 min). 60s to save."
- **Adult:** "You've used the 90 minutes you set for today. Pausing in 60s."

---

## 3. The ring as the on-device mark

The activity ring is the product's whole identity (`DESIGN.md` §1) and it is the
**same mark** on the device that it is on the favicon, the console, and `/me`.
On-device it is already drawn, not decorative, by the egui painter (`app.rs
ring()`, `lockout.rs ring()`): a `SUNKEN`/`LINE` track circle with a `BRAND`
green arc from twelve o'clock, clockwise, round line-caps.

### 3.1 What the mark does, per state

The ring is the emotional register of the whole client. It carries state through
**colour and fill**, never through motion-as-alarm.

| State | Ring | Colour | Reads as |
|---|---|---|---|
| **Running / plenty of time** | fill grows with time used; a calm partial arc | `BRAND` green (`#2e7d46`) | normal, healthy, yours |
| **Wind-down (approaching a stop)** | fill near-complete; the arc shifts to amber | `WARN` amber (`#8a6300`) | "wrap up soon" — a transition, not a threat |
| **Time's up / stopped** | ring complete, a padlock or "Stop" glyph inside | `STOP` red (`#b3151c`), used **once** | the one clear interrupt |
| **Paused by a parent** | a dashed `--ink-3` ring, disc dimmed to ~0.6 (`DESIGN.md` §Avatar) | neutral ink, **not red** | "someone paused this, nothing is broken" |
| **No limit set** | a plain disc, no ring (`DESIGN.md`: no target → no ring) | — | neutral; not a reward, not a gap |
| **Wrong unlock code** | the ring/segment flashes `STOP` once, then settles | `STOP`, momentary | "not that code" — corrective, not punitive |

A calm day shows **no red at all** (`DESIGN.md` §1). Red appears at exactly two
moments: the hard stop, and a wrong code. Amber is the only "attention" colour,
and it means *transition* (wind-down, offline catching up), never emergency.

### 3.2 What the mark must never do

- **No alarm, no shake, no siren, no flash-loop.** The wrong-code feedback is a
  single `STOP` flash and done (`DESIGN.md` §6: "no shake, no siren. Calm, not
  punitive"). Enforcement is firm; the mark stays composed.
- **No red on a healthy screen.** Red is reserved for the stop and the wrong
  code. A ring at 80% is amber at most.
- **No second chart language.** If it's about time, it's the ring (or a bar that
  reads like the ring). Never a gauge, a countdown dial, a progress spinner, or a
  battery metaphor.
- **No countdown that looks like a bomb.** The wind-down number is a calm amber
  line ("pausing in 60s"), paired with the ring — not a big red ticking numeral.
- **Never coloured by identity.** The disc holds who-you-are; the ring holds
  state only (`DESIGN.md` §Avatar).

---

## 4. The copy system — every on-device moment

Real strings for every state. **Sentence case throughout. No ALL-CAPS. One word
per concept** (`PRODUCT.md` §4): *pause* (a parent stops screens, reversible),
*block* (content only), *time's up* → the screen says **Stop**, *unlock code*
(the code that reopens time), *computer* (in prose), *time left*, *ask for more
time*. Where a bracket changes the words, it's marked; otherwise one string
serves all.

### 4.1 Running / at-a-glance (the app window — `app.rs`)

The headline the window leads with. Replaces the current mix; keeps the good
existing `app.rs` strings and fixes the rest.

| Situation | Headline | Sub-line |
|---|---|---|
| Time left, plenty | **"1 h 30 min left"** (ink) | "45 min used today" |
| Time left, ≤15 min | **"12 min left"** (amber, not red) | "Good time to start saving." |
| No limit today | **"No limit today"** (ink) | "Use it how you like." |
| Time's up (still in window) | **"Time's up for today"** (red) | Kid: "Ask a parent, or earn a few more." · Teen+: "You've hit today's limit." |
| Paused by a parent | **"Paused"** (red) | "A parent paused this computer. It comes back when they lift it — or ask them." |
| Not a managed user here | **"This computer is managed"** (ink) | "You're not on a limit here." |

Connection line (a small chip under the headline; keep the honest `app.rs`
phrasings, drop any caps):

- Online → **"Connected"** (green dot)
- Offline, soft → **"Offline — it catches up when it's back"** (faint, not red)
- Offline, fail-closed → **"Offline — locked until it reconnects"** (red)
- Agent not running → **"Not running"** (faint)

> Vocabulary note: the console word is **Online** (`PRODUCT.md`), but the
> on-device chip may say "Connected" as the friendly first person — keep it if
> the console and device agree the state is the same thing. If in doubt, use
> **"Online"** to match the console verbatim.

### 4.2 The wind-down (the countdown before a stop)

A calm heads-up, then a save-your-work grace. The grace is real (60s for
screen-time stops; `TRANSPARENCY.md`), and the copy names it plainly. Amber, ring
shifting toward amber. No countdown for Little (a grown-up manages it).

**Early nudges (10 min / 2 min before — Kid and up):**

- 10 min: **"10 minutes left today"** — "A good time to find a stopping point."
- 2 min: **"2 minutes left today"** — "Save what you're working on now."

**Bedtime approaching (up to 15 min before):**

- **"Bedtime soon"** — "Screens go off in 15 minutes. Time to wind down."

**The save-your-work grace (on-screen, once the stop lands, amber):**

- Kid: **"Saving your work — screen pauses in 45s."**
- Teen+: **"Screen pauses in 45s — save your work."**

(Keeps the existing `lockout.rs` amber line "Saving your work — pausing in {n}s";
this is the canonical wording.)

### 4.3 The hard stop (the full-screen `lockout.rs`)

The heart of the product. The stop reason is generated from `LockReason` in
`enforce/screentime.rs` (`headline()` + `detail()`) and by `runner.rs lock_copy`,
so the overlay, the headless broadcast, and the console **all say the same
words** — this is the console↔device lockstep (`PRODUCT.md` §4). The strings
below are those `headline()`/`detail()` values, confirmed and, where noted,
tuned.

Each stop is: **one headline (the word), one honest detail (the fact + what's
next), one calm action.** Never a wall of red, never a scold.

| Stop reason | Headline | Detail (Kid) | Detail adds (Teen+) |
|---|---|---|---|
| **Daily limit** (`DailyLimit`) | **"Stop"** | "Time's up for today — you used all {limit} minutes. Ask a parent, or earn a few more." | "…{used} of {limit} minutes used. Screen pauses in {n}s — save your work." |
| **Bedtime** (`Bedtime`) | **"Goodnight"** | "Screens are off until morning. See you tomorrow." | "Screens are off until morning." |
| **Outside allowed hours** (`OutsideWindow`) | **"Not now"** | "Screens are off at this time of day. They come back later." | "Screens are off at this time of day." |
| **A parent paused it** (device admin lock) | **"Paused"** | "A parent paused this computer. Save your work — it comes back when they lift it." | same |
| **Offline lockdown** (`offline_hard_lockdown`) | **"Offline for too long"** | "This computer needs to reach home to keep going. Connect it to the internet, or ask a parent for the unlock code." | same |
| **Tamper lockdown** (`tamper_lockdown`) | **"Locked"** | "OpenScreenTime was changed and stopped working, so the computer locked. A parent's unlock code opens it." | same |

Design notes on the stop screen:

- **The action button.** Current code fills it with "TAP TO CONTINUE" / the
  challenge action. Replace with a calm, honest label: **"Enter unlock code"**
  when a code can open it, **"OK"** for a plain acknowledgement (bedtime,
  outside-hours where nothing to do), or the earn/ask path where that exists.
  Never "CONTINUE" in caps.
- **The unlock-code prompt** (the parent-present escape). Keep the good `lockout.rs`
  faint-label pattern; the words:
  - Label: **"A parent's unlock code"** — sub: "From the console, or a recovery code."
  - Field hint: **"123 456"** (Space Mono — the one place mono survives,
    `DESIGN.md` §3).
  - Wrong code: **"That's not the code — try again."** (red, one line, one flash;
    never "ACCESS DENIED").
  - Rate-limited: **"Too many tries — wait a minute, then try again."**
- **The challenge prompts** (self-serve breather; replace the caps in
  `challenge::prompt`):
  - Math: **"Quick one: {a} × {b} = ?"** — field hint: "type your answer"
  - Wait: **"Take a {n}-second breather."**
  - (Little never sees a challenge; a grown-up unlocks.)

### 4.4 "Ask for more time"

One verb everywhere — the app window, the tray, `/me` (`PRODUCT.md` §4:
"Ask for more time" is *the* label; kill "REQUEST MORE TIME" / "REQUEST SENT" /
"earn more" as a stray verb). Kid/teen brackets only; Little has none; adults
have no one to ask.

- Button (idle): **"Ask for more time"**
- Button (in flight / sent): **"Asked — waiting for a parent"** (disabled)
- Confirmation line: **"Sent — a parent can say yes."**
- If it couldn't send: **"Couldn't send — try again in a moment."**
- Where earning exists (Kid): **"Earn a few minutes"** as a second, quieter
  option — only shown where a task can actually be picked (`PRODUCT.md` failure #8:
  don't promise earning with no button).

### 4.5 The sign-in code (`logincode.rs`, the app window's code card)

The person typed their name on the sign-in page; their own computer shows a
6-digit code for them to type there (docs/AUTH.md). A trust moment — plain,
slightly guarded, never alarming. Nothing to tap: the device only shows.

- Title: **"Your sign-in code"** (or **"Your confirm code"**), the code spaced
  "123 456".
- Line: **"Type it into {host} to sign in as {name}. Didn't ask? Ignore it."**
- Then: **"Works for {n} more minutes."**
- Surfaces: a card at the top of the app window (which comes forward), one
  desktop notification, and `ost code` in a terminal.

### 4.6 First-run intro cards (`intro.rs`)

The child-facing documentation — a few honest cards, skippable, shown once. The
current `intro.rs` copy is already close to right (it matches `TRANSPARENCY.md`);
this is the canonical set, warmed slightly and made bracket-neutral (a 10-year-old
and a 15-year-old both read these — keep them simple enough for the younger, honest
enough for the older). Wordmark ring on each; sentence case; "Skip" always
available.

1. **"This computer keeps track of screen time"**
   "It counts your screen time and blocks a few things online. Here's the honest
   version — swipe through, or skip."
2. **"What a parent can see"**
   "How much screen time you've used, on which computer, roughly where the time
   went, and whether someone changed OpenScreenTime. That's the whole list."
3. **"What it can't see"**
   "Not your screen. Not what you type. Not your messages, and not the pages you
   visit. It counts time and filters the network — it doesn't watch you."
4. **"When your time's up"**
   "You get a daily limit. You'll get a heads-up when it's nearly gone, and 60
   seconds to save your work before the screen pauses. Nothing gets deleted."
5. **"Need more time?"**
   "Open OpenScreenTime from your apps and tap *Ask for more time*. A parent gets
   the request and can say yes."
6. **"That's the deal"**
   "No camera, no microphone, no reading your messages, no remote control of your
   computer. It only enforces time and network rules — and everything it does
   shows up right here."

Nav: **"Next"** / on the last card **"Done"**; **"Skip"** top-right; the counter
is a quiet "{n}/{total}" in the faint ink (drop the monospace on it — Figtree with
tabular figures, per `DESIGN.md` §3).

### 4.7 The transparency promise ("what this can and can't see")

Always in view in the app window footer (`app.rs` already does this — keep it) and
restated in the intro. It is the product's whole thesis in one sentence
(`TRANSPARENCY.md`). Canonical wording:

> **"OpenScreenTime counts your screen time and filters the network. It can't see
> your screen, your messages, what you type, or the pages you visit — and it only
> does what's listed here."**

The tray's "About" item and any "what is this?" affordance point at the same
promise, never at a marketing line.

---

## 5. Notifications voice (freedesktop / `notify-rust`)

Fire on **state transitions only**, never on a timer, never to re-engage
(`tray.rs notify_transitions`; `OPENSCREENTIME.md`: "silent unless a human is
needed"). Title = the fact in a few words, sentence case; body = what it means or
what to do next. `appname` is always **"OpenScreenTime"** (never
"OPENSCREENTIME"). Critical urgency only for a genuine interrupt (imminent freeze,
fail-closed lockdown, tamper), normal for the rest. **Every one of these replaces
an ALL-CAPS string currently in `tray.rs`.**

### Time warnings (per user)

| Trigger | Title | Body | Urgency |
|---|---|---|---|
| 10 min left | **"10 minutes left today"** | "A good time to find a stopping point." | normal |
| 2 min left | **"2 minutes left today"** | "Save what you're working on now." | critical |
| Freeze imminent | **"Screen pauses in {n}s"** | "Save your work." | critical |
| Time's up (froze) | **"Time's up for today"** | Kid: "Ask a parent, or earn a few more." · Teen: "You've hit today's limit." | critical |
| Back after a stop/pause | **"You're back"** | "Have fun." | normal |

### Grants & denials (the answer to a request)

| Trigger | Title | Body | Urgency |
|---|---|---|---|
| Request sent (to the child) | **"Asked for more time"** | "Waiting for a parent to answer." | normal |
| Time granted | **"You got {n} more minutes"** | "Back to it — enjoy." | normal |
| Request denied | **"Not this time"** | "A parent said no for now. You can ask again later." | normal |

> Denial voice matters: **"Not this time"**, never "DENIED", never "REQUEST
> REJECTED." A refusal is honest but not a verdict on the person. The child is
> told clearly (`TRANSPARENCY.md`: "you're told clearly if it's denied instead of
> being left hanging") — kindly, not coldly.

### Parent-side (this device is paired / parent mode)

| Trigger | Title | Body | Urgency |
|---|---|---|---|
| New time request | **"{name} asked for {n} more minutes"** | "{task} · {computer}" | normal |
| You approved | **"Approved"** | "{name} has {n} more minutes." | normal |
| You denied | **"Declined"** | "{name}'s request for more time." | normal |
| Couldn't reach server | **"Couldn't update"** | "Check the connection and try again." | normal |

### Connection & device state

| Trigger | Title | Body | Urgency |
|---|---|---|---|
| Went offline (soft) | *(silent — it catches up; don't notify a blip)* | — | — |
| Offline lockdown engaged | **"Offline for too long"** | "This computer is locked until it reaches home again. A parent's unlock code opens it." | critical |
| Back online | **"Back online"** | "Connected to home again." | normal |
| A parent paused it | **"Paused"** | "A parent paused this computer. Save your work." | critical |
| A parent resumed it | **"Unpaused"** | "This computer is back to normal." | normal |
| Tamper lock engaged | **"Locked"** | "OpenScreenTime stopped working and the computer locked. A parent's unlock code opens it." | critical |
| Tamper lock lifted | **"Unlocked"** | "Back to normal." | normal |

---

## 6. What we deliberately reject

The failure modes this brand refuses — each is a real temptation in a screen-time
product, and each would break the identity in §1.

- **Fake gamification.** No confetti, no "streak lost!" guilt-bait, no coins,
  badges, XP, or mascots celebrating that you stopped. Earning time is a *plain
  transaction* (a task for minutes), not a game loop. Encouragement lives in the
  calm ring and the honest number, not in dopamine theatre
  (`OPENSCREENTIME.md`: "No mascot, no confetti storms").
- **Guilt & shame.** Never "you've been on too long", "again?", "you always do
  this." The app reports facts and offers a next step. It has no opinion about the
  person's worth. A stop is "that's it for today", not a reprimand.
- **Cop / surveillance language.** No "VIOLATION", "ACCESS DENIED", "UNAUTHORIZED",
  "you have been locked out", no ALL-CAPS, no LED status lights, no timestamps
  dressed as evidence, no "monitoring active." That was the retired "Nothing"
  surveillance voice (`DESIGN.md` §7, `DESIGN-AUDIT.md`) and it is exactly the
  impression the brand exists to escape. This app is a clock, not a security
  appliance.
- **Euphemism that hides the stop.** The opposite failure, and just as
  forbidden. No "session paused for wellness", no "you've reached a great
  stopping point!" pretending the limit was the child's idea, no burying "your
  time is up" under cheer. When it stops, it says *stop* (`OPENSCREENTIME.md`).
  Kindness is in the tone and the next step — never in obscuring the fact.
- **Overstated power.** Never imply it sees more than it does ("we're always
  watching", "we know what you did"). That would be a lie and a betrayal of the
  transparency contract. It sees what `TRANSPARENCY.md` lists and nothing more,
  and it says so.
- **Nagging / re-engagement.** No "come back!", no idle-time pings, no
  notifications that aren't a real state change a human must act on. Silence is
  the default and a feature.
- **Cutesy-fake warmth.** Warmth is real (plain kind words, a calm mark), never a
  cloying persona. No emoji-speak, no "Oopsie! Time's all gone! 🥺", no baby-talk
  even for Little — Little gets *simple*, not *saccharine*.

---

## 7. Quick reference — the hero copy

The three moments that define the client. If only these three are right, the
product feels calm, kind, and honest.

**Running (app window headline):**
> **"1 h 30 min left"** — "45 min used today."
> Green ring, quiet, one button: *Ask for more time.*

**Wind-down (on-screen grace, amber):**
> **"2 minutes left today"** — "Save what you're working on now."
> then: **"Screen pauses in 60s — save your work."**

**The hard stop (full screen):**
> **"Stop"** — "Time's up for today — you used all 90 minutes. Ask a parent, or
> earn a few more." *(Kid)*
> Ring complete in red, one calm line, one way forward. No alarm, no scold, no
> euphemism.
