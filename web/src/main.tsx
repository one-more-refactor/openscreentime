import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { App } from "./App";
import { applyTheme, getInitialTheme } from "./lib/theme";
import "@fontsource-variable/space-grotesk/wght.css";
import "@fontsource/space-mono/400.css";
import "@fontsource/space-mono/700.css";
// Nunito is used by the playful look of a child's own page only.
import "@fontsource-variable/nunito/wght.css";
import "./theme.css";
import "./addon.css";
import "./me.css";

// Apply the stored-or-system theme before first paint, without persisting —
// system-follow stays live until the user explicitly toggles.
applyTheme(getInitialTheme(), false);

const root = document.getElementById("root");
if (!root) throw new Error("#root not found");

createRoot(root).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
);
