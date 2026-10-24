import { expect, test, type Page } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4190/";

/** Wait until the evaluation of a profile containing a layer named `name` has landed. */
function waitForLayer(page: Page, name: string) {
  return page.waitForFunction(
    (n) => {
      const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { layers: { name: string }[] } | null } }).lvp;
      return !lvp.evaluating && lvp.evaluation !== null && lvp.evaluation.layers.some((l) => l.name === n);
    },
    name,
    { timeout: 60_000 },
  );
}

test("built-in presets switch from the profiles bar and evaluate the board fixture", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  const select = page.locator('select[title="profile"]');
  await expect(select.locator('optgroup[label="Built-in (read-only)"] option')).toHaveCount(4);

  // Negative: board interior minus buffered copper, hatched; outline passed through.
  await select.selectOption("preset:builtin-pcb-negative-hatch");
  await waitForLayer(page, "Background");
  await expect(page.locator(".bar .name")).toContainText("PCB negative");
  let ev = await summary(page);
  expect(ev.hasErrors).toBe(false);
  const background = ev.layers.find((l) => l.name === "Background")!;
  expect(background.outers).toBe(2);
  expect(background.holes).toBe(10);
  const hatch = ev.passes.find((p) => p.name.startsWith("Hatch"))!;
  expect(hatch.paths).toBeGreaterThan(1000);
  expect(hatch.closed).toBe(0);
  expect(ev.passes.find((p) => p.name.startsWith("Outline"))!.paths).toBe(1);

  // Cutting: one closed trace along the rounded 25 x 11.3 mm outline.
  await select.selectOption("preset:builtin-boundary-cutting");
  await waitForLayer(page, "Cut");
  ev = await summary(page);
  expect(ev.hasErrors).toBe(false);
  expect(ev.passes).toHaveLength(1);
  expect(ev.passes[0]!.paths).toBe(1);
  expect(ev.passes[0]!.closed).toBe(1);
  expect(ev.passes[0]!.lengthMm).toBeCloseTo(2 * (25 + 11.3) - 8 + 2 * Math.PI, 3);

  // Marking: the board has no MARK layer, so the required slot is missing and blocks output.
  await select.selectOption("preset:builtin-general-marking");
  await waitForLayer(page, "Marks");
  ev = await summary(page);
  expect(ev.hasErrors).toBe(true);
  expect(ev.bindings.find((b) => b.name === "Marks")!.status).toBe("missing");
  await expect(page.getByText("2 missing")).toBeVisible();
  expect(ev.passes.every((p) => p.paths === 0)).toBe(true);
});
