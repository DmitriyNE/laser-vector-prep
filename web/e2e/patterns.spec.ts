import { expect, test, type Page } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

type Lvp = {
  evaluating: boolean;
  evaluation: { revision: number } | null;
  profile: { workingLayers: { passes: { pattern: Record<string, unknown>; breaks?: Record<string, unknown> | null }[] }[] };
};

const revision = (page: Page) =>
  page.evaluate(() => (globalThis as unknown as { lvp: Lvp }).lvp.evaluation?.revision ?? 0);

/** Wait for an evaluation after `rev` to finish and return its summary. */
async function evaluatedAfter(page: Page, rev: number) {
  await page.waitForFunction(
    (r) => {
      const lvp = (globalThis as unknown as { lvp: Lvp }).lvp;
      return !lvp.evaluating && (lvp.evaluation?.revision ?? 0) > r;
    },
    rev,
    { timeout: 60_000 },
  );
  return summary(page);
}

/** The copper fill pass of the default profile, as stored. */
const copperPass = (page: Page) =>
  page.evaluate(() => {
    const p = (globalThis as unknown as { lvp: Lvp }).lvp.profile.workingLayers[0]!.passes[0]!;
    return JSON.parse(JSON.stringify(p)) as { pattern: Record<string, unknown>; breaks?: Record<string, unknown> | null };
  });

async function openCopperPass(page: Page) {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  await page.locator(".row.pass").first().locator(".name").click();
}

test("contour fill marks its rings outward when asked, with the same paths", async ({ page }) => {
  await openCopperPass(page);
  const before = await summary(page);
  const rev = await revision(page);
  await page.getByLabel("Ring order").selectOption("outward");
  const after = await evaluatedAfter(page, rev);
  expect(after.hasErrors).toBe(false);
  expect(after.passes[0]!.paths).toBe(before.passes[0]!.paths);
  expect((await copperPass(page)).pattern.ringOrder).toBe("outward");
  await expect(page.locator(".row.pass").first().locator(".sub")).toContainText("outward");
});

test("linked rings join each run of nested rings into one path", async ({ page }) => {
  await openCopperPass(page);
  const before = await summary(page);
  const rev = await revision(page);
  await page.getByLabel("link rings").check();
  const after = await evaluatedAfter(page, rev);
  expect(after.hasErrors).toBe(false);
  expect(after.passes[0]!.paths).toBeLessThan(before.passes[0]!.paths);
  // The connectors are marks: the total length grows.
  expect(after.passes[0]!.lengthMm).toBeGreaterThan(before.passes[0]!.lengthMm);
  expect((await copperPass(page)).pattern.link).toBe(true);
  await expect(page.locator(".row.pass").first().locator(".sub")).toContainText("linked");
});

test("centreline engraving draws single lines along the copper", async ({ page }) => {
  await openCopperPass(page);
  const rev = await revision(page);
  await page.getByLabel("Pattern").selectOption("centreline");
  const after = await evaluatedAfter(page, rev);
  expect(after.hasErrors).toBe(false);
  const pass = after.passes[0]!;
  expect(pass.paths).toBeGreaterThan(0);
  expect(pass.open).toBeGreaterThan(0);
  await expect(page.locator(".row.pass").first().locator(".sub")).toContainText("centreline");
  // A minimum length leaves the short lines out.
  const rev2 = await revision(page);
  await page.getByLabel("Minimum length (mm)").fill("1");
  const longer = await evaluatedAfter(page, rev2);
  expect(longer.passes[0]!.paths).toBeLessThan(pass.paths);
});

test("hatch rows link along the boundary into zigzags", async ({ page }) => {
  await openCopperPass(page);
  let rev = await revision(page);
  await page.getByLabel("Pattern").selectOption("hatch");
  const rows = await evaluatedAfter(page, rev);
  rev = await revision(page);
  await page.getByLabel("Link rows up to (mm)").fill("0.1");
  const linked = await evaluatedAfter(page, rev);
  expect(linked.hasErrors).toBe(false);
  expect(linked.passes[0]!.paths).toBeLessThan(rows.passes[0]!.paths / 3);
  expect(linked.passes[0]!.lengthMm).toBeGreaterThan(rows.passes[0]!.lengthMm);
  expect((await copperPass(page)).pattern.linkMm).toBe(0.1);
  await expect(page.locator(".row.pass").first().locator(".sub")).toContainText("linked");
});
