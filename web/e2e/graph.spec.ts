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
  await expect(node("outputGroup")).toHaveCount(2);
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
  await expect(node("outputGroup")).toHaveCount(3);

  // The preview is still there, with its own state, when switching back.
  await page.getByRole("tab", { name: "Preview" }).click();
  await expect(page.locator(".preview canvas")).toBeVisible();
  await expect(page.locator(".svelte-flow")).toBeHidden();
});

type Lvp = {
  profile: { sourceBindings: { id: string; name: string }[]; workingLayers: { id: string; name: string; inputs: unknown[]; operands: unknown[] }[] };
  canUndo: boolean;
  canRedo: boolean;
};
const profile = (page: import("@playwright/test").Page) =>
  page.evaluate(() => JSON.parse(JSON.stringify((globalThis as unknown as { lvp: Lvp }).lvp.profile)) as Lvp["profile"]);

/** Where an element is once it stops moving (a new node is laid out, then measured). */
async function settled(el: import("@playwright/test").Locator) {
  let last = "";
  await expect
    .poll(async () => {
      const box = JSON.stringify(await el.boundingBox());
      const same = box === last;
      last = box;
      return same;
    })
    .toBe(true);
  return (await el.boundingBox())!;
}

/** Drag a connection from one handle to another with the mouse. */
async function drag(page: import("@playwright/test").Page, from: import("@playwright/test").Locator, to: import("@playwright/test").Locator) {
  const a = await settled(from);
  const b = await settled(to);
  await page.mouse.move(a.x + a.width / 2, a.y + a.height / 2);
  await page.mouse.down();
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 12 });
  await page.mouse.up();
}

test("the graph edits the profile: connect, refuse, delete, undo and redo", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, [fixture("gerber/board-F_Cu.gbr"), fixture("gerber/board-Edge_Cuts.gbr")]);
  await page.getByRole("tab", { name: "Graph" }).click();
  const node = (id: string) => page.locator(`.svelte-flow__node[data-id="${id}"]`);
  const handle = (id: string, handleId: string) => node(id).locator(`.svelte-flow__handle[data-handleid="${handleId}"]`);
  await expect(page.getByRole("button", { name: "Undo" }).first()).toBeDisabled();

  // A new layer, fed from the copper slot by drawing a connection into its input A.
  await page.locator(".tools").getByRole("button", { name: "+ layer" }).click();
  const before = await profile(page);
  const added = before.workingLayers[before.workingLayers.length - 1]!;
  expect(added.inputs).toEqual([]);
  const copperSlot = before.sourceBindings.find((b) => b.name === "Copper")!.id;
  await expect(node(`layer:${added.id}`)).toBeVisible();
  await drag(page, handle(`slot:${copperSlot}`, "out"), handle(`layer:${added.id}`, "a"));
  await expect.poll(async () => (await profile(page)).workingLayers.find((l) => l.id === added.id)!.inputs).toEqual([{ source: copperSlot }]);
  await expect(page.locator(`.svelte-flow__edge[data-id="a:slot:${copperSlot}/out>layer:${added.id}/a"]`)).toHaveCount(1);

  // A connection into itself is refused and changes nothing.
  await drag(page, handle(`layer:${added.id}`, "out"), handle(`layer:${added.id}`, "a"));
  expect((await profile(page)).workingLayers.find((l) => l.id === added.id)!.inputs).toEqual([{ source: copperSlot }]);

  // Delete the operand edge, then the layer, with the keyboard.
  await page.locator(`.svelte-flow__edge[data-id="a:slot:${copperSlot}/out>layer:${added.id}/a"] .svelte-flow__edge-interaction`).click({ force: true });
  await page.keyboard.press("Delete");
  await expect.poll(async () => (await profile(page)).workingLayers.find((l) => l.id === added.id)!.inputs).toEqual([]);
  await node(`layer:${added.id}`).locator(".name").click();
  await page.keyboard.press("Delete");
  await expect(node(`layer:${added.id}`)).toHaveCount(0);
  expect((await profile(page)).workingLayers.some((l) => l.id === added.id)).toBe(false);

  // Undo brings the layer back, then its input; redo removes them again.
  const mod = process.platform === "darwin" ? "Meta" : "Control";
  await page.keyboard.press(`${mod}+z`);
  await expect(node(`layer:${added.id}`)).toHaveCount(1);
  await page.keyboard.press(`${mod}+z`);
  await expect.poll(async () => (await profile(page)).workingLayers.find((l) => l.id === added.id)?.inputs).toEqual([{ source: copperSlot }]);
  await page.getByRole("button", { name: "Redo" }).first().click();
  await page.getByRole("button", { name: "Redo" }).first().click();
  await expect(node(`layer:${added.id}`)).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Redo" }).first()).toBeDisabled();
  // Undoing everything returns to the preset as loaded: nothing left unsaved.
  while (await page.evaluate(() => (globalThis as unknown as { lvp: Lvp }).lvp.canUndo)) await page.keyboard.press(`${mod}+z`);
  expect((await profile(page)).workingLayers.map((l) => l.id)).toEqual(before.workingLayers.slice(0, -1).map((l) => l.id));
  await expect(page.locator(".bar .name")).not.toContainText("●");
});
