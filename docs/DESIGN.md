# OpenScreenTime — Design language

**One idea: time is a ring you fill.** A single green activity ring is the
product's whole identity — it is the favicon, the wordmark's marque, every
family avatar, the child's own page, and the shape the lock screen draws. Around
that one idea we build a warm, plain-spoken family dashboard: friendly like
Google Family Link, but with a point of view Family Link doesn't have.

The visual source of truth is [`web/design/reference.html`](../web/design/reference.html) —
open it in a browser (it renders every token and component in light **and** dark).
This document is the written spec. When the two disagree, the reference is right.

> This replaces the old "Nothing" austere-dark system. That system was rigorous
> but read as *surveillance* — OLED black, monospace ALL-CAPS labels, corner
> registration ticks, dot-grid, ink-on-ink buttons. Warmth was bolted on top
> (a green ring, emoji faces) without changing the bones, which is exactly why
> the client called it inconsistent. We change the bones.

---

## 1. Principles

1. **The ring is the product.** One mark, one meaning, everywhere. Green fills as
   the day is used; it is calm, not alarming, because a full ring is *normal*.
   Never invent a second chart language for "time used" — if it's about time,
   it's the ring or a bar that reads like the ring.

2. **Colour is reassurance, not decoration.** The palette is tiny and every hue
   has one job: **green = healthy / within budget / the action to take**,
   **amber = attention or a transition** (offline, wind-down, a waiting request),
   **red = the one interrupt** (locked, over the limit, destructive). Everything
   else is ink on warm neutral. A calm day shows no red at all.

3. **Read it like a person, not a console.** Sentence case. A humanist sans.
   No monospace ALL-CAPS labels, no timestamps dressed as data. Numbers mean
   something (minutes left, last seen) and are set in the same friendly type,
   with tabular figures — not a "technical" typeface.

4. **Warm, quiet, and finished.** Soft shadows over hard hairlines, generous
   radius, real whitespace, one gentle motion grammar. Every empty and loading
   state is authored so the product never looks broken or blank.

5. **The same language reaches the locked screen.** What a child is locked out
   *of* is unmistakably the same product that let them in — same ring, same
   colours, same warm off-white. Enforcement is calm, never punitive.

---

## 2. Colour

Roles, not shades. Every value is a CSS custom property; light is the default
and the on-device client is always light. Contrast ratios are against the
surface the colour normally sits on; all body/label text passes WCAG AA.

### Light (default)

| Token | Hex | Role | Contrast |
|---|---|---|---|
| `--bg` | `#f4f2ee` | warm paper canvas | — |
| `--surface` | `#ffffff` | cards, inputs, sheets | — |
| `--surface-2` | `#ecebe6` | sunken: ring tracks, bar tracks, skeletons | — |
| `--rail` | `#efede8` | nav plane (recedes under white cards) | — |
| `--line` | `#e6e3dd` | hairlines, card borders | — |
| `--line-2` | `#d5d1c9` | input borders, secondary-button edge | — |
| `--ink` | `#1e1c19` | primary text | 13.9:1 |
| `--ink-2` | `#57544e` | secondary text, labels | 7.0:1 |
| `--ink-3` | `#726e66` | meta / captions (use ≥ 12.5px) | 5.2:1 |
| `--brand` | `#2e7d46` | **the ring**, primary action, active nav | 4.8:1 |
| `--brand-strong` | `#266a3b` | primary hover / pressed | 6.0:1 |
| `--brand-tint` | `#e4f1e8` | selected fill, success wash, active nav bg | — |
| `--brand-ink` | `#1c5c33` | green text on tint or white | 5.6:1 |
| `--warn` | `#8a6300` | attention text (offline, wind-down) | 4.9:1 |
| `--warn-tint` | `#f7edd6` | attention banner bg | — |
| `--stop` | `#b3151c` | the interrupt: locked, over, destructive | 5.9:1 |
| `--stop-tint` | `#f8e3e2` | interrupt banner bg | — |
| `--focus` | `#2e7d46` | focus ring (ink `#1e1c19` when on a green fill) | — |

### Dark (web console only)

Warm near-black, never OLED. Green and status colours brighten for contrast.

| Token | Hex | | Token | Hex |
|---|---|---|---|---|
| `--bg` | `#171614` | | `--brand` | `#46b06a` |
| `--surface` | `#201e1b` | | `--brand-strong` | `#5cc07e` |
| `--surface-2` | `#2a2723` | | `--brand-tint` | `#1e3a28` |
| `--rail` | `#131211` | | `--brand-ink` | `#8fe0a6` |
| `--line` | `#302c27` | | `--warn` | `#d8ab4a` |
| `--line-2` | `#423d37` | | `--warn-tint` | `#38300f` |
| `--ink` | `#f3f0ea` | | `--stop` | `#e5595c` |
| `--ink-2` | `#b4aea4` | | `--stop-tint` | `#3a1d1e` |
| `--ink-3` | `#8b857b` | | `--focus` | `#46b06a` |

