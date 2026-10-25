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

/** Click an edge's line where nothing covers it (nodes, other edges, its own ×). */
async function clickEdge(page: import("@playwright/test").Page, id: string) {
  const at = await page.evaluate((id) => {
    const path = document.querySelector(`.svelte-flow__edge[data-id="${id}"] path.svelte-flow__edge-path`) as SVGPathElement;
    const m = path.getScreenCTM()!;
    for (let t = 0.1; t < 0.95; t += 0.05) {
      const p = path.getPointAtLength(path.getTotalLength() * t);
      const at = { x: m.a * p.x + m.c * p.y + m.e, y: m.b * p.x + m.d * p.y + m.f };
      const top = document.elementFromPoint(at.x, at.y);
      if (top?.closest(".svelte-flow__edge")?.getAttribute("data-id") === id) return at;
    }
    return null;
  }, id);
  expect(at, `a visible point on ${id}`).not.toBeNull();
  await page.mouse.click(at!.x, at!.y);
}

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
  const mod = process.platform === "darwin" ? "Meta" : "Control";

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

  // The input goes with the × on its edge.
  const operandEdge = `a:slot:${copperSlot}/out>layer:${added.id}/a`;
  await page.locator(`button[data-edge="${operandEdge}"]`).click();
  await expect.poll(async () => (await profile(page)).workingLayers.find((l) => l.id === added.id)!.inputs).toEqual([]);

  // So does an input whose edge is selected when Delete is pressed: copper in group B of
  // the keep-clear layer, a long edge whose line can be clicked away from its ×. Undo
  // puts it back.
  const keepClear = () => page.evaluate(() => JSON.stringify((globalThis as unknown as { lvp: Lvp }).lvp.profile.workingLayers.find((l) => l.id === "wl-keep-clear")!.operands));
  await clickEdge(page, "b:layer:wl-copper/out>layer:wl-keep-clear/b");
  await expect(page.locator(".svelte-flow__edge.selected")).toHaveCount(1);
  await page.keyboard.press("Delete");
  await expect.poll(keepClear).toBe("[]");
  await page.getByRole("button", { name: "Undo" }).first().click();
  await expect.poll(keepClear).toBe(JSON.stringify([{ layer: "wl-copper" }]));

  // A pass's group cannot just be removed: clicking the edge says how to change it.
  await clickEdge(page, "group:pass:pass-copper-fill/out>group:grp-copper/in");
  await expect(page.locator(".tools .message")).toContainText("drag from the pass to another group");
  await node(`layer:${added.id}`).locator(".name").click();
  await page.keyboard.press("Delete");
  await expect(node(`layer:${added.id}`)).toHaveCount(0);
  expect((await profile(page)).workingLayers.some((l) => l.id === added.id)).toBe(false);

  // An imported layer dragged onto a new slot maps it (by a rule naming the layer); the ×
  // on the mapping removes that rule again.
  await page.locator(".tools").getByRole("button", { name: "+ slot" }).click();
  const slot = (await profile(page)).sourceBindings.at(-1)!.id;
  await expect(node(`slot:${slot}`)).toBeVisible();
  await drag(page, handle("file:0", "row:board-F_Cu.gbr"), handle(`slot:${slot}`, "in"));
  const mapping = `map:file:0/row:board-F_Cu.gbr>slot:${slot}/in`;
  await expect(page.locator(`.svelte-flow__edge[data-id="${mapping}"]`)).toHaveCount(1);
  await page.locator(`button[data-edge="${mapping}"]`).click();
  await expect(page.locator(`.svelte-flow__edge[data-id="${mapping}"]`)).toHaveCount(0);
  await node(`slot:${slot}`).locator(".head").click();
  await page.keyboard.press("Delete");
  await expect(node(`slot:${slot}`)).toHaveCount(0);

  // Undo steps back one edit at a time: the slot's deletion, its unmapping, its mapping
  // (the slot is back), the slot's creation (gone again), the layer's deletion (back), the
  // input's removal (back again; the keep-clear edit was undone already and is no longer
  // in the history). Redo then deletes the layer once more.
  const undo = () => page.keyboard.press(`${mod}+z`);
  await undo();
  await undo();
  await undo();
  await expect(node(`slot:${slot}`)).toHaveCount(1);
  await undo();
  await expect(node(`slot:${slot}`)).toHaveCount(0);
  await undo();
  await expect(node(`layer:${added.id}`)).toHaveCount(1);
  await undo();
  await expect.poll(async () => (await profile(page)).workingLayers.find((l) => l.id === added.id)?.inputs).toEqual([{ source: copperSlot }]);
  await page.getByRole("button", { name: "Redo" }).first().click();
  await page.getByRole("button", { name: "Redo" }).first().click();
  await expect(node(`layer:${added.id}`)).toHaveCount(0);
  // Undoing everything returns to the preset as loaded: nothing left unsaved.
  while (await page.evaluate(() => (globalThis as unknown as { lvp: Lvp }).lvp.canUndo)) await undo();
  expect((await profile(page)).workingLayers.map((l) => l.id)).toEqual(before.workingLayers.slice(0, -1).map((l) => l.id));
  await expect(page.locator(".bar .name")).not.toContainText("●");

  // The preset's copper slot matches the layer by several rules, patterns among them: its
  // × removes every one of them, says which, and undo restores them.
  const copperMap = `map:file:0/row:board-F_Cu.gbr>slot:${copperSlot}/in`;
  await page.locator(`button[data-edge="${copperMap}"]`).click();
  await expect(page.locator(".tools .message")).toContainText("Removed from slot Copper: function Copper,L1,Top");
  await expect(page.locator(`.svelte-flow__edge[data-id="${copperMap}"]`)).toHaveCount(0);
  await expect(node(`slot:${copperSlot}`).locator(".node.missing")).toHaveCount(1);
  await undo();
  await expect(page.locator(`.svelte-flow__edge[data-id="${copperMap}"]`)).toHaveCount(1);
});
