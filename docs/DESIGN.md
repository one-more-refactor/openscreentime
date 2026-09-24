# OpenScreenTime — Design language (web console)

**The house clock.** The mark is the activity ring read as a clock face: one
added detail, a tick at twelve o'clock — the start line of every day —
separates it from a progress spinner. Around it, a warm, plain-spoken family
console.

The visual source of truth is [`brand/board.html`](../brand/board.html) —
open it in a browser: the mark and its geometry, colour and type, the icon
set, the product's key screens, and the voice. This doc writes the rules down
for the web console; [`web/src/theme.css`](../web/src/theme.css) is the same
token set in code. The computer's own screens are
[`DESIGN-CLIENT.md`](DESIGN-CLIENT.md).

---

## 1. Principles

1. **The ring means one thing: time used today**, filling clockwise from the
   tick at twelve. The number beside it may say time left, because that is
   what a person asks. It never depletes, never spins, and never counts
   anything but a day — no loading rings, hold rings, countdown rings or
   code-entry rings.

2. **Colour is hierarchy.** Most of any screen is paper, white and ink.
   **Green** is time and the one main action. **Amber** is a transition: 15
   minutes or less, offline, a waiting request. **Red** is stop — time's up, or
   a destructive confirm — and a healthy day has none. A parent's pause is
   neutral (a dashed ring, a plain tag), never red.

3. **Read it like a person, not a console.** Sentence case. A humanist sans.
   No monospace ALL-CAPS labels, no timestamps dressed as data. Numbers mean
   something (minutes left, last seen) and are set in the same friendly type,
   with tabular figures — not a "technical" typeface.

4. **Warm, quiet, and finished.** Soft shadows over hard hairlines, generous
   radius, real whitespace, one gentle motion grammar. Every empty and loading
   state is authored so the product never looks broken or blank.

5. **The same language reaches the lock.** The screen that stops someone is
   unmistakably the product that let them in — same ring, same colours, same
   warm paper. Calm, kind, firm, in that order.

---

## 2. Colour

Roles, not shades. Every value is a CSS custom property in `theme.css`; light
is the default and the computer's own screens are always light. Contrast ratios are against the
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
| `--ink-3` | `#8b857b` | | | |

Derived, not on the board: `--scrim` (the dialog backdrop, warm
`rgba(30,26,18,.45)`; black `.55` in dark), `--glow` (the 3 px brand halo on a
focused field), `--on-brand` (text on a green fill: white in light, near-black
in dark), and the motion tokens in §4. Focus is a 2 px `--brand` outline with
a 2 px offset; ink on a green fill.

### The avatar set — curated, never random

Eight warm pairs, chosen by `hash(id) % 8` (`web/src/lib/avatar.ts`), so a
family of faces looks like one family:

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

**Radius** — three steps plus the pill:

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
black-based shadows (`theme.css`).

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

Pill, Figtree 600. Focus: 2 px `--brand` outline, 2 px offset (ink on a green
fill). Press `scale(.98)`. Disabled `opacity .45`.

| Variant | Rest | Hover | Use |
|---|---|---|---|
| **primary** | `--brand` fill, white text, `--shadow-1` | `--brand-strong` | the one main action per view |
| **secondary** | `--surface`, `--line-2` border, ink | `--surface-2` | supporting actions ("Not now", "Manage") |
| **quiet** | transparent, `--ink-2` | `--surface-2`, ink | Cancel, low-stakes |
| **danger** | transparent, `--stop` text + faint border | `--stop-tint` bg, `--stop` border | destructive, at rest; **solid** (`--stop` fill, white) only for the final confirm |

Sizes: `sm` 36px / 14px · `md` 44px / 15px · `lg` 52px / 17px (kid). One
`<Button>` component; the segmented control, filter pills and bracket picker are
their own primitives but share these tokens.

### Text input & select

