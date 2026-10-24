import { defineConfig } from "@playwright/test";

// End-to-end tests run the production bundle (web/dist) in the Chrome installed on the
// machine (no browser download). Two servers: Vite preview sends the isolation headers;
// the plain static server exercises the coi-serviceworker and offline path.
export default defineConfig({
  testDir: "./e2e",
  timeout: 90_000,
  expect: { timeout: 20_000 },
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [["list"]],
  use: {
    channel: "chrome",
    headless: true,
    viewport: { width: 1400, height: 900 },
    trace: "retain-on-failure",
  },
  webServer: [
    { command: "npx vite preview --port 4190 --strictPort", url: "http://localhost:4190/", reuseExistingServer: false, timeout: 30_000 },
    { command: "node e2e/static-server.mjs 4191", url: "http://localhost:4191/", reuseExistingServer: false, timeout: 30_000 },
  ],
});
