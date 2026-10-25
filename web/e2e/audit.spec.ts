import { expect, test, type Page } from "@playwright/test";
import { fixture, importDxf, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

type Audit = { widthMm: number; regionMm2: number; missedMm2: number; spillMm2: number; shortSegments: number };
type Lvp = {
  evaluating: boolean;
  evaluation: { passes: { id: string; audit?: Audit }[] } | null;
  previewAudit: (id: string, spill: boolean) => Promise<{ starts: Uint32Array } | null>;
};

const copperAudit = (page: Page) =>
  page.evaluate(() => {
    const ev = (globalThis as unknown as { lvp: Lvp }).lvp.evaluation!;
    return ev.passes.find((p) => p.id === "pass-copper-fill")!.audit ?? null;
  });

async function settled(page: Page, check: (a: Audit | null) => boolean) {
  await expect
    .poll(async () => {
      const busy = await page.evaluate(() => (globalThis as unknown as { lvp: Lvp }).lvp.evaluating);
      return !busy && check(await copperAudit(page));
    }, { timeout: 30_000 })
    .toBe(true);
}

test("coverage audit: preset numbers, overlays, and the width set per output group", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));

  // The positive LDI preset audits its copper fill at the 20 µm fill pitch.
  const audit = (await copperAudit(page))!;
  expect(audit.widthMm).toBe(0.02);
  expect(audit.regionMm2).toBeGreaterThan(20);
  expect(audit.missedMm2 / audit.regionMm2).toBeLessThan(0.05);
  expect(audit.spillMm2).toBeLessThan(1e-3);

  // The pass's properties show it; the preview overlay has missed-area loops.
  await page.evaluate(() => {
    (globalThis as unknown as { lvp: { selection: unknown } }).lvp.selection = { kind: "pass", id: "pass-copper-fill" };
  });
  await expect(page.locator("p.audit")).toContainText("Audit at 0.02 mm");
  await page.getByLabel("audit", { exact: true }).check();
  const loops = await page.evaluate(async () => {
    const p = await (globalThis as unknown as { lvp: Lvp }).lvp.previewAudit("pass-copper-fill", false);
    return p ? p.starts.length - 1 : 0;
  });
  expect(loops).toBeGreaterThan(0);

  // Twice the pitch: the outer rings sweep beyond the copper.
  const groups = page.locator("section.panel", { has: page.getByRole("heading", { name: /Output groups/ }) });
  await groups.locator("button.name", { hasText: "Copper" }).click();
  const width = page.getByLabel("Audit width (mm)");
  await width.fill("0.04");
  await settled(page, (a) => a !== null && a.widthMm === 0.04 && a.spillMm2 > 0.1);

  // Empty: no audit for the group's passes.
  await width.fill("");
  await settled(page, (a) => a === null);
  await page.evaluate(() => {
    (globalThis as unknown as { lvp: { selection: unknown } }).lvp.selection = { kind: "pass", id: "pass-copper-fill" };
  });
  await expect(page.getByText("No coverage audit")).toBeVisible();
});