### The avatar set — curated, never random

The current build derives an avatar disc colour from `hsl(hueFor(seed) …)`, which
produces uncontrolled hues. Replace it with a fixed set of eight warm pairs,
chosen deterministically by `hash(id) % 8`, so a family of faces looks like one
family:

```
0 blush   bg #fbe3dd  ink #9a3b28      4 mint   bg #d6efe0  ink #1f6b45
1 peach   bg #fce6cf  ink #8a5a12      5 sky    bg #d9e9f5  ink #2b5878
2 butter  bg #f7efc9  ink #6f5a10      6 iris   bg #e2e2f7  ink #3f3f86
3 sage    bg #dfeecf  ink #3f6a2a      7 lilac  bg #efe0f2  ink #6d3577
```

The centre holds identity (parent-picked emoji, else a monogram in the pair's
ink). The **ring around it holds state** — it is never coloured by the avatar set.

---

## 3. Typography

**One typeface: Figtree** (Google Fonts, weights 400–900, variable). It is
warm and rounded enough to feel friendly, geometric enough to stay calm and
legible for a tired parent, and has excellent tabular figures for all the time
numbers. It is distinctive without being a novelty, and it carries the whole
product — parent console, child page, and lock screen — so nothing needs a
second font. Full Latin-Extended coverage (umlauts: Vali, Mika, Müller).

**Space Mono** survives in exactly one place: literal secret codes (the unlock
code, recovery codes), where fixed-width, unambiguous glyphs aid reading. It is
never a label, an eyebrow, a timestamp, or a heading again.

> Retired: Space Grotesk, the Space Mono ALL-CAPS label voice, Doto, Nunito.
> The playful child look is now expressed through Figtree's heavy weights
> (800/900), scale, and colour — not a separate typeface.

### Scale (base 16px / 1rem)

| Name | Size | Weight | Line | Use |
|---|---|---|---|---|
| Display | 48–64px | 800 | 1.0 | the big minutes number (child page, hero) |
| H1 | 28px | 700 | 1.15 | page title |
| H2 | 18px | 600 | 1.3 | section heading |
| H3 | 16px | 600 | 1.35 | card title, name |
| Body | 16px | 400 | 1.5 | anything a person reads |
| Small | 14px | 400 | 1.45 | secondary copy |
| Label | 13px | 600 | 1.3 | field labels (sentence case) |
| Meta | 12.5px | 500 | 1.4 | captions, "last seen", timestamps |
| Code | 18–28px | 700 | 1 | unlock / recovery codes only (Space Mono) |

Rules: sentence case everywhere. Display/number styles get
`font-variant-numeric: tabular-nums`. Tracking: `-0.03em` on Display, `-0.02em`
on H1, `0` elsewhere. Measure for body copy ≤ 60ch.

---

## 4. Space, radius, elevation, motion

**Spacing** — 4px base scale: `4 · 8 · 12 · 16 · 20 · 24 · 32 · 40 · 48 · 64`.
Card padding 20px; page gutters 20–24px; section gap 32–48px.

**Radius** — a real 3-step scale plus the pill, replacing today's 6/12/22/28 mix:

| Token | Value | Use |
|---|---|---|
| `--r-sm` | 10px | inputs, chips, tiles, code box |
| `--r` | 16px | cards |
| `--r-lg` | 24px | modals, hero/kid cards |
| pill | 999px | buttons, nav items, toggle pills, status tags |

**Elevation** — two warm, soft levels; no hard "hardware" shadows, no glow.

```
--shadow-1: 0 1px 2px rgba(30,26,18,.05), 0 1px 3px rgba(30,26,18,.06);
--shadow-2: 0 4px 12px rgba(30,26,18,.07), 0 14px 30px -10px rgba(30,26,18,.14);
```

Cards rest at `--shadow-1` + `--line`; on hover (if interactive) they lift to
`--shadow-2` and `translateY(-2px)`. Modals use `--shadow-2`. Dark mode swaps in
black-based shadows (see reference).

**Motion** — one gentle ease, three durations. Calm, never bouncy.

```
--ease: cubic-bezier(.2,.7,.2,1);
--dur-1: 140ms;   /* hover, press */
--dur-2: 240ms;   /* enter, state change */
--dur-3: 640ms;   /* data: the ring drawing, bars filling */
```

