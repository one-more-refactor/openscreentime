import { useSyncExternalStore } from "react";

export type Theme = "dark" | "light";
const KEY = "sentinel-theme";

export function getInitialTheme(): Theme {
  const stored = localStorage.getItem(KEY);
  if (stored === "dark" || stored === "light") return stored;
  return "dark"; // dark is the primary theme
}

// Module-level store so every useTheme() consumer shares one state — toggling
// in Settings updates the Shell (and vice versa) immediately.
let current: Theme = getInitialTheme();
const listeners = new Set<() => void>();

export function applyTheme(theme: Theme) {
  current = theme;
  document.documentElement.setAttribute("data-theme", theme);
  localStorage.setItem(KEY, theme);
  listeners.forEach((l) => l());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useTheme() {
  const theme = useSyncExternalStore(subscribe, () => current);
  return {
    theme,
    toggle: () => applyTheme(current === "dark" ? "light" : "dark"),
    setTheme: applyTheme,
  };
}
