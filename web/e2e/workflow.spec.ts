import { expect, test } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4190/";

test("import, evaluate and export a colour-separated PDF", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await expect(page.locator(".panel h2").first()).toContainText(/sources/i);
  await expect(page.getByText("2 missing")).toBeVisible();

  await importDxf(page, fixture("board-islands.dxf"));
  await expect(page.getByText("2 matched")).toBeVisible();
  await expect(page.locator(".layer > .row").first()).toContainText("10 islands, 1 hole");

  const ev = await summary(page);
  expect(ev.hasErrors).toBe(false);
  expect(ev.bindings.every((b) => b.status === "matched")).toBe(true);
  expect(ev.passes[0]!.paths).toBeGreaterThan(50);
  expect(ev.passes[1]!.paths).toBe(1);

  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export PDF", exact: true }).click();
  const file = await download;
  const stream = await file.createReadStream();
  const chunks: Buffer[] = [];
  for await (const c of stream) chunks.push(c as Buffer);
  const pdf = Buffer.concat(chunks).toString("latin1");
  expect(pdf.startsWith("%PDF-1.4")).toBe(true);
  expect(pdf).toContain("0 0 1 RG");
  expect(pdf).toContain("1 0 0 RG");
  expect(pdf).toContain("0.072 w");
  expect(pdf).not.toMatch(/\n(f|B|f\*)\n/);
});

test("profile slots: an unmapped layer becomes a slot and maps explicitly", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("nested-holes.dxf"));
  const slots = page.locator(".slot");
  await expect(slots).toHaveCount(2);
  await expect(page.locator(".extra .chip")).toContainText("nested-holes.dxf : SHAPES");
  await page.locator(".extra .chip button").click();
  await expect(slots).toHaveCount(3);
  await expect(slots.nth(2)).toContainText("SHAPES");
  await expect(slots.nth(2).locator(".status")).toHaveText(/matched/i);
  // The slot editor opened for the new slot.
  await expect(page.getByText("Matching rules (first match wins)")).toBeVisible();
});

test("two operand groups: envelope minus copper, hatched", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  await page.evaluate(() => {
    const lvp = (globalThis as unknown as { lvp: { update: (f: (p: any) => void) => void } }).lvp;
    lvp.update((p) => {
      const pat = p.workingLayers[0].passes[0].pattern;
      if (pat.kind === "contourFill") pat.pitchMm = 0.2;
      p.workingLayers.push({ id: "env", name: "Box", kind: "envelope", inputs: [{ layer: "wl-copper" }], operands: [], op: "union", envelope: { provider: "boundingBox", marginMm: 0.5 },
        interpretation: { loopRule: "mergeThenNest", strokeWidth: "original" }, passes: [], enabled: true, referenceOnly: true });
      p.workingLayers.push({ id: "neg", name: "Negative", kind: "region", inputs: [{ layer: "env" }], operands: [{ layer: "wl-copper" }], op: "subtraction", envelope: null,
        interpretation: { loopRule: "mergeThenNest", strokeWidth: "original" },
        passes: [{ id: "p-hatch", name: "Hatch", pattern: { kind: "hatch", pitchMm: 0.1, angleDeg: 45, phaseMm: 0, serpentine: true, insetMm: 0 }, group: "grp-copper", enabled: true }],
        enabled: true, referenceOnly: false });
    });
  });
  await page.waitForFunction(() => {
    const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { layers: { name: string }[] } | null } }).lvp;
    return !lvp.evaluating && lvp.evaluation?.layers.some((l) => l.name === "Negative");
  });
  const ev = await summary(page);
  const neg = ev.layers.find((l) => l.name === "Negative")!;
  expect(neg.outers).toBe(2);
  expect(neg.holes).toBe(10);
  const hatch = ev.passes.find((p) => p.name === "Hatch")!;
  expect(hatch.paths).toBeGreaterThan(300);
  expect(hatch.closed).toBe(0);
  const negativeRow = page.locator(".layer > .row").nth(3);
  await expect(negativeRow).toContainText("A − B");
  await expect(negativeRow).toContainText("2 islands, 10 holes");
});