The ring and week-bars draw on arrival (`stroke-dashoffset` / height, 640–900ms);
the number counts up to meet the ring. Press = `scale(.98)`. Page arrival = one
120ms fade of the whole view (no per-card cascade). Everything inside
`@media (prefers-reduced-motion: reduce)` is disabled. Retired: shake, saturate
"freeze-sweep" flicker, shimmer skeletons.

---

## 5. Components

### Button — one grammar, four roles, three sizes

Pill, Figtree 600. Focus ring `--focus` 2px + 2px offset (ink ring on green
fills). Press `scale(.98)`. Disabled `opacity .45`.

| Variant | Rest | Hover | Use |
|---|---|---|---|
| **primary** | `--brand` fill, white text, `--shadow-1` | `--brand-strong` | the one main action per view |
| **secondary** | `--surface`, `--line-2` border, ink | `--surface-2` | supporting actions ("Not now", "Manage") |
| **quiet** | transparent, `--ink-2` | `--surface-2`, ink | Cancel, low-stakes |
| **danger** | transparent, `--stop` text + faint border | `--stop-tint` bg, `--stop` border | destructive, at rest; **solid** (`--stop` fill, white) only for the final confirm |

Sizes: `sm` 36px / 14px · `md` 44px / 15px · `lg` 52px / 17px (kid). One
`<Button>` component; the segmented control, filter pills and bracket picker are
their own primitives but share these tokens. **This is the biggest single change
from today, where "primary" is a hairline outline and the real filled button is
ink-black — the primary action becomes green.**

### Text input & select

`--surface` bg, 1px `--line-2`, `--r-sm`, min-height 44px, Figtree **16px**
(never mono). Label above: 13px/600 `--ink-2`, sentence case. Focus: border
`--brand` + `0 0 0 3px` brand glow. Error: border `--stop`, hint text `--stop`.
Select adds a CSS chevron. Hint 12.5px `--ink-3`.

### Card

`--surface`, `--line`, `--r`, `--shadow-1`, padding 20px. No corner ticks, no
dot-grid. Interactive cards get the hover lift. Three card recipes:

- **Family card** — avatar-ring on the left; name (H3), a meta line
  (`ages · device`), a segmented budget bar, and one plain sentence of time
  ("**48m** left of 1h 30m"). Over the limit: bar and time go `--stop`, a
  `Time's up` status tag sits by the name.
- **Device card** — name (H3) + user (meta), a status pill (Online/Offline/
  Locked) top-right, a small facts list (last seen, today, protection), and a
  row of secondary/quiet buttons.
- **Plain card** — the generic container for settings rows and sheets.

### Avatar + activity ring — the mark

Identity disc (emoji or monogram, on an avatar-set pair) centred inside a ring.
Ring geometry is fixed everywhere:

- stroke = `round(diameter × 0.07)`, round line-caps, fill starts at 12 o'clock
  and grows clockwise.
- track `--line`; fill `--brand`; **over-limit fill `--stop`**; **paused = a
  dashed `--ink-3` ring** (`stroke-dasharray` ~2%/5%) and the disc dims to 0.6.
- no target (no goal, no limit) → a plain disc, no ring.
- disc inset = stroke + 3px; disc font-size = 50% of its own box (emoji) / 34%
  (monogram).

Sizes: 28px (rail), 56px (family grid), 72px (states), 140–180px (child/lock
hero). The favicon and wordmark marque use the same arc at a fixed ~40% fill.

### Chips, pills, toggles, status

- **Filter pill** (multi-select): rest `--surface` / `--line-2` / `--ink-2`;
  **selected `--brand-tint` bg + `--brand-ink` text + brand border** — not the
  old ink-on-ink black. 40px min height.
- **Removable chip**: `--surface`, `--line`, ink text, a 24px `✕` that turns
  `--stop` on hover.
- **Segmented control**: `--surface-2` track, selected segment lifts to
  `--surface` + `--shadow-1`.
- **Status tag**: tinted by tone — `ok` brand-tint/brand-ink, `warn`
  warn-tint/warn, `stop` stop-tint/stop, `neutral` surface-2/ink-2. Small,
  12.5px/600.

### Modal & confirm

