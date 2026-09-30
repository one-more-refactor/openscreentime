// ============================================================================
// Light, dark, or match my system — one choice, remembered.
//
// A brand-new visitor gets the warm light theme (a family console should not
// inherit a stark dark desktop by accident). Choosing "Match my system" is
// stored as exactly that, so it survives a reload and follows the OS live.
// The resolved theme lands on <html data-theme>, and the browser chrome's
// theme-color follows it. index.html runs the same logic before first paint.
// ============================================================================
import { useSyncExternalStore } from "react";

export type Theme = "dark" | "light";
export type ThemeMode = Theme | "system";

const KEY = "openscreentime-theme";
const PAPER: Record<Theme, string> = { light: "#f4f2ee", dark: "#171614" };

const systemDark =
  typeof window !== "undefined" && window.matchMedia
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : null;

function readMode(): ThemeMode {
  try {
    const v = localStorage.getItem(KEY);
    return v === "dark" || v === "light" || v === "system" ? v : "light";
  } catch {
    return "light";
  }
}

function resolve(mode: ThemeMode): Theme {
  if (mode === "system") return systemDark?.matches ? "dark" : "light";
  return mode;
}

let snapshot = { mode: readMode(), theme: resolve(readMode()) };
const listeners = new Set<() => void>();

function paint(theme: Theme) {
  document.documentElement.setAttribute("data-theme", theme);
  document
    .querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]')
    .forEach((m) => m.setAttribute("content", PAPER[theme]));
}

/** Choose a mode; it is remembered in this browser. */
export function setThemeMode(mode: ThemeMode) {
  try {
    localStorage.setItem(KEY, mode);
  } catch {
    /* private window: the choice lasts for this visit */
  }
  snapshot = { mode, theme: resolve(mode) };
  paint(snapshot.theme);
  listeners.forEach((l) => l());
}

/** First paint (main.tsx): apply whatever was chosen before. */
export function initTheme() {
  let mode = readMode();
  // Design review only (VITE_USE_MOCK=1): ?theme=dark|light|system for
  // screenshots, without touching the stored choice. Compiled out otherwise.
  if (import.meta.env.VITE_USE_MOCK === "1") {
    const q = new URLSearchParams(window.location.search).get("theme");
    if (q === "dark" || q === "light" || q === "system") mode = q;
  }
  snapshot = { mode, theme: resolve(mode) };
  paint(snapshot.theme);
}

// While following the system, follow it live.
systemDark?.addEventListener?.("change", () => {
  if (snapshot.mode !== "system") return;
  snapshot = { mode: "system", theme: resolve("system") };
  paint(snapshot.theme);
  listeners.forEach((l) => l());
});

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useTheme() {
  const snap = useSyncExternalStore(subscribe, () => snapshot);
  return { theme: snap.theme, mode: snap.mode, setMode: setThemeMode };
}
