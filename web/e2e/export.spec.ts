import { expect, test } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

test("SVG and DXF exports download and import back as one layer per group", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  const before = await summary(page);
  const copperPaths = before.passes[0]!.paths;

  const svgDownload = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export SVG", exact: true }).click();
  const svg = await svgDownload;
  expect(svg.suggestedFilename()).toMatch(/\.svg$/);
  await importDxf(page, (await svg.path())!);
  const svgSource = page.locator(".source").nth(1);
  await expect(svgSource.locator(".format")).toHaveText("SVG");
  await expect(svgSource.locator("li .name")).toHaveText(["Copper", "Outline"]);
  await expect(svgSource.locator("li").first().locator(".counts")).toContainText(`${copperPaths - before.passes[0]!.open}c`);

  const dxfDownload = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export DXF", exact: true }).click();
  const dxf = await dxfDownload;
  expect(dxf.suggestedFilename()).toMatch(/\.dxf$/);
  await importDxf(page, (await dxf.path())!);
  const dxfSource = page.locator(".source").nth(2);
  await expect(dxfSource.locator(".format")).toHaveText("DXF");
  await expect(dxfSource.locator("li .name")).toHaveText(["Copper", "Outline"]);
  await expect(dxfSource.locator(".meta").first()).toContainText("25.000 × 11.300 mm");
});
