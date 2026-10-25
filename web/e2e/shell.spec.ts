import { expect, test } from "@playwright/test";
import { fixture, importDxf, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";
const board = [fixture("gerber/board-F_Cu.gbr"), fixture("gerber/board-Edge_Cuts.gbr")];

// Closing, reloading and leaving all go through `beforeunload`; Playwright reports its
// prompt for navigations (not for page.close), so the tests reload.
test("leaving the page asks first once there is something to lose", async ({ page }) => {
  let asked = 0;
  page.on("dialog", (d) => {
    if (d.type() === "beforeunload") asked++;
    void d.dismiss(); // stay
  });
  await page.goto(url);
  await waitForApp(page);
  await page.getByRole("tab", { name: "Graph" }).click(); // browsers only ask after an interaction
  // Nothing loaded: the page reloads without a question.
  await page.reload();
  await waitForApp(page);
  expect(asked).toBe(0);

  // Files imported: leaving asks, and staying keeps everything.
  await importDxf(page, board);
  await page.getByRole("tab", { name: "Graph" }).click();
  await page.reload({ timeout: 3000 }).catch(() => undefined);
  expect(asked).toBe(1);
  expect(await page.evaluate(() => (globalThis as unknown as { lvp: { sources: unknown[] } }).lvp.sources.length)).toBe(2);
});

test("the graph tab shows progress and Cancel, double-click fits, and the selector marks unsaved edits", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, board);
  const selected = page.locator('select[title="profile"] option:checked');
  await expect(selected).toHaveText("PCB positive LDI (contour fill)");
  await expect(page.locator(".bar .name")).toHaveCount(0); // no second copy of the name

  await page.getByRole("tab", { name: "Graph" }).click();
  await expect(page.locator(".svelte-flow__node")).not.toHaveCount(0);
  const scale = () => page.locator(".svelte-flow__viewport").evaluate((el) => new DOMMatrix(getComputedStyle(el).transform).a);
  const fitted = await scale();
  await page.locator(".svelte-flow__controls-zoomin").click();
  await page.locator(".svelte-flow__controls-zoomin").click();
  await expect.poll(scale).toBeGreaterThan(fitted * 1.3);
  const pane = (await page.locator(".svelte-flow").boundingBox())!;
  await page.mouse.dblclick(pane.x + pane.width / 2, pane.y + pane.height - 60);
  await expect.poll(scale).toBeCloseTo(fitted, 2);

  // A long evaluation shows progress and Cancel on this tab too; the edit marks the profile.
  await page.evaluate(() => {
    const lvp = (globalThis as unknown as { lvp: { update: (f: (p: { workingLayers: { passes: { pattern: { kind: string; pitchMm?: number } }[] }[] }) => void) => void } }).lvp;
    lvp.update((p) => {
      for (const l of p.workingLayers) for (const pass of l.passes) if (pass.pattern.kind === "contourFill") pass.pattern.pitchMm = 0.001;
    });
  });
  await expect(selected).toHaveText("PCB positive LDI (contour fill) ● unsaved");
  const cancel = page.getByRole("button", { name: "Cancel" });
  await expect(cancel).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".busy progress")).toBeVisible();
  await cancel.click();
  await expect(cancel).toBeHidden();
});