`--surface` bg, 1px `--line-2`, `--r-sm`, min-height 44px, Figtree **16px**
(never mono). Label above: 13px/600 `--ink-2`, sentence case. Focus: border
`--brand` + `0 0 0 3px` brand glow. Error: border `--stop`, hint text `--stop`.
Select adds a CSS chevron. Hint 12.5px `--ink-3`.

### Card

`--surface`, `--line`, `--r`, `--shadow-1`, padding 20px. No corner ticks, no
dot-grid. Interactive cards get the hover lift. Three card recipes:

- **Family card** — the avatar ring; name (H3); a meta line (`Kid · Mia's
  laptop`); one plain sentence of time ("**27 min** left of 1 h 15 min", "12
  min today · no limit set", "Paused by you"). A waiting request sits on the
  card with its two answers. Time's up: the ring completes in `--stop` and a
  `Time's up` tag sits by the name.
- **Computer card** — name (H3), who uses it (meta), a status tag (Online /
  Offline / Away, allowed / Paused / Not set up yet / Pausing… / Resuming…),
  one status line ("Last online 20 min ago"), and a row of secondary/quiet
  buttons; the rest under Details.
- **Plain card** — the generic container for settings rows and sheets.

### Avatar + activity ring — the mark

Identity disc (emoji or monogram, on an avatar-set pair) centred inside a ring.
One ring component draws every ring (`web/src/components/Ring.tsx`):

- stroke 9% of the diameter on hero rings (120 px and up), 8% from 48 px, 7%
  below; the arc starts flush under the tick and sweeps clockwise to a round
  cap.
- **the tick** at twelve o'clock, in ink, on every ring 40 px and larger,
  painted last.
- fill `--brand`; `--warn` at 15 minutes or less; `--stop` only at zero (time's
  up); **paused = a dashed `--ink-3` ring**, no fill.
- no limit set → just the track and the tick.

Sizes: 28 px (rail), 64 px (family cards), 140–220 px (hero, the lock). The
static mark (favicon, lockup, app icon) is fixed at 40% — a morning, most of
the day still ahead; its heavier stroke (16% of the ring) holds at 16 px.

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

`--surface`, `--r-lg`, `--shadow-2`. Scrim `--scrim`, no dot-grid. Title is an **H2 in sentence
case** (not a mono-caps 11px "terminal" title). Close `✕` is a 44px target. Traps
focus, restores on close, Escape to dismiss. Destructive confirm: danger-**solid**
final button, with type-the-name confirmation.

### App shell & navigation

- **The rail** (`--rail` plane, `layout/Shell.tsx`): the lockup, then **Family ·
  Computers · Settings · Me** as pill rows — active = `--brand-tint` bg +
  `--brand-ink` text + the leading icon. Below, **Today**: each person as a
  28 px ring, their name and time left. Footer: who is signed in, and
  sign-out.
- **Below 1024 px** the rail becomes a drawer behind a slim top bar with the
  lockup and a menu button.
- **Icons come from `brand/icons/` only** (24 grid, 2 px stroke, round caps and
  joins, `currentColor`), through `components/Icon.tsx`. No cog (settings is a
  pair of sliders), no shield. Emoji are faces, never icons.

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
- **Success**: a short toast that states the result ("Gave Mia 15 more
  minutes."), with **Undo** where the inverse is one call.

---

## 6. The computer's own screens

The app window, the warnings and the lock on the managed computer use the same
light tokens, the same ring and Figtree. Their layout, sizes and words are
[`DESIGN-CLIENT.md`](DESIGN-CLIENT.md) and [`BRAND-CLIENT.md`](BRAND-CLIENT.md);
the key moments are drawn on the brand board (§05 a–c).

---

## 7. What we deliberately dropped

Corner registration ticks, the dot-grid texture, the 5×7 dot-matrix and fleet
glyph strips, the OLED-black canvas, the mono ALL-CAPS metadata voice, random
HSL avatar colours, ink-on-ink "primary" buttons, and the harsh black modal
scrim. They belonged to the surveillance-console era. What we keep from it: the
discipline — a tiny palette, one accent that means *stop*, hierarchy from
type and space before boxes, and every diagram carrying real meaning.
