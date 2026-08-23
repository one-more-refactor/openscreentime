# Design System — "Nothing"-style

The control center should feel like Nothing OS: stark monochrome, generous negative space,
dot-matrix typography, precise thin lines, and a single restrained accent. Function-first,
gadget-like, quietly confident. No gradients-as-decoration, no drop-shadow soup, no rounded
candy. Think engineering blueprint meets consumer product.

## Tokens (Tailwind theme + CSS variables)

```css
:root {
  /* surfaces — near-pure monochrome */
  --bg:        #0a0a0a;   /* app background (dark is the primary theme) */
  --surface:   #141414;   /* cards / panels */
  --surface-2: #1c1c1c;   /* raised / hover */
  --line:      #2a2a2a;   /* hairline borders */
  --line-2:    #3a3a3a;

  /* ink */
  --fg:        #fafafa;   /* primary text */
  --fg-dim:    #9a9a9a;   /* secondary */
  --fg-faint:  #5a5a5a;   /* tertiary / disabled */

  /* the one accent — Nothing red */
  --accent:    #d71921;
  --accent-dim:#7a0f13;

  /* status LEDs */
  --ok:        #37d67a;   /* online */
  --warn:      #f2c94c;   /* attention */
  --crit:      #d71921;   /* locked / tamper (reuses accent) */
  --idle:      #5a5a5a;   /* offline */

  --radius: 4px;          /* small, hard-ish corners only */
}
```

A light theme mirrors these (white surfaces, black ink, same accent). Dark is default.

## Typography

- **Display / numerals / labels:** a dot-matrix / LED face. Ship a self-hosted font resembling
  Nothing's *NDot* — use **"Nothing You Could Do"** ✗ (no). Instead bundle a dotted font such as
  a monospace like **Space Mono** for body and a dotted display face for headings and big
  numbers. If a dot-matrix font isn't available at build time, fall back to `ui-monospace,
  "Space Mono", monospace` and render section headers in `uppercase` with wide `letter-spacing`.
- **Everything structural is monospace and uppercase** with `letter-spacing: 0.08em` for labels.
- Big status numbers (screen-time remaining, device counts) are oversized dot/LED numerals.

## Motifs

- **Dot grid.** Subtle repeating dot pattern as texture on empty panels
  (`radial-gradient(var(--line) 1px, transparent 1px)` at ~16px spacing).
- **Hairlines, then depth.** 1px `--line` borders everywhere; separation is the hairline.
  Depth is ambient and tokenised — `--elev-1` for a resting card, `--elev-2` for anything
  floating (modal, drawer) or hovered — never an ad-hoc shadow. The dark theme has no light to
  cast a shadow in, so its tokens use a faint inset top edge and a surface step instead; the
  rail is its own plane (`--rail`). Interactive cards lift 1px on hover; every button presses.
- **LED indicators.** Small filled circles with a soft glow for status (online/offline/locked).
- **Glyph / pixel icons.** Simple, monoline or pixel-style icons. No filled emoji.
- **Mono captions** under controls, like device labels on hardware.
- **Red is rare.** Accent only for: destructive actions, locked/tamper state, the active nav item.

## Components the web app needs

- `StatusLed` — colored dot + label (online/offline/locked/pending).
- `Panel` — bordered card with optional dot-grid background and a mono uppercase header.
- `Stat` — oversized dot-numeral with a mono caption (e.g. `07` DEVICES).
- `DeviceCard` — name, StatusLed, last-seen, per-user chips, quick actions (lock/unlock).
- `PolicyEditor` — structured form over the Policy jsonb (DNS allowlist, screen-time schedule,
  gamification toggles). Zero-trust framing: "BLOCKED BY DEFAULT — add exceptions below."
- `PasskeyButton` — the login/register affordance.
- `Toggle`, `TextInput`, `TagInput` (for DNS allowlists), `TimeRange`, `Button` (variants:
  `primary` mono outline, `danger` red).
- `EventFeed` — audit log with severity LEDs.
- `LockOverlay` preview — a mock of the full-screen host interruption for design reference.

## Layout

- Left rail: wordmark (`OpenScreenTime` wordmark), nav (DEVICES / PROFILES / EVENTS / SETTINGS),
  admin identity + logout at the bottom.
- Main: page header (mono uppercase + count Stat), content grid of `Panel`s.
- Density: airy. Big margins. Content max-width ~1200px.

## Host-side full-screen interruption (agent GUI)

The Duolingo-style lockout/nudge screen rendered by the **agent** must share this language:
black background, dot grid, one big dot-numeral countdown or streak flame drawn in monochrome,
a single accent-red action, mono uppercase copy ("TIME'S UP", "EARN 15 MIN — READ FOR 20").
Keep it calm and game-like, not punitive.

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
