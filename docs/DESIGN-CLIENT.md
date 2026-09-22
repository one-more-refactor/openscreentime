# OpenScreenTime — the on-device client

The design of the four surfaces that run on the managed computer: the app window
(`ost app`), the full-screen wind-down and hard-stop (`lockout.rs`), the
first-run intro (`intro.rs`), and the tray + notifications (`tray.rs`). It
extends [`DESIGN.md`](DESIGN.md) — the ring language — onto the Rust/egui
client. Where this and `DESIGN.md` disagree, `DESIGN.md` §6 wins; where this is
more specific (px, ms, tuples, per-surface layout), build to this.

> **One idea, carried to the last screen.** *Time is a ring you fill.* The same
> ring that shows a child how much of the day is left is the ring that fills to
> full and closes when it runs out. Enforcement isn't a different visual
> language bolted on — it is the natural end of the same gauge. The screen a
> child is locked out of is unmistakably the product that let them in.

---

## 0. Tokens as egui constants

Replace the approximate constants currently in `app.rs`, `lockout.rs` and
`intro.rs` (they drifted — `0xf5f5f4` paper, `0x1a1a1a` ink, gray lines) with
the real palette. One shared block, ideally lifted into a small `client/src/ui.rs`
(or `theme.rs`) module and `use`d by all four surfaces so they can never diverge
again.

```rust
// OpenScreenTime — the ring language, as egui tuples. Verbatim from DESIGN.md §2.
pub const BG:          (u8,u8,u8) = (0xf4,0xf2,0xee); // warm paper canvas
pub const SURFACE:     (u8,u8,u8) = (0xff,0xff,0xff); // cards, code box, inputs
pub const SUNKEN:      (u8,u8,u8) = (0xec,0xeb,0xe6); // ring/bar track, sunken fills
pub const LINE:        (u8,u8,u8) = (0xe6,0xe3,0xdd); // hairline
pub const LINE_2:      (u8,u8,u8) = (0xd5,0xd1,0xc9); // input / secondary-button edge
pub const INK:         (u8,u8,u8) = (0x1e,0x1c,0x19); // primary text
pub const INK_2:       (u8,u8,u8) = (0x57,0x54,0x4e); // secondary text, detail line
pub const INK_3:       (u8,u8,u8) = (0x72,0x6e,0x66); // meta, captions, paused ring
pub const BRAND:       (u8,u8,u8) = (0x2e,0x7d,0x46); // THE RING, primary action, ok
pub const BRAND_STRONG:(u8,u8,u8) = (0x26,0x6a,0x3b); // button hover / pressed
pub const BRAND_TINT:  (u8,u8,u8) = (0xe4,0xf1,0xe8); // success wash, "sent" banner
pub const BRAND_INK:   (u8,u8,u8) = (0x1c,0x5c,0x33); // green text on tint/white
pub const WARN:        (u8,u8,u8) = (0x8a,0x63,0x00); // wind-down, offline (soft)
pub const WARN_TINT:   (u8,u8,u8) = (0xf7,0xed,0xd6); // wind-down banner bg
pub const STOP:        (u8,u8,u8) = (0xb3,0x15,0x1c); // the interrupt: locked/over/wrong
pub const STOP_TINT:   (u8,u8,u8) = (0xf8,0xe3,0xe2); // interrupt banner bg
pub const FOCUS:       (u8,u8,u8) = (0x2e,0x7d,0x46); // = BRAND (ink ring on green fills)
```

Set `egui::Visuals::light()` (already done) and additionally push the panel fill,
widget rounding and selection color so egui's own chrome (text-edit borders,
button focus) matches:

```rust
let mut v = egui::Visuals::light();
v.panel_fill = col(BG);
v.window_fill = col(SURFACE);
v.override_text_color = Some(col(INK));
v.selection.bg_fill = col(BRAND_TINT);
v.selection.stroke  = egui::Stroke::new(1.0, col(BRAND));
v.widgets.inactive.rounding = egui::Rounding::same(10.0); // --r-sm
v.widgets.hovered.rounding  = egui::Rounding::same(10.0);
v.widgets.active.rounding   = egui::Rounding::same(10.0);
```

