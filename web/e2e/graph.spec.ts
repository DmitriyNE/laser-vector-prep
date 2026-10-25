import { expect, test } from "@playwright/test";
import { fixture, importDxf, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

type Store = { selection: { kind: string; id?: string }; hiddenPasses: Set<string> };
const store = (page: import("@playwright/test").Page) =>
  page.evaluate(() => {
    const lvp = (globalThis as unknown as { lvp: Store }).lvp;
    return { selection: lvp.selection, hiddenPasses: [...lvp.hiddenPasses] };
  });

test("the graph tab shows files, slots, layers, passes and groups with their status and selects into the panels", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, [fixture("gerber/board-F_Cu.gbr"), fixture("gerber/board-Edge_Cuts.gbr")]);
  await page.getByRole("tab", { name: "Graph" }).click();

  const node = (kind: string) => page.locator(`.svelte-flow__node-${kind}`);
  await expect(node("file")).toHaveCount(2);
  await expect(node("slot")).toHaveCount(2);
  await expect(node("layer")).toHaveCount(4);
  await expect(node("pass")).toHaveCount(2);
  await expect(node("output")).toHaveCount(2);
  // Two matches, five operand inputs (two slots, outline → box, box and copper → keep
  // clear), two passes and their two groups.
  await expect(page.locator(".svelte-flow__edge")).toHaveCount(11);
  await expect(page.locator(".svelte-flow__edge.edge-map")).toHaveCount(2);
  await expect(page.locator(".svelte-flow__edge.edge-b")).toHaveCount(1);

  // Live status: both slots matched, every evaluated layer clean.
  await expect(node("slot").locator(".node.matched")).toHaveCount(2);
  await expect(node("layer").locator(".node.ok")).toHaveCount(4);
  await expect(node("pass").first()).toContainText("paths");

  // A click selects into the properties panel; an eye hides a pass in the preview.
  await node("layer").filter({ hasText: "Copper" }).locator(".name").click();
  expect((await store(page)).selection).toEqual({ kind: "layer", id: "wl-copper" });
  await expect(page.locator(".panel").filter({ hasText: "Properties" }).getByRole("textbox").first()).toHaveValue("Copper");
  const fill = node("pass").filter({ hasText: "Contour fill" });
  await fill.locator(".eye").click();
  expect((await store(page)).hiddenPasses).toEqual(["pass-copper-fill"]);

  // An edit shows up in the graph without re-importing.
  await page.evaluate(() => {
    const lvp = (globalThis as unknown as { lvp: { update: (f: (p: { outputGroups: { id: string; name: string }[] }) => void) => void } }).lvp;
    lvp.update((p) => {
      p.outputGroups.push({ id: "grp-extra", name: "Extra", color: [0, 128, 0], export: true, visible: true } as never);
    });
  });
  await expect(node("output")).toHaveCount(3);

  // The preview is still there, with its own state, when switching back.
  await page.getByRole("tab", { name: "Preview" }).click();
  await expect(page.locator(".preview canvas")).toBeVisible();
  await expect(page.locator(".svelte-flow")).toBeHidden();
});
