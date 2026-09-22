# OpenScreenTime — Product Definition

> Written for the engineer about to rewrite the client. This is the **what** and
> the **behaviour**; a separate design lead owns the **look**. Where this
> conflicts with `DESIGN.md`, this wins on product and behaviour; where it
> conflicts with `CONTRACT-0.6.md` on the enforcement/login model, 0.6 wins.
> Cited files are under `web/src/` and `client/src/` unless noted.
>
> The bar to clear, in the owner's words: today "most of the stuff is completely
> unusable" and "looks like someone told a chatbot to do it." The cause is not
> missing features — the console is over-built. It is **too many words for the
> same thing, two controls for the same job, and a dozen screens' worth of dead
> components shipped alongside the live ones.** This document cuts.

---

## 1. The product in one paragraph

OpenScreenTime is a **self-hosted screen-time manager for a whole family** —
including adults with no kids, who only track themselves. One parent is the
**hub**: they run the household from a web console — see everyone's day at a
glance, pause a screen (or the whole house) now, set a daily limit and rules per
person, answer requests for more time, and hold the keys to unlock a device.
Every managed person also has **one page of their own** (`/me`) plus a calm
on-device app and, when the day's time is spent, a full-screen stop. The core
promise: **see the family's day in three seconds, and put a real, honest limit
only where you actually drew one** — allow by default, block for real what you
chose to block, and help people reduce screen time without nagging or
surveillance. It is not "parental controls." It is the family's shared clock,
and it practises what it preaches: silent unless a human is needed.

**Primary jobs — parent (the hub):**
1. **Glance:** is everyone OK today? Who's over, who's paused, who's asking.
2. **Answer:** grant or refuse a request for more time.
3. **Pause now:** stop one person's screens — or every screen in the house — and resume just as fast.
4. **Set the rules for a person:** daily limit, allowed hours, bedtime, what's blocked.
5. **Keep the keys:** read a device's unlock code / recovery codes when someone's locked out.

**Primary jobs — member (child / teen / self-tracking adult):**
1. **See time left today** — one number, answered at a glance.
2. **Ask for more time** (kid/teen brackets).
3. **Know the shape of the day** — what's blocked, when screens are off.
4. **(teens/adults) Own a goal and see the week** — the shift from an imposed cap to a target you chose.
5. **Trust the deal** — know exactly what a parent can and cannot see.

---

## 2. Information architecture

### Navigation model
- **Parent/hub — a left rail, nothing else** (`layout/Shell.tsx`). Rail = wordmark → **Family · Devices · Settings** → a live jump-list of people ("Today") → identity + theme + sign-out. No top bar. On mobile, one floating button opens the rail as a drawer. This is right; keep it.
- **Member — no navigation at all.** A member session is one page (`/me`) and can reach nothing else; the server 403s every other route and `MemberGate` (`App.tsx`) redirects to `/me`. Keep it.

### Full route / screen inventory
| Route | Screen | Who | Keep / change |
|---|---|---|---|
| `/login` | Login / first-run register | everyone | keep |
| `/welcome` | SSO name-pick (first SSO sign-in) | new SSO hub | keep |
| `/` | **Family** (home) | hub | keep — the best screen in the product |
| `/family` | duplicate of `/` | — | **CUT the alias**; one home route |
| `/child/:key` | **Person** detail (today `ChildDetail.tsx`) | hub | keep the route, **split the page** (§ below) |
| `/devices` | Devices | hub | keep, de-jargon |
| `/add` | Add a person (`AddChild.tsx`) | hub | keep |
| `/settings` | Settings | hub | keep, trim |
| `/me` | The person's own page | member + hub-viewing-self | keep |
| (gate) | `Enroll2FA` first-login 2FA | hub | **demote** from forced gate to soft prompt |

On-device client surfaces (`client/src/`): the **app window** (`app.rs`), the
**full-screen stop** (`lockout.rs`), the **first-run intro cards** (`intro.rs`),
the **tray** (`tray.rs`). These must speak the console's vocabulary (§4); today
they don't.

### Per screen: purpose · must-have · primary action · cut

**Login (`pages/Login.tsx`)**
- *Purpose:* one door in. Fresh install → create the first account (passkey only). Otherwise → type username, your own computer approves (number-match), passkey beneath as fallback.
- *Must-have:* username field; the number-match "check your computer" state; passkey fallback; SSO button when configured.
- *Primary action:* Continue (device-approval sign-in).
- *Cut:* the mock-only "Enter design review (skip auth)" button must never ship in a prod build (already `mock`-gated — keep it gated).

