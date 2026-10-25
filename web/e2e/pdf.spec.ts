import { expect, test } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

test("a PDF exported by the app imports back as one colour layer per group", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  const before = await summary(page);

  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export PDF", exact: true }).click();
  const file = await download;
  const path = await file.path();
  expect(path).toBeTruthy();

  // Import the exported PDF next to the DXF.
  await importDxf(page, path!);
  await expect(page.locator(".source .format")).toHaveText(["DXF", "PDF"]);
  const pdfSource = page.locator(".source").nth(1);
  await expect(pdfSource.locator("li .name")).toHaveText(["#0000ff", "#ff0000"]);
  await expect(pdfSource.locator(".meta").first()).toContainText("Point");
  // Blue carries the copper fill's closed rings and open gap-fill centrelines, one to one.
  const copper = before.passes[0]!;
  await expect(pdfSource.locator("li").first().locator(".counts")).toContainText(`${copper.closed}c ${copper.open}o`);
});

test("the features PDF fixture lists its optional-content layer and colour layers with diagnostics", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("pdf/features.pdf"));
  await expect(page.locator(".source li .name")).toHaveText(["#000000", "#009900", "#808080", "#ff0000", "Outline"]);
  const ev = await summary(page);
  // Text in the PDF is an error-level import diagnostic, surfaced without blocking the DXF-less profile evaluation.
  const diags = await page.evaluate(() => (globalThis as unknown as { lvp: { sources: { diagnostics: { code: string }[] }[] } }).lvp.sources[0]!.diagnostics.map((d) => d.code));
  expect(diags).toContain("pdf.text");
  expect(diags).toContain("pdf.image");
  expect(ev).toBeTruthy();
});
