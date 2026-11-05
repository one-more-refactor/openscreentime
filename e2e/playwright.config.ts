// Browser tests of the console's main journeys.
//
// They run against the console in design-review mode (VITE_USE_MOCK=1): the
// bundled sample household in web/src/mock.ts, no server, no database, no
// agent. That makes them fast and deterministic, and it means they test the
// console — what a parent clicks and reads — not the server. The server's
// own behaviour is covered by `cargo test` in server/ (against Postgres).
//
//   cd web && bun install            # once
//   cd e2e && npm install && npx playwright install chromium
//   npx playwright test              # starts the console itself
import { defineConfig, devices } from "@playwright/test";

const PORT = Number(process.env.OST_E2E_PORT ?? 4173);
const BASE = `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: "./tests",
  testMatch: "**/*.e2e.ts",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 2 : undefined,
  reporter: process.env.CI ? [["github"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: BASE,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    // The console formats times for the browser's zone and locale; pin both.
    locale: "en-GB",
    timezoneId: "Europe/Berlin",
  },
  projects: [
    { name: "desktop", use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } } },
    { name: "mobile", use: { ...devices["Pixel 7"] } },
  ],
  webServer: {
    // The Vite dev server in mock mode. `bunx vite` resolves web/node_modules.
    command: `bunx vite --host 127.0.0.1 --port ${PORT} --strictPort`,
    cwd: "../web",
    url: BASE,
    env: { VITE_USE_MOCK: "1" },
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
