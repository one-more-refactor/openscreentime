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
- **Hairlines.** 1px `--line` borders everywhere; no shadows for separation.
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
