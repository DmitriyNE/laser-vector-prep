import { expect, test } from "@playwright/test";
import { fixture, importDxf, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

type Layer = { name: string; areaMm2: number | null };
const layerArea = (page: import("@playwright/test").Page, name: string) =>
  page.evaluate((n) => {
    const lvp = globalThis as unknown as { lvp: { evaluation: { layers: Layer[] } } };
    return lvp.lvp.evaluation.layers.find((l) => l.name === n)?.areaMm2 ?? null;
  }, name);

test("mitred and bevelled buffer joins change the envelope as the kernel computes them", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  await page.locator('select[title="profile"]').selectOption("preset:builtin-pcb-negative-hatch");
  const envName = "Copper + 50 um clearance";
  await page.waitForFunction((n) => {
    const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { layers: { name: string }[] } | null } }).lvp;
    return !lvp.evaluating && !!lvp.evaluation?.layers.some((l) => l.name === n);
  }, envName);
  const round = (await layerArea(page, envName))!;

  const envRow = page.locator(".layer > .row").filter({ has: page.locator(".title", { hasText: new RegExp(`^${envName.replace(/[+]/g, "\\+")}$`) }) });
  await envRow.locator(".name").click();
  const joins = page.getByLabel("Corner joins");
  await joins.selectOption("miter");
  await expect(page.getByLabel(/Miter limit/)).toHaveValue("4");
  await page.waitForFunction(
    ([n, r]) => {
      const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { layers: Layer[] } } }).lvp;
      const a = lvp.evaluation.layers.find((l) => l.name === n)?.areaMm2;
      return !lvp.evaluating && a !== undefined && a !== null && Math.abs(a - (r as number)) > 1e-6;
    },
    [envName, round] as const,
  );
  const miter = (await layerArea(page, envName))!;
  // Rectangular pads grow square corners: more area than round joins.
  expect(miter).toBeGreaterThan(round);
  await expect(envRow).toContainText("miter");

  await joins.selectOption("bevel");
  await page.waitForFunction(
    ([n, m]) => {
      const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { layers: Layer[] } } }).lvp;
      const a = lvp.evaluation.layers.find((l) => l.name === n)?.areaMm2;
      return !lvp.evaluating && a !== undefined && a !== null && Math.abs(a - (m as number)) > 1e-6;
    },
    [envName, miter] as const,
  );
  const bevel = (await layerArea(page, envName))!;
  expect(bevel).toBeLessThan(round);
});
