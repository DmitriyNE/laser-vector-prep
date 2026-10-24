import { expect, test } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4190/";

test("an Inkscape-style SVG imports as layers and drives the general-marking preset", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("svg/marking.svg"));
  await expect(page.locator(".source .format")).toHaveText(["SVG"]);
  await expect(page.locator(".source li .name")).toHaveText(["CLIPPED", "FILL", "MARK"]);

  await page.locator('select[title="profile"]').selectOption("preset:builtin-general-marking");
  await page.waitForFunction(() => {
    const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { layers: { name: string }[] } | null } }).lvp;
    return !lvp.evaluating && lvp.evaluation !== null && lvp.evaluation.layers.some((l) => l.name === "Marks");
  });
  const ev = await summary(page);
  expect(ev.hasErrors).toBe(false);
  expect(ev.bindings.every((b) => b.status === "matched")).toBe(true);
  const marks = ev.passes.find((p) => p.name.startsWith("Marks"))!;
  expect(marks.paths).toBe(4);
  expect(marks.closed).toBe(0);
  const fill = ev.passes.find((p) => p.name.startsWith("Shell"))!;
  expect(fill.closed).toBeGreaterThanOrEqual(4);
  expect(fill.open).toBeGreaterThan(20);
  // Declared millimetres; the size shown is the content's extent (x 2..55, y 2..16).
  await expect(page.locator(".source .meta").first()).toContainText("Millimetre · 53.000 × 14.000 mm");
});
