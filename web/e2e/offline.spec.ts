import { expect, test } from "@playwright/test";
import { waitForApp } from "./helpers.js";

// Plain static server: no isolation headers, so the vendored coi-serviceworker must
// install, add the headers and precache the bundle; then the app must work offline.
const url = "http://127.0.0.1:4191/";

test("service worker isolates the page and the app runs offline", async ({ page, context }) => {
  await page.goto(url);
  // First visit: the worker installs and reloads the page once.
  await page.waitForFunction(() => globalThis.crossOriginIsolated === true, null, { timeout: 60_000 });
  await waitForApp(page);
  await expect(page.locator(".panel")).toHaveCount(8);
  const cacheKeys: string[] = await page.evaluate(() => caches.keys());
  expect(cacheKeys.some((k) => k.startsWith("lvp-"))).toBe(true);

  await context.setOffline(true);
  await page.reload();
  await page.waitForFunction(() => globalThis.crossOriginIsolated === true, null, { timeout: 60_000 });
  await waitForApp(page);
  await expect(page.locator(".panel")).toHaveCount(8);
  const threads = await page.evaluate(() => (globalThis as unknown as { lvp: { threads: number | null } }).lvp.threads);
  expect(threads).toBeGreaterThan(0);
  await context.setOffline(false);
});
