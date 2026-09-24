import { defineConfig, searchForWorkspaceRoot } from "vite";
import react from "@vitejs/plugin-react";

// Web dev server on :5173, proxying the two API surfaces to the Rust server on :8080.
// (See docs/DEVELOPMENT.md — Ports.)
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
    // The icon set lives in ../brand/icons (one source for the console and
    // the device); let the dev server read it.
    fs: { allow: [searchForWorkspaceRoot(process.cwd()), "../brand/icons"] },
    proxy: {
      "/api": {
        target: "http://localhost:8080",
        changeOrigin: true,
      },
      "/agent": {
        target: "http://localhost:8080",
        changeOrigin: true,
        ws: true, // the agent's command bus
      },
    },
  },
});
