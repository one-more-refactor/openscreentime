/** @type {import('tailwindcss').Config} */
// Utilities read the same tokens as src/theme.css (the brand board's set), so
// light and dark swap by re-declaring the variables, never by rewriting classes.
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        bg: "var(--bg)",
        surface: "var(--surface)",
        "surface-2": "var(--surface-2)",
        rail: "var(--rail)",
        line: "var(--line)",
        "line-2": "var(--line-2)",
        ink: "var(--ink)",
        "ink-2": "var(--ink-2)",
        "ink-3": "var(--ink-3)",
        brand: "var(--brand)",
        "brand-strong": "var(--brand-strong)",
        "brand-tint": "var(--brand-tint)",
        "brand-ink": "var(--brand-ink)",
        warn: "var(--warn)",
        "warn-tint": "var(--warn-tint)",
        stop: "var(--stop)",
        "stop-tint": "var(--stop-tint)",
      },
      borderRadius: {
        sm: "var(--r-sm)",
        DEFAULT: "var(--r)",
        lg: "var(--r-lg)",
      },
      fontFamily: {
        sans: ["Figtree Variable", "Figtree", "system-ui", "sans-serif"],
        // Only for a literal code (an unlock code, a recovery code).
        mono: ['"Space Mono"', "ui-monospace", "monospace"],
      },
    },
  },
  plugins: [],
};