### Radius / stroke / spacing → egui

| Language token | egui |
|---|---|
| `--r-sm` 10px | `Rounding::same(10.0)` — inputs, code box, footer card |
| `--r` 16px | `Rounding::same(16.0)` — cards, the primary button on the window |
| `--r-lg` 24px | `Rounding::same(24.0)` — nothing needs it on-device except a sheet |
| pill | `Rounding::same(h/2.0)` — action buttons, connection chip, status tag |
| ring stroke | `round(diameter * 0.09)` on the **hero** rings (matches the 16/180 in the web reference), `round(d*0.07)` on the small marque; round caps always |
| spacing | the 4px scale: `4·8·12·16·20·24·32·40·48·64`. Card padding 20; screen gutters 24–32 |

### Type (Figtree, bundled) → egui `FontId`

Bundle Figtree **and** Space Mono as `FontData::from_static` on `.ttf`s in the
binary, registered before the first frame, so the lock matches the console with
no network and no fontconfig (the overlay runs as root with a scrubbed `$HOME` —
it *cannot* rely on system fonts). Register a `"figtree"` family and a
`"mono"` family; fall back to `egui`'s built-in proportional/monospace if the
bytes fail to load.

| Role | Size / weight | egui | Use |
|---|---|---|---|
| Ring number | 64 / 800 | `FontId::new(64.0, figtree)` + heaviest weight face | minutes left / countdown, inside the ring |
| Headline | 34 / 700 | `36.0` | the stop line, the intro title |
| Message | 22 / 700 | `22.0` | secondary headline, challenge prompt |
| Sub / detail | 18 / 400 | `18.0` | the reason detail under a headline |
| Body | 16 / 400 | `16.0` | app-window detail, intro body |
| Label | 13 / 600 | `13.0` | field labels ("A parent's code") |
| Meta | 12.5 / 500 | `12.5` | connection chip, footer, "used today" |
| Code | 22 / 700 | `FontId::new(22.0, mono)`, tracking via spaced groups | the unlock code field only |