`--surface`, `--r-lg`, `--shadow-2`. Scrim `rgba(30,26,18,.5)` (warm, lighter
than today's harsh `rgba(0,0,0,.72)`), no dot-grid. Title is an **H2 in sentence
case** (not a mono-caps 11px "terminal" title). Close `✕` is a 44px target. Traps
focus, restores on close, Escape to dismiss. Destructive confirm: danger-**solid**
final button, with type-the-name confirmation.

### App shell & navigation

- **Desktop rail** (`--rail` plane): wordmark, then nav as **pill rows** —
  active = `--brand-tint` bg + `--brand-ink` text + leading monoline icon (not a
  left ink border). Below, "Today at home": each person as a small avatar-ring
  row with minutes left. Footer: identity, sign-out, and the change-mode lock.
- **Mobile: a bottom tab bar** (Family · Devices · Settings · You) — conventional
  and thumb-reachable, replacing today's floating hamburger. Active tab uses the
  same brand-tint treatment.
- Icons are monoline (1.75–2px stroke, `currentColor`, round joins). No emoji as
  iconography — emoji are identity (faces) only.

### Empty / loading / error / success

- **Empty**: authored, warm, centred — the ring mark, one plain sentence, one
  primary button ("No one here yet. Add your first child to see their day.").
  Dashed `--line-2` border, no dot-grid.
- **Loading**: structural skeletons in `--surface-2` that breathe once
  (opacity .9↔.5, 1.8s). They show the *shape* of what's coming; never a
  spinner, never a shimmer sweep. A refresh over existing data is a 2px brand
  top-bar, not a blanked page.
- **Error**: an inline banner, `--stop-tint` bg / `--stop` text, a monoline icon,
  human copy and a **Retry** action. Never a bare stack trace.
- **Success**: an inline `--brand-tint` banner with a check ("Saved. Vali's new
  limit is live."). Brief, inline, no toast.

---

## 6. The on-device client (Rust / egui)

The agent's app, wind-down, lock and intro screens share this language via
hand-set constants. They are **light** (the black lock is retired). Update the
tuples in `client/src/app.rs`, `client/src/lockout.rs`, `client/src/intro.rs`:

```rust
// tokens → egui Color32::from_rgb tuples
const BG:     (u8,u8,u8) = (0xf4,0xf2,0xee); // warm paper
const SURFACE:(u8,u8,u8) = (0xff,0xff,0xff);
const SUNKEN: (u8,u8,u8) = (0xec,0xeb,0xe6); // ring / bar track
const LINE:   (u8,u8,u8) = (0xd5,0xd1,0xc9);
const INK:    (u8,u8,u8) = (0x1e,0x1c,0x19);
const INK2:   (u8,u8,u8) = (0x57,0x54,0x4e);
const INK3:   (u8,u8,u8) = (0x72,0x6e,0x66);
const BRAND:  (u8,u8,u8) = (0x2e,0x7d,0x46); // the ring / ok
const WARN:   (u8,u8,u8) = (0x8a,0x63,0x00); // wind-down
const STOP:   (u8,u8,u8) = (0xb3,0x15,0x1c); // time-up / wrong code
```

Rules that translate:

- **Corner radius** ~16px on cards/buttons, pill (`h/2`) on action buttons;
  round line-caps on the ring stroke. Set egui `Rounding` and `Stroke` to match.
- **Type**: bundle Figtree (`egui::FontData::from_static` on a `.ttf` in the
  binary) so the lock matches the console; fall back to the platform sans if the
  font can't load. Sizes follow the scale: the big number ~64px/800, one message
  line ~22px/700, sub ~16px/400.
- **The ring is drawn, not decorative.** Use egui's painter: a `SUNKEN` track
  circle + a `BRAND` arc from top, clockwise, proportional to time used. Over →
  `STOP` arc. This is the same mark as the web favicon.
- **Lock screen** (`lockout.rs`): warm `BG`, the ring large with the fact inside
  (minutes left, or a padlock glyph when stopped), then one sentence and one next
  step, drawn from the lock reason so copy matches the console verbatim —
  "Stop — time's up for today", "Goodnight — screens are off until morning",
  "Paused — a parent paused this computer. Save your work." A wind-down countdown
  precedes every stop, ring and text in `WARN`, sliding to `STOP` at zero.
- **Intro** (`intro.rs`): same `BG`/`INK`, the wordmark ring, plain sentences.
- Wrong-code feedback: the ring/segment flashes `STOP` once; no shake, no siren.
  Calm, not punitive — the enforcement is firm, the tone is kind.

---

## 7. What we deliberately dropped

Corner registration ticks, the dot-grid texture, the 5×7 dot-matrix and fleet
glyph strips, the OLED-black canvas, the mono ALL-CAPS metadata voice, random
HSL avatar colours, ink-on-ink "primary" buttons, and the harsh black modal
scrim. They belonged to the surveillance-console era. What we keep from it: the
discipline — a tiny palette, one accent that means *stop*, hierarchy from
type and space before boxes, and every diagram carrying real meaning.
