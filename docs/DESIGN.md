# Design System

OpenScreenTime keeps Nothing's *restraint* — hierarchy in grayscale, hairlines
before boxes, one red that means "interrupt" and is absent on a healthy day —
and drops its *volume*. Since 0.6 the product is **warm, not loud**: sentence
case for everything a human reads, a humanist sans for content, monospace only
for small labels and data. Dot-matrix type, ALL-CAPS copy and the LED aesthetic
are retired (they read as surveillance, the exact thing the brand escapes).

The canonical values live in `web/src/theme.css` (its header states the rules)
and `web/src/me.css` (the child's page). This document explains them; when the
two disagree, the CSS is right and this file needs fixing.

## Tokens (`web/src/theme.css`)

```css
:root, :root[data-theme="dark"] {
  /* surfaces — OLED black, elevation by surface step */
  --bg: #000000;  --surface: #111111;  --surface-2: #1a1a1a;
  --line: #222222;  --line-2: #333333;  --rail: #0a0a0a;

  /* ink — the four-level grayscale hierarchy (display / primary / secondary / faint) */
  --fg-display: #ffffff;  --fg: #e8e8e8;  --fg-dim: #999999;  --fg-faint: #7a7a7a;

  /* the ONE accent — an urgent interrupt only (blocked, tamper, time-up, a destructive
     action being taken). Never decoration, never a resting section header. */
  --accent: #d71921;  --accent-subtle: rgba(215,25,33,.15);

  /* data status — encoded in the value, never as a row background */
  --ok: #4a9e5c;  --warn: #d4a843;  --crit: #d71921;  --idle: #666666;

  /* shape: cards ≤16px, buttons are pills; "technical" corners are 6px */
  --radius: 12px;  --radius-sm: 6px;

  /* the one focus ring */
  --focus: var(--fg-dim);

  /* motion: percussive chrome, living data, one easing — no spring, no bounce */
  --ease: cubic-bezier(.25,.1,.25,1);  --dur-tick: 180ms;  --dur-data: 400ms;

  /* depth is ambient and tokenised: resting vs floating/hover */
  --elev-1 … --elev-2  (see theme.css — an inset top edge stands in for a shadow on black)
}
```

Light mode redefines the same names (warm off-white `#f5f5f4`, black ink, status
colours re-derived for contrast: `--ok #2e7d46`, `--warn #8a6300`, `--crit #b3151c`).
`--fg-faint` is tuned to still pass AA at label sizes in both modes; it is for
metadata and hairlines, not body copy.

## Typography

- **Content:** Space Grotesk (`--font-sans`), sentence case, weights 400/600.
- **Labels & data:** Space Mono (`--font-mono`), small, uppercase, `letter-spacing .08em`,
  tabular numerals. This is the *tertiary* voice — captions, refs, timestamps — never a
  headline and never a sentence a person is meant to read.
- **The child's page** adds Nunito for the "playful" look (see below).
- Max three sizes and two weights per screen. Numbers *mean* something (a count, minutes
  left); they never pose.

## Motifs

- **Hierarchy is grayscale, not boxes.** The most important thing on a screen is never
  boxed. Containers use the lightest sufficient tool: spacing → hairline → border → surface.
- **Dot grid** as a background texture only (`.dotgrid`, 0.1–0.2 opacity), never over content.
- **Elevation, not shadow soup:** `--elev-1` rests, `--elev-2` floats or is hovered.
- **Status dots are flat** — no glow, no blur.
- **Time bars are segmented: one cell = 15 minutes** — a diagram that carries meaning.
- **The activity ring** is the product's one mark (`AvatarRing`, the favicon, the wordmark's
  marque): most of a day still ahead.
- **Red is an interrupt.** Locked, tamper, time-up, and the moment a destructive action is
  *taken* (hover/active/confirm). A calm day shows no red at all — including the Danger
  Zone, which is monochrome at rest.
- Glyphs are monoline SVG. The parent-picked "faces" are the one deliberate exception —
  emoji, chosen for warmth; they are identity, not iconography.

## Components (`web/src/components`)

