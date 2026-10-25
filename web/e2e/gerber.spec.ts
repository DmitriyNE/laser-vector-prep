import { expect, test } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

test("Gerber X2 pair binds through layer functions and evaluates like the DXF board", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, [fixture("gerber/board-F_Cu.gbr"), fixture("gerber/board-Edge_Cuts.gbr")]);

  // Both files show as Gerber with their declared functions; both slots are matched.
  await expect(page.locator(".source .format")).toHaveText(["Gerber", "Gerber"]);
  await expect(page.locator(".source .fn").first()).toContainText("Copper,L1,Top");
  await expect(page.getByText("2 matched")).toBeVisible();
  await expect(page.locator(".layer > .row").first()).toContainText("10 islands, 1 hole");

  const ev = await summary(page);
  expect(ev.hasErrors).toBe(false);
  expect(ev.bindings.every((b) => b.status === "matched")).toBe(true);
  expect(ev.passes[1]!.paths).toBe(1);

  // The negative preset uses the drawn profile as the board interior.
  await page.locator('select[title="profile"]').selectOption("preset:builtin-pcb-negative-hatch");
  await page.waitForFunction(() => {
    const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { layers: { name: string }[] } | null } }).lvp;
    return !lvp.evaluating && lvp.evaluation !== null && lvp.evaluation.layers.some((l) => l.name === "Background");
  });
  const neg = await summary(page);
  expect(neg.hasErrors).toBe(false);
  const background = neg.layers.find((l) => l.name === "Background")!;
  expect(background.outers).toBe(2);
  expect(background.holes).toBe(10);
});

test("the features Gerber composites clear polarity and macros into one region", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("gerber/features.gbr"));
  await expect(page.locator(".source .fn").first()).toContainText("Other,Features");
  // Nothing in the default profile claims it: it is listed as unmapped and gets a slot.
  await expect(page.locator(".extra .chip")).toContainText("features.gbr");
  await page.locator(".extra .chip button").click();
  await expect(page.locator(".slot").nth(2).locator(".status")).toHaveText(/matched/i);
  await expect(page.getByText("Matching rules", { exact: true })).toBeVisible();
  await expect(page.locator(".rule select").first()).toHaveValue("function");
});