Weights: Figtree is variable but egui loads a static instance per `FontData`.
Register **three** Figtree faces — Regular (400), SemiBold (600), ExtraBold (800)
— as three families (`fig`, `fig_semi`, `fig_black`) and choose per role. Numbers
use tabular figures (Figtree's default figures are tabular-friendly; the count-up
won't jitter).

Rules that carry: **sentence case everywhere** (this kills the current
`SOLVE 7×8=?`, `TIME LEFT: NN MIN`, `WAIT 60s TO CONTINUE`, `SAVE YOUR WORK`
voice — see §7). Tracking `-0.03em` only on the 64px number; `0` elsewhere.

---

## 1. The ring — one painter, every state

Everything below draws through **one** function. It is the single most important
change to the client: today `ring()` paints a fixed ~100° green marque and
nothing draws the *real* proportional ring. Give the client a real gauge.

```rust
pub enum RingState {
    /// Fills clockwise from 12 o'clock by `frac` (used/limit, 0.0..=1.0) in BRAND;
    /// caller passes WARN at ≤15 min, STOP at/over the limit.
    Fill { frac: f32, color: (u8,u8,u8) },
    /// Parent-paused: a dashed INK_3 ring (dash ≈ 2% on, 5% off), disc dimmed.
    Paused,
    /// No daily limit: track only, no arc (a plain disc / a check goes inside).
    None,
    /// Full ring in one color — the hard stops (STOP red, or INK_2 for bedtime).
    Full { color: (u8,u8,u8) },
}
```

Geometry (identical to the web favicon and every avatar): stroke `round(d*0.09)`
on hero sizes, round caps, arc starts at `-π/2` and sweeps clockwise. egui has no
arc primitive, so:

1. Paint the **track**: `painter.circle_stroke(c, r, Stroke::new(sw, col(SUNKEN)))`.
2. Sample the **arc** as ~64 points over the sweep and `Shape::line(pts, Stroke::new(sw, color))`.
3. **Round caps** (egui lines are butt-capped): `painter.circle_filled(start_pt, sw/2, color)` and the same at `end_pt`. This is the whole trick — two dots make the caps.
4. For `Paused`, sample the full circle but only emit segments for the dash pattern; use `INK_3`.
5. Center content (number, glyph) is drawn by the caller inside the disc inset (`sw + 3px`).

**Draw-in motion.** On a surface's first appearance, animate `frac` from 0 to
its value over **640ms** with the one ease `cubic-bezier(.2,.7,.2,1)` (approx in
egui with `egui::emath::easing` or a hand-rolled ease-out), and count the number
up to meet it. `ctx.animate_value_with_time` is enough for `frac`; drive the
number from the same animated value. Under `prefers-reduced-motion` — read once
from the environment, or expose a build flag — skip the tween and draw the final
value. (egui has no media query; treat "reduced motion" as: if a
`OST_REDUCED_MOTION` env var or config flag is set, animations are instant.)

**Ring behaviour across every state (the through-line):**

| State | Ring | Center |
|---|---|---|
| Healthy (>15 min) | `Fill` green, `frac = used/limit` | number (min left) + "minutes left" |
| Wind-down warn (≤15 min) | `Fill` **amber**, frac near full | number + "minutes left" |
| Save-your-work grace | `Full` **amber**, number counts the grace **down** | seconds + "pausing soon" |
| Over / time's up | `Full` **stop red** | padlock glyph |
| Bedtime | `Full` **ink-2** (a calm "night", not red) | crescent moon |
| Parent-paused | `Paused` (dashed ink-3) | pause bars ‖ |
| Offline lockdown | `Full` **stop red** | cloud-with-slash |
| Tamper | `Full` **stop red** | shield / padlock |
| No limit today | `None` (track only) | a green check |
| Agent not running | track only, **no fill, no center glyph**, disc dimmed | — |
| Wrong code (flash) | current ring flashes to STOP for 240ms, then back | unchanged |

The stop is red, but the **reason** is told by the center glyph and the words —
so bedtime and a parent pause read calm (moon, pause bars) while the three
"you hit a wall" stops (time's up, offline, tamper) share the red padlock family.
A calm night shows no alarm; only the genuine walls are red.

---

## 2. App window — `ost app` (`app.rs`)

420×520, min 360×440. The window's whole job: say, in plain words, how much time
is left, whether it's connected, and the honest limits of what the software can
see — with one thing it can *do* (ask for more). Redesign from the current
left-aligned column to a **centered gauge**: the ring is the hero.

```
┌──────────────────────────────────┐  BG paper, 28px inner margin
│  ◔  OpenScreenTime                │  marque ring 18px + name 15/700 INK, top-left
│                                    │
│              ╭─────────╮           │  hero ring, 168px, green Fill
│            ╱     45      ╲         │  number 64/800 INK (WARN ≤15, STOP at 0)
│           │   minutes     │        │  label 12.5/500 INK_2, inside
│            ╲    left      ╱        │
│              ╰─────────╯           │
│                                    │
│           ● Connected              │  chip: 8px dot + 14/500, centered, BRAND
│                                    │
│   ╭────────────────────────────╮   │  primary button, full-ish width, r=16
│   │     Ask for more time       │   │  GREEN fill, white 16/700, min-h 44
│   ╰────────────────────────────╯   │
│                                    │
│  ────────────────────────────────  │  LINE hairline
│  OpenScreenTime counts screen      │  footer, 12.5/500 INK_3, in a SUNKEN
│  time and filters the network.     │  r=10 card OR below the hairline.
│  It can't see your screen, your    │  The honest promise, always in view.
│  messages, what you type, or your  │
│  browsing history.                 │
└──────────────────────────────────┘
```

**Layout in egui.** `CentralPanel`, `Frame::inner_margin(28.0)`. Top row: the
18px marque ring + name (left-aligned — the wordmark stays put). Then
`ui.vertical_centered`: `add_space` to push the ring toward optical center
(≈ 12% of available height), the 168px ring via §1 with the number+label drawn in
the disc, `add_space(20)`, the connection chip, `add_space(24)`, the button
sized `[available_width().min(300.0), 44.0]`, then the footer.

**The number and its color** come from `time_headline()`, which already returns
`(String, color)` — keep the logic, feed the ring: green when `m > 15`, WARN when
`m <= 15`, STOP when `m <= 0` (and swap the number for a padlock glyph). `frac`
is `used_minutes / (used_minutes + remaining_minutes)`. `None` remaining → ring
`None` + a check + "No limit today". `frozen` → `Paused` ring, disc dimmed,
number replaced by pause bars, label "Paused".

**Connection chip** (`connection()` already returns text+color): a filled dot +
label, centered. `Connected` → BRAND; soft offline → WARN, "Offline — catching
up when it's back"; hard offline → STOP, "Offline — locked"; agent gone → INK_3,
"Not running".

**The button becomes green.** This is the §DESIGN change: the current
`Button::new(...).fill(col(FG))` (ink slab) becomes `fill(col(BRAND))`, white
text, `Rounding::same(16.0)`, hover `BRAND_STRONG`. When asked, it disables and
reads "Asked — waiting for a parent", and a BRAND_TINT confirmation line "Sent —
a parent can say yes." sits under it. Focus: egui draws its own; set the widget
focus stroke to BRAND (ink stroke would need the on-green rule but the button
loses focus outline to its own fill — a 2px BRAND ring at 2px offset is fine
here since the button is green-on-paper, not green-on-green).

**Device-level banner.** When `device_banner()` fires (parent-paused / offline
lockdown / tamper), it takes the top of the content as a STOP_TINT card
(r=10, 14px pad) with STOP text and a small glyph — the window mirrors what the
full lock says, in miniature, so an open window is never out of sync with the
locked session.

**Degraded — agent not running (`status == None`).** Not an error, a calm
waiting state: ring drawn as **track only** (no fill), disc dimmed, number
replaced by an ellipsis or nothing, headline "OpenScreenTime isn't running yet",
detail "It'll pick up in a moment.", the button disabled. No red, no stack trace.

---

## 3. Wind-down — the save-your-work countdown (`lockout.rs`, `deadline`)

The amber bridge before a stop. It is a full-screen surface (the grace runs while
the session is still usable, but the overlay is up so the warning can't be
missed). This is the surface's `countdown_secs` / `deadline` path.

```
┌──────────────────────────────────────────────┐  BG paper, fullscreen
│                                                │
│                  ◔ OpenScreenTime              │  marque + name, DIM, centered
│                                                │
│                 ╭───────────╮                  │  ring 220px, FULL amber
│               ╱      30       ╲                │  number 64/800 WARN, counts DOWN
│              │    seconds       │               │  label 16/400 INK_2
│               ╲     left        ╱               │
│                 ╰───────────╯                  │
│                                                │
│              Wrapping up                        │  headline 34/700 INK
│      Save your work — the screen pauses soon.  │  detail 18/400 INK_2
│                                                │
└──────────────────────────────────────────────┘
```

**Ring:** `Full { color: WARN }`, the number counting the grace seconds down
(the existing `deadline` math). The ring itself does **not** deplete — a full
amber ring reads "the day is spent, here's your grace"; the number carries the
countdown so the motion is legible and calm. Repaint at 500ms (already coded).

**The one motion: amber → red at zero.** As the countdown crosses into the last
~3 seconds, tween the ring color and number from WARN `(0x8a,0x63,0x00)` to STOP
`(0xb3,0x15,0x1c)` over the final second (lerp per frame). At zero the surface is
replaced by the matching hard-stop (§4) — same ring size, now red and full, so
it reads as one continuous object hardening, not two screens. No shake, no
flash, no siren.

For teens the detail already carries "The screen stops in N — save your work.";
keep it. For little/kid, the shorter detail. The wind-down **notification**
(tray, §6) precedes this overlay at the 10- and 2-minute marks.

---

## 4. Hard-stop lock (`lockout.rs` `LockApp`)

**Art direction, one sentence:** the day's ring, drawn full and closed, centered
on warm paper, with the reason said plainly in a real sans and one calm way back
in — the lock is the gauge completed, not an alarm raised.

Shared skeleton for every reason (fullscreen, `BG`, content optically centered):

```
┌──────────────────────────────────────────────┐  BG paper
│                                                │
│                  ◔ OpenScreenTime              │  marque + name, DIM 15/700
│                                                │
│                 ╭───────────╮                  │  ring 220px, state per reason
│                │    [glyph]   │                 │  center glyph per reason
│                 ╰───────────╯                  │
│                                                │
│                    Stop                         │  headline 34/700 INK (verbatim)
│     Time's up for today — 60 of 60 minutes used.│ detail 18/400 INK_2 (verbatim)
│                                                │
│   · · · · · · ·  challenge / code  · · · · · · ·│  §5, only if a way back exists
│                                                │
│           ╭──────────────────╮                 │  the one button, §5
│           │      Continue      │                │
│           ╰──────────────────╯                 │
└──────────────────────────────────────────────┘
```

The headline and detail are passed in the `LockSpec` **verbatim from the
runner** (`LockReason::headline/detail`, `lock_copy`, the device-lock strings),
so the overlay, the tray and the docs never drift. Per reason — ring state
(§1), center glyph, and the words the code already produces:

| Reason (source) | Headline · detail (verbatim) | Ring | Glyph |
|---|---|---|---|
| **Time's up** — `LockReason::DailyLimit` | "Stop" · "Time's up for today — {u} of {l} minutes used." | `Full` **STOP** | padlock |
| **Bedtime** — `LockReason::Bedtime` | "Goodnight" · "Screens are off until morning." | `Full` **INK_2** (night) | crescent moon |
| **Outside window** — `LockReason::OutsideWindow` | "Not now" · "Screens are off at this time of day." | `Full` **INK_2** | clock/moon |
| **Parent-paused** — `device_locked` | "Paused" · "A parent paused this computer." (+ grace variant) | `Paused` (dashed INK_3) | pause bars ‖ |
| **Offline lockdown** — `offline_hard_lockdown` | "Stopped" · "No contact with the family server for days. Ask a parent — their code unlocks." | `Full` **STOP** | cloud-with-slash |
| **Tamper** — `tamper_lockdown` | "Stopped" · "OpenScreenTime was tampered with. Ask a parent — their code unlocks." | `Full` **STOP** | shield / padlock |

Bedtime and outside-window are **expected** pauses, so they use a calm INK_2
ring with a moon — no red. Parent-paused reuses the language's own *paused* ring
(dashed ink-3), which already means "no time is accruing" — the fullscreen lock
is that same signal at hero scale. The three genuine walls share the red
padlock family. The center glyph is drawn with the painter (padlock: a rounded
rect body + a stroked shackle arc; moon: two offset filled circles; pause: two
rounded rects; cloud-slash: three overlapping circles + a stroke line), all in
the ring's color at ~44px.

**Motion.** The ring draws in (640ms) on appearance — for a stop it draws
straight to full, so it reads as the gauge closing. Nothing pulses. Nothing
loops. If arriving from the wind-down (§3), the ring is already on screen and
only its color hardens amber→red; don't re-draw it.

---

## 5. The challenge / parent-code entry (`lockout.rs` `challenge`)

Below the reason, only when there is a way back. Four challenge shapes plus the
always-available parent code. All sentence case, calm, firm.

**Parent's unlock code (always offered when configured).** The escape hatch a
present parent can always use.

```
  A parent's code                          ← label 13/600 INK_2, sentence case
  ┌────────────────────────────┐
  │  • • •   • • •              │          ← masked, Space Mono 22/700, groups of 3
  └────────────────────────────┘
  From the console, or a recovery code.    ← hint 12.5/500 INK_3
```

- Field: `SURFACE` bg, `LINE_2` border, `Rounding::same(10.0)`, min-h 44,
  `password(true)`, mono font. On focus, border → BRAND (egui: set
  `visuals.selection` / widget stroke) — no glow needed on-device.
- The maths answer (Math challenge) is a **separate, visible** field above it:
  prompt "What's 7 × 8?" (sentence case — not `SOLVE 7×8=?`), field shows the
  typed digits, because a child working it out must see their answer.
- `Wait` challenge: no field. A line "You can continue in 42s" counting down; the
  button is disabled until 0 (tween its opacity from .45 to 1 at zero).
- `None` (nudge only) and the whole-device locks with no self-serve: no
  challenge field — only the parent-code box (if any) and the button.

**The button.** One button, green primary (BRAND fill, white 16/700,
`Rounding::same(16.0)`, min-h 46, hover BRAND_STRONG). Its label matches the act:
"Continue" when it acknowledges/verifies a solved challenge; the verify logic is
unchanged (`Challenge::verify`, `write_unlock_grant`). Keep the grant sizes
(parent 30 min, challenge 5 min).

**Wrong code — one calm flash, no shake.** On a rejected code
(`pin_msg` set), do exactly two things:
1. Flash the ring to `STOP` for **240ms** then ease back to its resting color
   (`animate_value` from 1→0 driving a STOP↔resting lerp). The whole ring, once.
2. Show the message in STOP, 13/600, sentence case: "That code didn't work. Try
   again." / for a rate-limit, the verdict's own message ("Locked — try again in
   60s."). Clear the field.

No screen shake, no red full-bleed, no sound. The enforcement is firm; the tone
is kind. This is the emotional core of the brief — the same warmth that greeted
the child is what turns them away.

**Mapping:** this is the `if matches!(... Math ...)` / `if configured()` block in
`LockApp::update`. Replace the mono-caps prompt with the sentence-case string,
the ink button fill with BRAND, and add the 240ms ring-flash on the
`self.pin_msg = ...` branch (store a `flash_started: Option<Instant>` on
`LockApp`, drive the lerp, request_repaint while it runs).

---

## 6. First-run intro (`intro.rs`)

560×380. The child's documentation as a few honest cards. The ring becomes the
**progress indicator**: a small ring, top, that fills one segment per card — you
literally fill the ring as you learn how the ring works.

```
┌──────────────────────────────────────────────┐  BG paper
│  ◔ (fills 3/6)                          Skip   │  progress ring 24px + Skip (quiet)
│                                                │
│  What a parent can see                          │  title 34/800 INK, left, 48px in
│                                                │
│  How much screen time you've used, on which     │  body 18/400 INK_2, measure ≤ 46ch
│  device, and if someone tampers with            │
│  OpenScreenTime. That's it.                     │
│                                                │
│                              ╭──────────────╮   │  Next / Done, green primary,
│                              │     Next       │   │  bottom-right, r=16
│                              ╰──────────────╯   │
└──────────────────────────────────────────────┘
```

- **Progress** is the ring at `frac = (slide+1)/SLIDES.len()`, BRAND fill, drawn
  by the same painter at 24px — not the current mono `3/6`. It draws-in one
  segment on each `Next` (a 240ms tween of `frac`).
- **Skip** — quiet button, top-right, INK_2, hover SUNKEN.
- **Title** 34/800 INK (Figtree ExtraBold), **body** 18/400 INK_2, left-aligned
  with the 48px top space kept.
- **Next / Done** — green primary pill, bottom-right (not the current ink slab).
- Copy is unchanged (`SLIDES`) — it's already the right voice.
- Keep it one-time-and-skippable; the marker logic stays.

---

## 7. Tray + notifications (`tray.rs`)

The tray can't paint (it's freedesktop `ksni` + `notify-rust`), so the design
work here is **voice and iconography** — and it's where the old surveillance tone
still shouts loudest. Every string is currently ALL-CAPS mono-console
(`TIME LEFT: NN MIN`, `SAVE YOUR WORK`, `AGENT NOT RUNNING`, `REQUEST MORE TIME`,
`DEVICE MANAGED`). That is exactly the voice `DESIGN.md` retires. Rewrite to
sentence case — calm, kind, firm:

| Now (mono-caps) | Rewrite (sentence case) |
|---|---|
| `TIME LEFT: 45 MIN` | `45 minutes left today` |
| `NO LIMIT` | `No limit today` |
| `PAUSED` | `Paused by a parent` |
| `DEVICE MANAGED` | `This device is managed` |
| `CONNECTION: ONLINE` | `Connected` |
| `OFFLINE — LOCKED` | `Offline — locked` |
| `AGENT NOT RUNNING` | `OpenScreenTime isn't running` |
| `REQUEST MORE TIME` | `Ask for more time` |
| `NN MIN LEFT TODAY` / `SAVE YOUR WORK` | `{n} minutes left` / `Time to wrap up and save your work.` |
| `SCREEN PAUSES IN NS` | `Screen pauses in {n}s — save your work.` |
| `Time's up` / `EARN MORE OR ASK A PARENT` | `Time's up for today` / `Ask a parent, or earn more.` |
| `OFFLINE TOO LONG` | `Offline too long` / `Restricted until it reconnects.` |
| `TAMPERING DETECTED` / `... TAMPERED WITH ...` | `Something changed` / `OpenScreenTime was changed — ask a parent; their code opens it.` |
| `ABOUT OPENSCREENTIME` | `About OpenScreenTime` |
| `N TIME REQUEST(S)` / `APPROVE +N MIN` / `DENY` | `{n} time requests` / `Approve {n} more minutes` / `Not now` |

**Urgency mapping** (already mostly right): the 2-minute warning, the freeze
countdown, time's-up, offline-lockdown, parent-pause and tamper are `Critical`;
"back online / you're back / resumed / lifted" are normal. Keep transitions-only
(no re-engagement nagging).

**Tray icon = the ring.** GNOME has no SNI host, but where a tray exists the icon
should be the mark. Ship three bundled ring icons (green ~40% arc, amber full,
red full padlock) and use them by state; fall back to the current freedesktop
names (`security-high/medium/low`) only if the bundled icons can't be themed.
The tooltip: `"{time line} · {connection}"` in the new sentence-case strings.

**Notification body** for the sign-in prompt is already sentence case and good —
leave it; just ensure the summary "Sign-in request" and action "Not me" stay.

---

## 8. Degraded, empty, headless — authored, never blank

- **Agent not running** (status file missing): app window §2 calm waiting state;
  tray shows "OpenScreenTime isn't running". Never an error dialog.
- **No graphical session** (`session_env` returns `None`): the GUI overlay can't
  draw; the code already falls back to the TTY `wall` broadcast. Design that text
  too — sentence case, the same headline+detail: `OpenScreenTime — Goodnight.
  Screens are off until morning.` (drop the `▍`, the `· ·` dot rows and the box
  art from `render_ascii`; a plain, indented three-line message reads as the same
  calm product, not a console).
- **Offline (soft)**: connection chip / tray in WARN, "catching up when it's
  back" — amber, not red; the day keeps working from cached policy.
- **Offline (hard lockdown)**: the §4 red stop. The distinction — amber "we'll
  catch up" vs red "we've lost contact too long" — is the whole point of the two
  colors.
- **No limit configured**: ring `None` + green check + "No limit today". The
  product is present and honest even when it's enforcing nothing.

---

## 9. What I deliberately left out

- **No second accent, no gradient, no illustration.** The ring and the type
  carry it; adding art would read as the template look the rebrand escaped.
- **No progress spinner, no shimmer, no looping pulse** anywhere — the ring's
  one draw-in is the only motion, plus the wind-down amber→red and the single
  wrong-code flash. Motion is seasoning; nothing here can leave content blank if
  a tween doesn't fire.
- **No shake, no sound, no full-bleed red** on a wrong code or a stop — firmness
  comes from the plain words and the closed ring, not from alarm.
- **No dark mode on-device** — the client is always light (`DESIGN.md` §2); dark
  is web-console only.
- **No decorative dot-grid, corner ticks, dot-matrix or box-art** — the retired
  surveillance motifs stay retired, including in the headless `render_ascii`.
</content>
</invoke>
