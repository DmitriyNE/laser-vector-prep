import { expect, test } from "@playwright/test";
import { fixture, importDxf, waitForApp } from "./helpers.js";

const url = "http://localhost:4190/";

type Hidden = { sources: string[]; layers: string[]; passes: string[] };
const hidden = (page: import("@playwright/test").Page) =>
  page.evaluate((): Hidden => {
    const lvp = (globalThis as unknown as { lvp: { hiddenSourceLayers: Set<string>; hiddenLayers: Set<string>; hiddenPasses: Set<string> } }).lvp;
    return { sources: [...lvp.hiddenSourceLayers].sort(), layers: [...lvp.hiddenLayers].sort(), passes: [...lvp.hiddenPasses].sort() };
  });

test("eyes toggle, a layer eye takes its passes along, solo isolates and show-all restores", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  const layerRows = page.locator(".layer > .row");
  const copperEye = layerRows.first().locator(".eye");
  const fillEye = page.locator(".layer li.pass .eye").first();
  const sourceEyes = page.locator(".source li .eye");
  await expect(sourceEyes).toHaveCount(2);
  await expect(page.getByRole("button", { name: /show all/ })).toHaveCount(0);

  // The layer's eye hides the region and its pass together, and shows both again.
  await copperEye.click();
  expect(await hidden(page)).toEqual({ sources: [], layers: ["wl-copper"], passes: ["pass-copper-fill"] });
  await expect(copperEye).toHaveAttribute("aria-pressed", "false");
  await expect(fillEye).toHaveAttribute("aria-pressed", "false");
  await copperEye.click();
  expect(await hidden(page)).toEqual({ sources: [], layers: [], passes: [] });

  // A pass eye is independent.
  await fillEye.click();
  expect(await hidden(page)).toEqual({ sources: [], layers: [], passes: ["pass-copper-fill"] });

  // Alt-click on an eye shows only that item; the toolbar offers to show everything again.
  await layerRows.nth(1).locator(".eye").click({ modifiers: ["Alt"] });
  expect(await hidden(page)).toEqual({
    sources: ["0:ROUTE_TOOL_PATH", "0:TOP_LAYER"],
    layers: ["wl-copper"],
    passes: ["pass-copper-fill"],
  });
  const showAll = page.locator(".hud").getByRole("button", { name: /show all/ });
  await expect(showAll).toContainText("4 hidden");

  // Alt-click again on the only visible item restores everything.
  await layerRows.nth(1).locator(".eye").click({ modifiers: ["Alt"] });
  expect(await hidden(page)).toEqual({ sources: [], layers: [], passes: [] });

  // The hover "only" button on a source layer does the same without the modifier.
  const topLayerRow = page.locator(".source li", { hasText: "TOP_LAYER" });
  await topLayerRow.hover();
  await topLayerRow.getByRole("button", { name: "only", exact: true }).click();
  expect((await hidden(page)).sources).toEqual(["0:ROUTE_TOOL_PATH"]);
  expect((await hidden(page)).layers).toEqual(["wl-copper", "wl-outline"]);
  await showAll.click();
  expect(await hidden(page)).toEqual({ sources: [], layers: [], passes: [] });

  // The file eye hides every layer of the file.
  await page.locator(".source .head .eye").click();
  expect((await hidden(page)).sources).toEqual(["0:ROUTE_TOOL_PATH", "0:TOP_LAYER"]);
});
