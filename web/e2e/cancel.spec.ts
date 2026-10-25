import { expect, test, type Page } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4190/";

type Lvp = {
  evaluating: boolean;
  notice: string | null;
  evaluation: { passes: { paths: number }[] } | null;
  update: (f: (p: { workingLayers: { passes: { pattern: { kind: string; pitchMm?: number } }[] }[] }) => void) => void;
};

/** Set the copper contour fill pitch (the edit evaluates after the store's debounce). */
const setPitch = (page: Page, pitch: number) =>
  page.evaluate((p) => {
    (globalThis as unknown as { lvp: Lvp }).lvp.update((profile) => {
      const pattern = profile.workingLayers[0]!.passes[0]!.pattern;
      if (pattern.kind === "contourFill") pattern.pitchMm = p;
    });
  }, pitch);

const copperPaths = async (page: Page) => (await summary(page)).passes[0]!.paths;

// A 1 µm fill of the board takes far longer than these tests allow: it only finishes if
// cancellation does not work.
const HEAVY_PITCH = 0.001;

test("Cancel stops a long evaluation and keeps the previous result", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  const before = await copperPaths(page);

  await setPitch(page, HEAVY_PITCH);
  const cancel = page.getByRole("button", { name: "Cancel" });
  await expect(cancel).toBeVisible({ timeout: 10_000 });
  // Progress is reported from the worker while it runs.
  await expect(page.locator(".busy progress")).toBeVisible();
  const clicked = Date.now();
  await cancel.click();
  await page.waitForFunction(() => !(globalThis as unknown as { lvp: Lvp }).lvp.evaluating, null, { timeout: 10_000 });
  expect(Date.now() - clicked).toBeLessThan(5_000);
  await expect(cancel).toBeHidden();
  expect(await page.evaluate(() => (globalThis as unknown as { lvp: Lvp }).lvp.notice)).toMatch(/cancelled/);
  expect(await copperPaths(page)).toBe(before);

  // The app keeps working: the next edit evaluates normally.
  await setPitch(page, 0.05);
  await page.waitForFunction(
    (n) => {
      const lvp = (globalThis as unknown as { lvp: Lvp }).lvp;
      return !lvp.evaluating && lvp.evaluation !== null && lvp.evaluation.passes[0]!.paths !== n;
    },
    before,
    { timeout: 30_000 },
  );
  expect(await page.evaluate(() => (globalThis as unknown as { lvp: Lvp }).lvp.notice)).toBeNull();
});

test("an edit during a long evaluation supersedes it instead of waiting for it", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));

  // Reference: the 50 µm fill on its own.
  await setPitch(page, 0.05);
  await page.waitForFunction(() => !(globalThis as unknown as { lvp: Lvp }).lvp.evaluating);
  await page.waitForTimeout(400);
  await page.waitForFunction(() => !(globalThis as unknown as { lvp: Lvp }).lvp.evaluating);
  const coarse = await copperPaths(page);

  await setPitch(page, HEAVY_PITCH);
  await expect(page.getByRole("button", { name: "Cancel" })).toBeVisible({ timeout: 10_000 });
  const edited = Date.now();
  await setPitch(page, 0.05);
  await page.waitForFunction(
    (n) => {
      const lvp = (globalThis as unknown as { lvp: Lvp }).lvp;
      return !lvp.evaluating && lvp.evaluation?.passes[0]?.paths === n;
    },
    coarse,
    { timeout: 30_000 },
  );
  expect(Date.now() - edited).toBeLessThan(10_000);
  expect(await page.evaluate(() => (globalThis as unknown as { lvp: Lvp }).lvp.notice)).toBeNull();
});
