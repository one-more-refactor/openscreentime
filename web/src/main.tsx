import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { App } from "./App";
import { initTheme } from "./lib/theme";
// One humanist sans carries the whole product (docs/DESIGN.md §3); Space Mono
// survives only for literal secret codes.
import "@fontsource-variable/figtree/wght.css";
import "@fontsource/space-mono/400.css";
import "@fontsource/space-mono/700.css";
import "./theme.css";
import "./styles/family.css";
import "./styles/computers.css";
import "./styles/settings.css";
import "./styles/sign-in.css";
import "./styles/add.css";
import "./styles/person.css";
import "./styles/me.css";

// Warm light by default for a brand-new visitor; any explicit prior choice
// (including "match my system") is respected.
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
