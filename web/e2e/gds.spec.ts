import { expect, test } from "@playwright/test";
import { fixture, importDxf, waitForApp } from "./helpers.js";

const url = "http://localhost:4190/";

test("a GDSII stream imports with layer/datatype layers in micrometres", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("gds/cells.gds"));
  await expect(page.locator(".source .format")).toHaveText(["GDSII"]);
  await expect(page.locator(".source li .name")).toHaveText(["1/0", "2/0", "3/0", "4/0"]);
  await expect(page.locator(".source .meta").first()).toContainText("Micrometre · 0.310 × 0.152 mm");
  // Eight pad placements on layer 1/0: 1 + 1 magnified + 6 in the array.
  await expect(page.locator(".source li").first().locator(".counts")).toContainText("8h");
});
