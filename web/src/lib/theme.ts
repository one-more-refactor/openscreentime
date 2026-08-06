import { useSyncExternalStore } from "react";

export type Theme = "dark" | "light";
const KEY = "openscreentime-theme";

const systemDark = window.matchMedia("(prefers-color-scheme: dark)");

/** Stored choice wins; otherwise follow the OS. Both modes are first-class. */
export function getInitialTheme(): Theme {
  const stored = localStorage.getItem(KEY);
  if (stored === "dark" || stored === "light") return stored;
  return systemDark.matches ? "dark" : "light";
}

// Module-level store so every useTheme() consumer shares one state — toggling
// in Settings updates the Shell (and vice versa) immediately.
let current: Theme = getInitialTheme();
const listeners = new Set<() => void>();

export function applyTheme(theme: Theme, persist = true) {
  current = theme;
  document.documentElement.setAttribute("data-theme", theme);
  if (persist) localStorage.setItem(KEY, theme);
  listeners.forEach((l) => l());
}

// Until the user toggles explicitly, track the OS live.
systemDark.addEventListener("change", (e) => {
  if (localStorage.getItem(KEY) === null) {
    applyTheme(e.matches ? "dark" : "light", false);
  }
});

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export type ThemeMode = Theme | "system";

/** "system" until the user pins a mode explicitly. */
export function themeMode(): ThemeMode {
  const stored = localStorage.getItem(KEY);
  return stored === "dark" || stored === "light" ? stored : "system";
}

/** Un-pin: forget the stored choice and follow the OS again, live. */
export function followSystem() {
  localStorage.removeItem(KEY);
  applyTheme(systemDark.matches ? "dark" : "light", false);
}

export function useTheme() {
  const theme = useSyncExternalStore(subscribe, () => current);
  return {
    theme,
    mode: themeMode(),
    toggle: () => applyTheme(current === "dark" ? "light" : "dark"),
    setTheme: applyTheme,
    followSystem,
  };
}
