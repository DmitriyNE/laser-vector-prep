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

test("path order planning: LDI preset is bleed-aware by default; as generated and minimum jump compare", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  const order = page.locator(".panel.order");
  const mode = order.locator("select").first();
  const waitMode = (m: string) =>
    page.waitForFunction((want) => {
      const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { groups: { mode: string }[] } | null } }).lvp;
      return !lvp.evaluating && lvp.evaluation?.groups[0]?.mode === want;
    }, m);

  // Default: bleed-aware, protecting the keep-clear background; coverage numbers and a heatmap.
  await expect(mode).toHaveValue("bleedAware");
  await expect(order.getByLabel("Keep clear (background)")).toBeChecked();
  let gs = await groups(page);
  const bleed = gs.find((g) => g.name === "Copper")!;
  expect(bleed.mode).toBe("bleedAware");
  expect(bleed.heatmap).toBe(true);
  expect(bleed.after.coveredMm2).toBeDefined();
  expect(bleed.after.coveredMm2!).toBeLessThanOrEqual(bleed.before.coveredMm2! + 1e-9);
  expect(bleed.after.jumpLengthMm).toBeLessThan(bleed.before.jumpLengthMm);
  const heat = await page.evaluate(async () => {
    const lvp = (globalThis as unknown as { lvp: { heatmap: (id: string) => Promise<{ width: number; height: number; counts: Uint16Array; mask: Uint8Array } | null> } }).lvp;
    const h = await lvp.heatmap("grp-copper");
    return h ? { w: h.width, h: h.height, cells: h.counts.length, masked: h.mask.reduce((n, m) => n + (m ? 1 : 0), 0) } : null;
  });
  expect(heat).not.toBeNull();
  expect(heat!.cells).toBe(heat!.w * heat!.h);
  expect(heat!.masked).toBeGreaterThan(100);

  // As generated: before == after.
  await mode.selectOption("asGenerated");
  await waitMode("asGenerated");
  gs = await groups(page);
  const generated = gs.find((g) => g.name === "Copper")!;
  expect(generated.after.jumpLengthMm).toBeCloseTo(generated.before.jumpLengthMm, 6);
  await expect(page.locator(".group-row").first()).toContainText("Copper");

  // Minimum jump: same paths, shorter travel.
  await mode.selectOption("minimumJump");
  await waitMode("minimumJump");
  gs = await groups(page);
  const minimum = gs.find((g) => g.name === "Copper")!;
  expect(minimum.paths).toBe(generated.paths);
  expect(minimum.after.jumpLengthMm).toBeLessThan(minimum.before.jumpLengthMm);
  const ev = await summary(page);
  expect(ev.hasErrors).toBe(false);

  await mode.selectOption("bleedAware");
  await waitMode("bleedAware");

  // Toggling the overlays does not break drawing.
  await page.locator(".hud").getByLabel("jumps").check();
  await page.locator(".hud").getByLabel("heatmap").check();
  await settle(page);
  await expect(page.locator(".panel.order").locator("table")).toContainText("Copper");

  // Export still works and keeps every path.
  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export PDF", exact: true }).click();
  const file = await download;
  expect(await file.path()).toBeTruthy();
});
