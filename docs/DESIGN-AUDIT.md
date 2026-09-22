# Design audit — why the current client reads inconsistent

The client was pushed from an austere "Nothing" dark system to a warm light one,
but only the surface changed: the **bones still execute the old system** (ink
fills, monospace ALL-CAPS labels, corner ticks, dot-grid, gray focus) while a
green ring and emoji faces sit on top. That mismatch is what reads as "someone
told a chatbot to do it." The twelve worst problems, each with the fix the new
language (`docs/DESIGN.md`, `web/design/reference.html`) dictates.

### 1. Two typographic voices fight — and the loud one is "surveillance"
Metadata everywhere is Space Mono, ALL-CAPS, letter-spaced: `.label`, `.ref`,
`.fam-meta`, `.ch-meta`, `.dev-fact`, `.ph-eyebrow`
(`web/src/theme.css:165, 181, 444, 508, 744, 1304`), `.apps-tile-state`
(`web/src/addon.css:68`). That is the exact "technical/monitoring" tone the
rebrand is trying to escape.
**Fix:** one humanist sans (Figtree), sentence case, tabular figures for numbers.
Monospace survives only for literal secret codes.

### 2. There is no brand colour in the system — green is a one-off
Green appears in exactly two decorative places: `AvatarRing`
(`web/src/components/AvatarRing.tsx:54`) and `Wordmark`
(`web/src/components/Wordmark.tsx:41`). Every structural fill is ink: the child
bar (`theme.css:461`), selected pills (`theme.css:890`), the "yes" button
(`theme.css:549`). The one identity element isn't wired into the system.
**Fix:** green becomes the affordance/positive role — primary buttons, active
nav, selected chips, the ring. Ink stops being the "action" colour.

### 3. The primary action is black, and half-defined
`Button.tsx` variant `primary` is a *transparent hairline*
(`web/src/components/Button.tsx:24`) — there is no filled primary button in the
component at all — while the real filled CTA `.ch-btn-yes` is ink-black
(`theme.css:549`). The most important action on every screen is either invisible
or a cold black slab.
**Fix:** primary = filled `--brand` green, one definition, used once per view.

### 4. At least nine divergent button/toggle systems
`Button.tsx`, `.ch-btn`/`.ch-btn-yes` (`theme.css:538`), `.fam-cta`
(`theme.css:491`), `.ph-action` (`theme.css:1316`), `.pill` (`theme.css:875`),
`.seg-btn` (`theme.css:1348`), `.apps-cat` (`addon.css:18`), `.add-bracket`
(`addon.css:123`), `.me-ask-big` (`me.css:142`). Each has its own padding,
radius, weight and hover.
**Fix:** one `<Button>` (4 variants × 3 sizes); pills/segments/brackets are
primitives that reuse the same tokens.

### 5. Avatar colours are randomly generated
`hueFor(seed)` feeds `hsl(${hue} 45% 88%)` in `AvatarRing.tsx:41,94` and
`Family.tsx:27,60`. Arbitrary hues across the family are the signature "chatbot"
look and clash with the warm palette.
**Fix:** a curated set of eight warm pairs, picked by `hash(id) % 8`.

### 6. Radius has no scale
Tiles 6px (`theme.css:46`), cards 12px (`theme.css:33`), me-cards 22px and 28px
(`me.css:54,78`), modal 12px, pills 999px — no intent.
**Fix:** `--r-sm 10 / --r 16 / --r-lg 24 / pill`.

### 7. "Hardware/surveillance" motifs contradict the warmth
Corner registration ticks `.tick` (`theme.css:202`, rendered in `Modal.tsx:100`),
dot-grid on empty states (`addon.css:247`), the 5×7 dot-matrix `.dm`
(`theme.css:252`), fleet-cell strips (`theme.css:232`), and the "Hardware-module
dialog" framing in `Modal.tsx:21`. These read as an ops console, not a family app.
**Fix:** remove them; empty states use the ring mark + warm copy.

### 8. The focus ring is low-contrast and off-brand
`--focus: var(--fg-dim)` — a mid-gray (`theme.css:42, 86`). It barely reads on
either surface and belongs to nothing.
**Fix:** 2px `--brand` ring + 2px offset everywhere (ink ring on green fills),
which also passes as a visible AA focus indicator.

### 9. The modal is cold and cramped
Title is Space Mono `.dot` at 11px (`Modal.tsx:108`), the scrim is a harsh
`rgba(0,0,0,0.72)` over a dot-grid (`Modal.tsx:87`), radius 12px.
**Fix:** sentence-case H2 title, warm `rgba(30,26,18,.5)` scrim, `--r-lg` 24px,
no dot-grid, `--shadow-2`.

### 10. Inputs feel like a terminal
`TextInput` renders the value itself in `font-mono` under a mono uppercase label
(`web/src/components/TextInput.tsx:20`) — a parent types a child's name in
monospace.
**Fix:** Figtree 16px input text, sentence-case 13px label, brand focus glow.

### 11. Green carries two conflicting meanings
Green means "time used" on the ring, favicon and wordmark, *and* "ok/online" on
device states and dots (`theme.css:642, 732`; `addon.css:100`). A user can't tell
whether green is good or just "spent."
**Fix:** one meaning — green = **healthy / within budget / good**. "Time used"
is reframed as "within budget" (green while inside the day, red only when over),
so green is always reassuring and red is always the single bad signal.

### 12. The default theme is the flat one
Ambient canvas warmth is dark-only (`theme.css:1511–1523`); the light default —
what almost everyone sees — gets nothing, so it looks flatter and cheaper than
the mode fewer people use.
**Fix:** give the light canvas its own subtle warm radial and warm-tinted
shadows, as in the reference.
