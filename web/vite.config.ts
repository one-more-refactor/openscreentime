import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Web dev server on :5173, proxying the two API surfaces to the Rust server on :8080.
// (See docs/DEVELOPMENT.md — Ports.)
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": {
        target: "http://localhost:8080",
        changeOrigin: true,
      },
      "/agent": {
        target: "http://localhost:8080",
        changeOrigin: true,
        ws: true,
      },
    },
  },
});
