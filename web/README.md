# Sentinel — Web Control Center

The admin web app for **Sentinel**, a zero-trust device-management platform.
Nothing-style monochrome UI: dot-grid texture, hairlines, mono uppercase labels,
oversized dot-numerals, and one restrained accent (Nothing red).

Stack: **Bun · Vite · React · TypeScript (strict) · Tailwind · WebAuthn passkeys.**

## Quick start

```bash
bun install
bun run dev      # http://localhost:5173, proxies /api and /agent → :8080
bun run build    # tsc -b && vite build
bun run preview  # serve the production build
```

The dev server proxies `/api` and `/agent` to the Rust server on `:8080`
(see `../docs/DEVELOPMENT.md`). **No backend required for design review** — every
read degrades gracefully to bundled sample data (`src/mock.ts`) when the API is
unreachable, and a "MOCK DATA" badge appears in the rail. Force it with
`VITE_USE_MOCK=1`; disable the fallback with `VITE_USE_MOCK=0`.

## Design system (`src/theme.css`)

All tokens from `docs/DESIGN.md` are CSS variables under `:root[data-theme]`.
**Dark is default; light mirrors it.** Tailwind colors (`tailwind.config.js`) map
to those variables, so theming swaps variables, not utility classes. The
dot-matrix look is achieved without remote fonts: monospace stack
(`ui-monospace, "Space Mono", monospace`) with wide tracking (`.dot`, `.wordmark`,
`.label`) plus a `radial-gradient` dot-grid (`.dotgrid`) and glowing LEDs.

## Components (`src/components/`)

`StatusLed` · `Panel` (dot-grid option) · `Stat` (oversized dot-numeral) ·
`DeviceCard` · `PolicyEditor` · `PasskeyButton` · `Toggle` · `TextInput` /
`Select` · `TagInput` · `TimeRange` · `Button` (primary mono-outline + danger
red) · `EventFeed` (severity LEDs) · `Modal` · `LockOverlay` (agent-GUI preview).

## Pages (`src/pages/`, React Router)

- **Login** — dot wordmark, single `PasskeyButton`, register/first-admin toggle,
  agent-GUI lock preview.
- **Devices** — device-count `Stat`, `DeviceCard` grid, lock/unlock quick actions,
  add-device → enroll token, LAN Discovery panel with per-host enroll.
- **Device detail** — identity, per-user profile assignment, recent `EventFeed`,
  tamper-level toggle with the Level-3 confirm + recovery procedure (`TAMPER.md`).
- **Profiles** — kids/teen/default presets + custom, full `PolicyEditor`,
  duplicate/delete/save.
- **Events** — filterable audit feed (device/type/severity + payload search).
- **Settings** — admin identity, passkey list + add, theme, logout.

## API & types

`src/types.ts` mirrors the `docs/API.md` Policy and `docs/DATA_MODEL.md` entities
exactly (keep in sync with the Rust serde shapes). `src/api.ts` is the typed
client — session-cookie auth via `credentials: "include"`, WebAuthn ceremonies via
`@simplewebauthn/browser`, and the graceful mock fallback for reads.
