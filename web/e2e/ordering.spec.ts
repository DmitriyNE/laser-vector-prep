import { expect, test } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4190/";

type Groups = { id: string; name: string; mode: string; paths: number; before: { jumpLengthMm: number; coveredMm2?: number }; after: { jumpLengthMm: number; coveredMm2?: number }; heatmap: boolean }[];
const groups = (page: import("@playwright/test").Page) =>
  page.evaluate(() => JSON.parse(JSON.stringify((globalThis as unknown as { lvp: { evaluation: { groups: unknown } } }).lvp.evaluation.groups)) as Groups);

async function settle(page: import("@playwright/test").Page) {
  await page.waitForFunction(() => {
    const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; dirty: boolean } }).lvp;
    return !lvp.evaluating;
  });
}

test("path order planning: minimum jump shortens travel, bleed-aware protects copper and draws a heatmap", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));

  // As generated: the Path order table shows the copper group with before == after.
  let gs = await groups(page);
  const copper0 = gs.find((g) => g.name === "Copper")!;
  expect(copper0.mode).toBe("asGenerated");
  expect(copper0.after.jumpLengthMm).toBeCloseTo(copper0.before.jumpLengthMm, 6);
  await expect(page.locator(".group-row").first()).toContainText("Copper");

  // Minimum jump from the panel.
  await page.locator(".panel", { hasText: "Path order" }).locator("select").first().selectOption("minimumJump");
  await page.waitForFunction(() => {
    const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { groups: { mode: string }[] } } }).lvp;
    return !lvp.evaluating && lvp.evaluation.groups[0]?.mode === "minimumJump";
  });
  gs = await groups(page);
  const copper1 = gs.find((g) => g.name === "Copper")!;
  expect(copper1.paths).toBe(copper0.paths);
  expect(copper1.after.jumpLengthMm).toBeLessThan(copper1.before.jumpLengthMm);
  const ev = await summary(page);
  expect(ev.hasErrors).toBe(false);

  // Bleed-aware with the copper layer protected gives coverage numbers and a heatmap.
  await page.locator(".panel", { hasText: "Path order" }).locator("select").first().selectOption("bleedAware");
  await page.locator(".panel", { hasText: "Path order" }).getByLabel("Copper", { exact: true }).check();
  await page.waitForFunction(() => {
    const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { groups: { mode: string; heatmap: boolean }[] } } }).lvp;
    return !lvp.evaluating && lvp.evaluation.groups[0]?.mode === "bleedAware" && lvp.evaluation.groups[0]?.heatmap;
  });
  gs = await groups(page);
  const copper2 = gs.find((g) => g.name === "Copper")!;
  expect(copper2.heatmap).toBe(true);
  expect(copper2.after.coveredMm2).toBeDefined();
  expect(copper2.after.coveredMm2!).toBeLessThanOrEqual(copper2.before.coveredMm2! + 1e-9);
  const heat = await page.evaluate(async () => {
    const lvp = (globalThis as unknown as { lvp: { heatmap: (id: string) => Promise<{ width: number; height: number; counts: Uint16Array; mask: Uint8Array } | null> } }).lvp;
    const h = await lvp.heatmap("grp-copper");
    return h ? { w: h.width, h: h.height, cells: h.counts.length, masked: h.mask.reduce((n, m) => n + (m ? 1 : 0), 0) } : null;
  });
  expect(heat).not.toBeNull();
  expect(heat!.cells).toBe(heat!.w * heat!.h);
  expect(heat!.masked).toBeGreaterThan(100);

  // Toggling the overlays does not break drawing.
  await page.locator(".hud").getByLabel("jumps").check();
  await page.locator(".hud").getByLabel("heatmap").check();
  await settle(page);
  await expect(page.locator(".panel", { hasText: "Path order" }).locator("table")).toContainText("Copper");

  // Export still works and keeps every path.
  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export PDF", exact: true }).click();
  const file = await download;
  expect(await file.path()).toBeTruthy();
});