- `Wordmark` — the lockup: ring marque + "Open" (dim) "ScreenTime" (600). Rail, login, README.
- `Button` / `.ch-btn` — pill, sans, sentence case; `primary` (ink-filled), secondary
  (hairline), `danger` (hairline that turns `--crit` only when reached for), `ghost`.
  Direction of travel: one button grammar everywhere (the mono-caps `Button` variant is
  legacy and should converge on the pill).
- `AvatarRing` — the ring around a person; the family grid's unit.
- `SecuritySlider` — the protection presets.
- `UnlockCodePanel` — the rotating per-device unlock ("parent") code with its countdown ring.
- `Moments` — the day's story, not a log: only moments that mattered.
- `WhereTheTime` — apps, sites (age-gated), the day's curve.
- `TextInput`, `Toggle`, `TagInput`, `PasskeyButton`, `LockOverlay` (a preview of the
  device's overlay for design reference).

## Layout

- Left rail: the wordmark, then **Family / Devices / Settings**, the household's day at a
  glance (each person's ring), and identity + sign-out at the bottom. The rail is its own
  plane (`--rail`).
- Pages are stacked sections with a sentence-case `h2`; airy, content max-width ~1200px,
  designed to be read on a phone.
- Loading is a quiet breathe (`.wait-text` / the structural skeletons), never a spinner
  or a toast.

## Host-side full-screen interruption (agent GUI)

The overlay the **agent** draws on the child's screen shares this language and, crucially,
this *voice*: black, sentence case, one fact, one next step. The words come from the
lock reason itself so every surface says the same thing — **"Stop — time's up for today"**,
**"Goodnight — screens are off until morning"**, **"Not now"**, and for a parent's pause
**"Paused — a parent paused this computer. Save your work — it pauses in 2 min."** A
wind-down countdown precedes the stop for every age. Calm, not punitive.

## The person's own page (`/me`) — three looks (0.4)

A member session (a child, or an adult who only self-tracks) has exactly one page: their own.
It is also what a parent sees under "My screen time". The console's monochrome system is the
base; the page is scoped by `.me.theme-*` (see `web/src/me.css`) so nothing leaks outward:

| look | for | what it is |
|---|---|---|
| `playful` | little / kid | **One huge ring** (24px stroke, round caps) with the minutes left inside, Nunito 800/900, a warm sun palette (`#FFF7E8` paper, `#FFB020` ring, `#58CC02` "ask" button, `#30326B` bedtime card). Duolingo energy, no mascot, no confetti. The stop is a red ring and the word **Stop**. |
| `calm` | teens | The console's own tokens, a thin ring, a mono stats row (used / limit / earned), blocked as a list, ask as pills. |
| `plain` | adults | No ring. A compact private dashboard: minutes today, allowed hours, what they've blocked, devices. |

Motion in all three is the living-data rule only: the ring draws itself on arrival (900 ms,
`stroke-dashoffset`), the number counts up to meet it. `prefers-reduced-motion` turns both off.
The parent picks the look per child (auto by bracket, or an explicit override) on the child's page.

Enforcement copy is the same plain words everywhere: "Stop — time's up for today", "paused by a
parent". The **unlock code** (read live from the console — there is no authenticator app and no
QR for devices) replaces the parent code / PIN in all copy; **recovery codes** are the one-time
spare keys.

### Change mode

The console has one security state, and it is visible. Reading is free; changing needs a second
factor **once**: a verified code turns *change mode* on for fifteen minutes. The control lives in
the rail footer (and the phone drawer): a shut lock and *Make changes* while locked; an open
lock, the minutes left, *Extend* (once) and *Lock* while on. Every control that mutates sits at
the same reduced presence (`[data-changemode="off"]`, opacity 0.55) until it is on, then the whole
console relaxes at once; the first locked control you touch opens the dialog, nothing asks
again while it is on. Turning it on or off plays a full-screen veil (`ChangeModeVeil`): an ink
field, a lock glyph that draws and opens (or closes), one ring sweep, the words — ≈1.1 s in,
≈0.7 s out, a 150 ms fade under `prefers-reduced-motion`. It never blocks input.
