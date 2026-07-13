/** @type {import('tailwindcss').Config} */
// Colors map to the CSS variables defined in src/theme.css so the light/dark
// themes swap by re-declaring the variables, not by rewriting utility classes.
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        bg: "var(--bg)",
        surface: "var(--surface)",
        "surface-2": "var(--surface-2)",
        line: "var(--line)",
        "line-2": "var(--line-2)",
        fg: "var(--fg)",
        "fg-dim": "var(--fg-dim)",
        "fg-faint": "var(--fg-faint)",
        accent: "var(--accent)",
        "accent-dim": "var(--accent-dim)",
        ok: "var(--ok)",
        warn: "var(--warn)",
        crit: "var(--crit)",
        idle: "var(--idle)",
      },
      borderRadius: {
        DEFAULT: "var(--radius)",
      },
      fontFamily: {
        mono: ['"Space Mono"', 'ui-monospace', '"SFMono-Regular"', 'Menlo', 'Consolas', 'monospace'],
        dot: ['"DotGothic16"', '"Space Mono"', 'ui-monospace', 'monospace'],
      },
      letterSpacing: {
        label: "0.08em",
        dot: "0.14em",
      },
    },
  },
  plugins: [],
};
