import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { App } from "./App";
import { initTheme } from "./lib/theme";
import "@fontsource-variable/space-grotesk/wght.css";
import "@fontsource/space-mono/400.css";
import "@fontsource/space-mono/700.css";
// Nunito is used by the playful look of a child's own page only.
import "@fontsource-variable/nunito/wght.css";
import "./theme.css";
import "./addon.css";
import "./me.css";

// Warm light by default for a brand-new visitor; any explicit prior choice
// (including follow-system) is respected. Runs before first paint.
initTheme();

const root = document.getElementById("root");
if (!root) throw new Error("#root not found");

createRoot(root).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
);