**Family (`pages/Family.tsx`) — home**
- *Purpose:* the family's day at a glance. One honest verdict sentence, Pause Everything, a card per person (ring + name + time-left bar + "asking" flag), the parent's own card, trouble only when real.
- *Must-have:* the verdict sentence; `PauseEverything`; the person grid sorted "who needs me first"; first-run setup when empty.
- *Primary action:* open a person, or Pause Everything.
- *Cut:* nothing — this screen is the model the rest should follow.

**Person (today `pages/ChildDetail.tsx`) — OVER-BUILT, split it**
- *Purpose (should be):* everything about one person, control-first. Today's ring + time left → requests waiting → **Pause / give time** → where they use it → the keys.
- *Must-have:* today's number; requests; pause + "give 15/30 min"; devices; unlock code ("keys"); the age/face/look identity control.
- *Primary action:* Pause their devices (or answer a waiting request).
- *CUT / MOVE, this is the bloat:*
  - **The "Protection" `SecuritySlider` — remove it entirely.** It is a *second, competing* model of the same policy the granular rules already edit, and it silently rewrites fields the parent set by hand (see failure #1). One rules model, not two.
  - **Move "The rules" (the whole `ChildRules.tsx` suite) to its own subpage/section**, reached from the person page — not stacked inline under the glance. The glance page and the editor are two jobs.
  - **"Block account" (Danger Zone) — fold into Pause + Remove** (failure #2). It is a third lock verb that overlaps Pause.
  - "Where {name} uses it" (devices list) and `Moments` stay, but below the fold; they are reference, not the reason a parent opened the page.

**Rules (extracted from `ChildRules.tsx`)**
- *Purpose:* the full parenting suite for one person: apps & categories, daily limit, allowed hours, bedtime, websites (blocklist), safe search, earning time, what happens when time runs out.
- *Must-have:* all of the above — each control is deliberate and each maps to a real enforced field. Keep them.
- *Primary action:* every change is one save = one (rare) step-up.
- *Cut:* the `TimesUp` "Parent code" option copy must use the one credential name — "unlock code," not "parent code from your authenticator app" (there is no authenticator on the device; §4).

**Devices (`pages/Devices.tsx`)**
- *Purpose:* the machinery, kept human — one card per device: steady? paused? last heard? allowed to be offline?
- *Must-have:* the verdict sentence; per-device Pause/Resume; "allow offline" window; last-heard/steadiness.
- *Primary action:* Pause / Resume a device.
- *Cut:* **"Ping"** is a developer verb on a parent screen — rename to "Check it's on" or drop. Soften "not calling home"/"patchy"/"silent" to plain words. Recovery-code counts belong under Settings/keys, not on the connectivity card.

**Add a person (`pages/AddChild.tsx`)**
- *Purpose:* a person first (name, face, birthdate → bracket → starting rules), their computer second (one-line install + the unlock code).
- *Must-have:* both steps; the "make recovery codes now" nudge; the "Linux only for now" honesty.
- *Primary action:* Continue → Done.
- *Cut:* nothing structural; rename to match the person vocabulary (the page can still say "child" when the bracket is a child, but the concept is "person").

**Settings (`pages/Settings.tsx`)**
- *Purpose:* You + Appearance (free to read) and a locked "Security & access" back room (unlock codes, second factor, phone companion, passkeys, paired companions).
- *Must-have:* You; Theme; the confirm-gated Security room.
- *Cut / question:* the back room has **five** auth mechanisms — unlock codes, TOTP, Telegram, passkeys, and "paired companions" (parent tokens). For a family product that is a lot of surface. At minimum group them; seriously consider whether **Telegram companion + paired-companion tokens** both need to exist, or whether one "phone/companion" concept covers it. `mock` review banner must stay mock-gated.

**Me (`pages/Me.tsx`)**
- *Purpose:* one page, one question — how much time is left today — then ask/goal/week/blocked/schedule/devices, in three age-keyed looks (playful/calm/plain).
- *Must-have:* the ring or big number; Ask for more time; the transparency intro + standing "what can they see?"; the goal; the week.
- *Primary action:* Ask for more time (kid/teen) / set a goal (teen/adult).
- *Cut:* nothing major — this page is strong. Keep the `WhereTheTime` from appearing twice (the week already shows per-device); one attribution block.

---

## 3. States, per screen — where "unusable" lives

Every screen must define these. The product's honesty rule: **never show a wish
as a fact** (a pause in flight is "Pausing…", not "Paused"), and **a healthy day
shows no red and no trouble.**

**Family**
- *Loading:* skeleton grid, real layout in outline (already: `FamilyWaiting`), verdict line shows ` `. No spinner.
- *Empty:* `FirstRun` — three honest steps + "Add the first person." Not a lonely button.
- *Error:* one-line `fam-error` above the grid; do **not** show FirstRun on error (already handled).
- *Success:* verdict sentence + sorted cards + the parent's own card.
- *Offline/stale:* `data-refreshing` dims quietly; last-good data stays on screen.
- *Paused / pausing:* card shows "Paused" only when devices confirm; "Pausing…" while in flight.
- *Trouble:* only a genuinely dark device (offline and **not** in an allowed-offline window) surfaces `Trouble`; otherwise nothing.

**Person / Rules**
- *Loading:* "Loading…" quiet text while the family store fills.
- *Not found:* "There's no one with that name in your family." + Try again. (Keep.)
- *Empty (no devices):* "No devices yet. Set one up." Keys section: "No set-up device yet — a code appears once they have one."
- *Error on a change:* inline `fam-error`; the optimistic note is replaced, never left lying.
- *Success on a change:* one plain note stating the result ("Gave {name} 15 more minutes."). Reversible → no confirm.
- *Permission-denied / step-up:* `guard()` turns a server 428 into "confirm it's you," then retries; cancelling is a silent no-op (`StepUpCancelled`). Never a dead end.
- *Irreversible (remove person):* modal + type-the-name. (Keep — the only type-to-confirm in the product.)

**Devices**
- *Loading:* "Checking on every device…" (keep), last-good list shown instantly on revisit.
- *Empty:* "No devices yet. A device joins when you set up a person on it." + CTA.
- *Error:* whole-page "Couldn't load the devices." + message.
- *Per-device states:* connected / pausing… / paused / resuming… / waiting to join (pending) / away · allowed / not calling home. Each is one word, tone-coloured, **derived from what the agent actually reported** — never optimistic.
- *Action failed / offline:* "That didn't work — the computer may be off; it catches up when it's back." Queued-not-delivered is stated as such.

**Me**
- *Loading:* "…" quiet text; the ring draws itself on arrival.
- *Error:* "Couldn't load your day" + Try again; the week failing is **not** an error (decoration).
- *Spent / stopped:* ring shows "Stop" + "time's up for today" or "paused by a parent" — the two are distinguished honestly.
- *Paused by a parent (blocked):* a calm status line, "Nothing here is broken — talk to them, and it comes back."
- *Offline:* the page polls every 30s; living data updates in place.
- *First visit (member):* the transparency intro, once per browser.

**Login / Welcome**
- *Idle / waiting:* "Check your computer" + the three-number match code + a progress bar.
- *Error:* one `role="alert"` line; a failed sign-in points at the passkey fallback.
- *Registration closed / username taken:* precise inline messages (keep).

**Cross-cutting empties to get right:** an unconfigured limit is **"no limit
set"** (neutral, not a reward); "0 of 0" is never shown (`limit_minutes: null`
means no limit, per `types.ts`). A pending device is "not set up yet," never
"offline."

---

## 4. Consistency rules the whole client must obey

The single biggest lever on "usable." Today the same concept has 3–5 names
across web + client (measured: in `web/src` alone, `locked` ×74, `blocked` ×61,
`paused` ×58, `pause` ×43, `lock` ×39, `block` ×38; `child` ×118 vs `member`
×21 vs `person` ×17). **Pick one word per concept and enforce it everywhere,
console and device.**

### The vocabulary (authoritative)
| Concept | The ONE word | Kill these synonyms |
|---|---|---|
| Temporarily stop a person's / house's screens, reversible | **Pause** ("Paused" / "Pausing…") | lock, locked, freeze, frozen, lockdown, "restricted" *(all internal-only)* |
| Deny access to an app / category / website (content) | **Block** ("blocked") | *never use "block" for a person* |
| The day's time ran out → hard stop | **Time's up** → the screen says **Stop** | "lockout" (internal only) |
| A managed human (kid/teen/adult) | **person** (generic); the name otherwise | "child" as the generic (keep "child" only where the person truly is one), "member" (data-layer only), "kid" |
| The parent's daily cap | **limit** | budget, allowance, quota |
| The person's own target | **goal** | — (already distinct — keep it) |
| Remaining time | **time left** | "remaining," "left today" vs "minutes left" drift |
| The per-device 6-digit code that unlocks a screen / reopens time / allows sudo | **unlock code** | "parent code," "parent PIN," "PIN," "backup code" *(all internal-only)* |
| The one-time spare keys | **recovery code** | — |
| A member asking for time | **Ask for more time** | "REQUEST MORE TIME," "REQUEST SENT," "earn more" as a verb with no button |
| Connected to the server | **Online** | "Connected" |
| Suspend a person's account entirely | **Remove** (or "Suspend") | "Block account" *(collides with content "block")* |
| The managed machine (in prose a person reads) | **computer** | "machine," "screen" (as the object), "device" *(keep "Devices" only as the section/machinery name)* |

Two hard rules on top of the table:
1. **Sentence case for everything a human reads.** ALL-CAPS is retired
   (`DESIGN.md`) but still live in `tray.rs` ("TIME LEFT", "PAUSED", "REQUEST
   MORE TIME"), `App.tsx` ("AUTHENTICATING…" + `StatusLed`), and stray labels
   (`Devices.tsx` "allow offline for"). Caps + LED read as the surveillance tool
   the brand fled. Kill them.
2. **The wordmark is `OpenScreenTime`, always** — never `OPENSCREENTIME`
   (`tray.rs` uses both).

### Buttons / actions
- One pill grammar: **primary** (ink-filled, one per screen), **secondary**
  (hairline), **danger** (hairline that turns red only when reached for),
  **ghost**. The legacy caps-mono `Button` variant converges on the pill.
- **Verb-first, plain labels:** "Pause their devices," "Allow," "Give 15
  minutes," "Resume everything." Not "Submit," not nouns.
- One primary action per screen; everything else is quieter.
- **Route every button through `Button`.** Many live components hand-roll
  `.ch-btn`/inline buttons and disagree on casing *within one file*
  (`UnlockCodePanel` row buttons are sentence case, its modal footers are
  ALL-CAPS), and destructive styling is misapplied (the everyday Pause/Lock is
  dressed `danger` in the orphaned `DeviceCard`, while a real delete is not).
  One component, one grammar, danger only for the genuinely destructive.

### Confirmation vs. undo
- **Reversible action → do it, then state the result** (a note or toast with the
  plain outcome). No confirm dialog. Pausing, granting time, changing a rule,
  toggling safe search are all reversible — they must not nag.
- **Irreversible action → confirm.** Only two exist: **remove a person**
  (type-the-name modal) and **Pause Everything** (hold-to-commit, `HOLD_MS`
  600 ms — the one deliberate "you meant this"). Keep both; add no others.
- **Undo where cheap:** a pause/grant toast can carry "Undo" since the inverse
  is one call. Today the app mutates **optimistically** (`lib/family.ts`
  `patchChild`) but offers **no undo anywhere** (`lib/toast.tsx` has tones and a
  dismiss ✕, no action) — add the undo, or stop mutating optimistically.
- Never confirm a thing you can instantly reverse; never make a destructive
  thing fire on a single stray click.

### How time is shown (consolidate 8 formatters → 2)
Today there are at least eight: `fmtMin` (ChildRules), `fmt`/`fmtLong` (Me),
`minutesToHm`/`relTime` (format.ts), `since()` twice (Family, ChildDetail),
`agoLabel`/`leftLabel` (Devices) — and the tray prints raw minutes with **no
hour rollover** ("90 MIN"). Replace with:
- **`duration(mins)`** → "45 min" / "1 h 30 min" (prose, everywhere, with hour rollover).
- **`ago(iso)`** → "just now" / "20 min ago" / "3 h ago" / "2 days ago" (relative time).
- **The two *pictures* stay:** the segmented bar (**one cell = 15 minutes**) and
  the ring (**fraction of the day still left**). Same grammar on the Family
  cards, the person page, and `/me`.
- "Time left" is the phrase; the number counts up to its value (living data).

### How a person is represented
One unit, everywhere: **avatar** (parent-picked emoji face, else a deterministic
monogram + warm hue) **+ name + activity ring** (used vs their goal, else the
parent's limit). It is the Family grid card, the rail dot, the person-page
header, and their own `/me` ring — identical semantics on all four. No second
representation. (Implementation coherence, for the design lead too: the ring is
currently re-coded **six** times — `AvatarRing`, `StateRing`, `CodeRing`, the
`uc-ring`, the `pause-ring`, and the `Wordmark` arc — and `hueFor` is
copy-pasted in three files. One ring primitive, one `hueFor`.)

### Console ↔ on-device coherence
The parent's console and the child's screen must **say the same words for the
same event**, because they describe one thing from two sides:
- Parent taps **Pause** → child's app/lock says **"Paused — a parent paused
  this computer"** (not "locked," not "frozen").
- Daily limit hits zero → both say **"Time's up"**; the stop screen says
  **Stop**.
- The **unlock code** the console shows is the exact code the device asks for —
  so both surfaces must call it "unlock code," never "parent code" on one side
  and "PIN" on the other.
- **Ask for more time** is the child's verb in the app, the tray, and `/me`
  alike — one label.
- A managed device that lost the server is **offline**, and the copy says it
  catches up — the console and the device agree it is not "broken."

---

## 5. Top usability failures in the current client (ranked)

1. **Two competing rule models on the person page.** `ChildDetail.tsx` renders
   *both* the granular "The rules" suite (`ChildRules.tsx`) *and* the
   "Protection" `SecuritySlider` (`components/SecuritySlider.tsx`: Off / Safe
   search / Protected / Strict). The slider **overwrites the same policy fields**
   the parent just set by hand (limit, schedule, bedtime, blocklist). A parent
   sets 90 min in the rules; the slider reads "Protected · 60 min." Nobody can
   hold two truths about one policy.
   → **Target:** delete the Protection slider. One rules model. Seed each new
   person with a bracket-appropriate **healthy default** (that is what "presets"
   are for), then let the granular rules be the only editor.

2. **"Pause" and "Block account" are two lock verbs for one job.**
   `ChildDetail.tsx` has "Pause their devices" (control row, instant, reversible)
   *and* "Block account" (Danger Zone) — which **also locks every device** plus
   makes the account read-only. A parent can't tell them apart, and "block"
   already means *content* blocking elsewhere.
   → **Target:** one **Pause** for "stop the screens now." Suspending the whole
   account is **Remove/Suspend**, described as such, not a second pause.

3. **One stop, five names.** Across console + client the same freeze is
   "paused" / "locked" / "frozen" / "lockdown" / "restricted"
   (`app.rs`, `tray.rs`, `lockout.rs`, `types.ts`, `Devices.tsx`), and the
   credential is "parent code" / "parent PIN" / "unlock code" / "backup code"
   (`ChildDetail.tsx` "Parent code," `ChildRules.tsx` "Parent code … from your
   authenticator app," `parentcode.rs`). The internal→user-word mapping is not
   1:1 and appears nowhere.
   → **Target:** the vocabulary table in §4, enforced repo-wide.

4. **The retired surveillance aesthetic is still live.** ALL-CAPS + LED where
   `DESIGN.md` says both are dead: `tray.rs` ("TIME LEFT: 90 MIN," "PAUSED,"
   "DEVICE MANAGED"), `App.tsx` RequireAuth/TwoFactorGate ("AUTHENTICATING…"
   with `StatusLed`), `Devices.tsx` "allow offline for." It reads as a
   monitoring appliance — the exact impression the brand escapes.
   → **Target:** sentence case + the quiet "breathe" loading state everywhere;
   remove `StatusLed` from live paths.

5. **The person page is one giant scroll of everything.** `ChildDetail.tsx`
   stacks: identity editor + today + where-the-time + requests + pause/grant +
   the entire rules suite + the protection slider + devices + moments + keys +
   danger zone. It is the opposite of glanceable.
   → **Target:** split into **Person** (glance + the controls a parent came for)
   and **Rules** (the editor), per §2.

6. **~12 dead components ship next to the live ones — the literal "chatbot did
   it" residue.** Exported from `components/index.ts` but rendered by no route:
   `VpnProfiles`, `EventFeed`, `UsageHistory`, `LockOverlay`, `PolicyEditor`
   (+ its only-consumers `TagInput`, `TimeRange`), `ErrorPanel`, `Panel`,
   `Stat`, `DotMatrix`, and `components/DeviceCard` (Devices uses a *local*
   `DeviceCard`, not this one). `StatusLed` survives only in the two loading
   gates above.
   → **Target:** delete them. Every shipped component is reachable.

7. **Time is formatted eight different ways, and one is wrong.** See §4 — eight
   helpers, and the tray shows "90 MIN" with no hour rollover (`tray.rs`).
   → **Target:** two formatters + the bar/ring pictures.

8. **One child action, three labels.** "Ask for more time" (`app.rs`,
   `intro.rs`, `/me`) vs "REQUEST MORE TIME" (`tray.rs`) vs "REQUEST SENT," and
   prose tells kids to "earn more" while no earn button exists in these windows.
   → **Target:** "Ask for more time" everywhere; earn offers surface only where
   a task can actually be picked.

9. **A forced 2FA gate contradicts the trust model.** `App.tsx`'s
   `TwoFactorGate` + `Enroll2FA` interrupt a hub's first login to set up an
   authenticator — but `confirm.tsx` states the model is now "trust at login;
   reading is free; mutations just work," with a factor needed only for the
   sensitive corner. So the gate demands a factor most sessions never use.
   → **Target:** demote to a soft, dismissible prompt inside Settings/Security,
   not a login wall.

10. **Back-of-house language and dead routes leak into parent surfaces.**
    `Devices.tsx` exposes a developer **"Ping,"** plus "patchy/silent/not
    calling home" and recovery-code counts on the connectivity card; `/family`
    duplicates `/`; "No limit" reads as a reward rather than "not configured"
    (`app.rs`).
    → **Target:** "Check it's on" (or cut Ping), plain connection words, keys
    under Settings, one home route, "no limit set" phrasing.

---

## 6. Rewrite plan

### Keep (these are good — build around them)
- **Family page** (`Family.tsx`) — the verdict sentence, the "who needs me"
  sort, the honest paused/pausing states. This is the product's model screen.
- **`/me`** (`Me.tsx`) — three age looks, the ring, the goal, the week, the
  transparency intro. Strong; leave it.
- **Pause Everything** (`PauseEverything.tsx`) — hold-to-commit, the sweep, and
  honest "N offline, will pause on reconnect" reporting. Exemplary.
- The **family store + rail** (`lib/family.ts`, `Shell.tsx`), the **`guard()` /
  confirm** flow (`confirm.tsx`), **AddChild** two-step, **AvatarRing**,
  **UnlockCodePanel**, and the "never show a wish as a fact" discipline in
  `Devices.tsx`.

### Rebuild
- **Person page** — split into Person + Rules; delete `SecuritySlider`; collapse
  Block-account into Pause + Remove.
- **Vocabulary + casing** — apply §4 across web *and* `client/src` (tray, app,
  lockout, intro).
- **Time formatting** — one `duration` + one `ago`, plus the bar/ring.
- **Dead components** — delete the twelve in failure #6.
- **On-device client** — sentence-case pass; one "Ask for more time"; console↔device word-lockstep.

### Phase order
- **Phase 0 — the coherence sweep (the smallest first slice; see below).**
- **Phase 1 — one rules model.** Delete the Protection slider; seed new people
  with bracket healthy-defaults; collapse Block-account into Pause + Remove.
- **Phase 2 — split the person page** into Person (glance + controls) and Rules.
- **Phase 3 — de-jargon Devices**, demote the 2FA gate, drop the `/family`
  alias, group/trim the Settings back room.
- **Phase 4 — on-device coherence pass** so a paused screen, a spent day, and an
  unlock code read identically on both sides.

### The smallest first slice that makes it feel like one product
**Phase 0: a vocabulary + casing + dead-code sweep — no behaviour change.**
1. Adopt the §4 vocabulary table: rename lock/freeze/lockdown → **pause**;
   parent-code/PIN/backup → **unlock code**; sentence-case the tray, `App.tsx`
   gates, and stray caps labels; wordmark always `OpenScreenTime`.
2. Collapse the eight time helpers into `duration` + `ago`; fix the tray's
   missing hour rollover.
3. Delete the twelve orphaned components and the `/family` alias.

This touches almost every file, changes no behaviour, ships in one pass, and is
the fastest route to a console that reads as **one product with one voice**
instead of "someone told a chatbot to do it." Everything after it is real
product work (one rules model, the page split) standing on a coherent base.
