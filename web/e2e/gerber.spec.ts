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

type HealLvp = {
  evaluating: boolean;
  evaluation: {
    revision: number;
    hasErrors: boolean;
    layers: { id: string; name: string; paths: number; looseEnds: number; diagnostics: { code: string; message: string }[] }[];
    passes: { id: string; paths: number; closed: number }[];
  } | null;
  profile: { workingLayers: { name: string; kind: string; heal?: { toleranceMm: number } | null; referenceOnly: boolean }[] };
  previewLooseEnds(id: string): Promise<{ starts: Uint32Array } | null>;
};

const heal = (page: import("@playwright/test").Page) =>
  page.evaluate(() => JSON.parse(JSON.stringify((globalThis as unknown as { lvp: HealLvp }).lvp.evaluation)) as NonNullable<HealLvp["evaluation"]>);

async function evaluatedAfter(page: import("@playwright/test").Page, rev: number) {
  await page.waitForFunction(
    (r) => {
      const lvp = (globalThis as unknown as { lvp: HealLvp }).lvp;
      return !lvp.evaluating && (lvp.evaluation?.revision ?? 0) > r;
    },
    rev,
    { timeout: 60_000 },
  );
  return heal(page);
}

test("the heater's KiCad profile, drawn in pieces 9 nm apart, is healed into one outline", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, [fixture("designs/heater/heater-F_Cu.gtl"), fixture("designs/heater/heater-Edge_Cuts.gm1")]);
  // The default preset's outline layer heals the eight draws into one closed path.
  let ev = await heal(page);
  expect(ev.hasErrors).toBe(false);
  expect(ev.passes.find((p) => p.id === "pass-outline")).toMatchObject({ paths: 1, closed: 1 });

  let rev = ev.revision;
  await page.locator('select[title="profile"]').selectOption("preset:builtin-boundary-cutting");
  ev = await evaluatedAfter(page, rev);
  expect(ev.hasErrors).toBe(false);
  expect(ev.layers.find((l) => l.id === "wl-outline")).toMatchObject({ paths: 1, looseEnds: 0 });
  expect(ev.passes.find((p) => p.id === "pass-cut")).toMatchObject({ paths: 1, closed: 1 });
  const row = page.locator(".layer").filter({ hasText: "Outline (healed)" });
  await expect(row).toContainText("heal · joins ends ≤ 1 µm");

  // Below the gaps the outline stays open: the cut names the gap and marks its loose ends.
  rev = ev.revision;
  await row.locator(".name").first().click();
  await page.getByLabel("Heal: join ends within (mm)").fill("0.000005");
  ev = await evaluatedAfter(page, rev);
  expect(ev.hasErrors).toBe(true);
  const cut = ev.layers.find((l) => l.id === "wl-cut")!;
  expect(cut.looseEnds).toBe(4);
  expect(cut.diagnostics.find((d) => d.code === "eval.open_linework_in_region")?.message).toContain("9.00 nm apart");
  const ends = await page.evaluate(async () => (await (globalThis as unknown as { lvp: HealLvp }).lvp.previewLooseEnds("wl-cut"))?.starts.length);
  expect(ends).toBe(5);

  // "+ heal" adds a reference-only heal layer at the default tolerance.
  await page.locator(".panel h2 .add", { hasText: "+ heal" }).click();
  const added = await page.evaluate(() => JSON.parse(JSON.stringify((globalThis as unknown as { lvp: HealLvp }).lvp.profile.workingLayers.at(-1))));
  expect(added).toMatchObject({ kind: "linework", heal: { toleranceMm: 0.001 }, referenceOnly: true });
});
