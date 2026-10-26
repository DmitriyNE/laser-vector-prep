import { expect, test } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

test("an IPC-2581 board binds the PCB preset through layer functions", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("ipc2581/board.xml"));
  await expect(page.locator(".source .format")).toHaveText(["IPC-2581"]);
  await expect(page.locator(".source li .name")).toHaveText([/^BOTTOM\s*CONDUCTOR,BOTTOM$/, /^Profile\s*PROFILE$/, /^SM_TOP\s*SOLDERMASK,TOP$/, /^TOP\s*CONDUCTOR,TOP$/]);
  await expect(page.getByText("2 matched")).toBeVisible();
  const ev = await summary(page);
  expect(ev.hasErrors).toBe(false);
  expect(ev.bindings.every((b) => b.status === "matched")).toBe(true);
  const copper = ev.layers.find((l) => l.name === "Copper")!;
  expect(copper.outers).toBe(7);
  expect(copper.holes).toBe(1);
  expect(ev.passes[1]!.paths).toBe(1);
});

// As Altium writes IPC-2581 (`.cvg`): copper called SIGNAL, inner layers sharing one
// function, traces grouped in UserSpecial with their line descriptions inline, and pads
// only in padstacks (here a via's, with its pad's shape inline).
const line = (y: number) => `<Line startX="2" startY="${y}" endX="18" endY="${y}"><LineDesc lineEnd="ROUND" lineWidth="0.5"/></Line>`;
const altium = `<?xml version="1.0" encoding="utf-8"?>
<IPC-2581 revision="B" xmlns="http://webstds.ipc.org/2581">
 <Ecad name="b"><CadHeader units="MILLIMETER"/><CadData>
  <Layer name="Top Layer" layerFunction="SIGNAL" side="TOP" polarity="POSITIVE"/>
  <Layer name="Layer 1" layerFunction="SIGNAL" side="INTERNAL" polarity="POSITIVE"/>
  <Layer name="Layer 2" layerFunction="SIGNAL" side="INTERNAL" polarity="POSITIVE"/>
  <Step name="s">
   <Profile><Polygon><PolyBegin x="0" y="0"/><PolyStepSegment x="20" y="0"/><PolyStepSegment x="20" y="20"/><PolyStepSegment x="0" y="20"/><PolyStepSegment x="0" y="0"/></Polygon></Profile>
   <PadStack net="N"><LayerPad layerRef="Top Layer"><Location x="10" y="16"/><Circle diameter="1"/></LayerPad><LayerPad layerRef="Layer 1"><Location x="10" y="16"/><Circle diameter="1"/></LayerPad></PadStack>
   <LayerFeature layerRef="Top Layer"><Set><Features><UserSpecial>${line(2)}</UserSpecial></Features></Set></LayerFeature>
   <LayerFeature layerRef="Layer 1"><Set><Features><UserSpecial>${line(5)}${line(6)}</UserSpecial></Features></Set></LayerFeature>
   <LayerFeature layerRef="Layer 2"><Set><Features><UserSpecial>${line(8)}</UserSpecial></Features></Set>
    <Set><Features><Xform rotation="90"/><Location x="10" y="12"/><RectCenter width="2" height="1"/></Features></Set></LayerFeature>
  </Step>
 </CadData></Ecad>
</IPC-2581>`;

test("inner layers that share a function map onto the copper slot one at a time", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await page.locator('input[type="file"][accept*="dxf"]').setInputFiles({ name: "board.cvg", mimeType: "application/xml", buffer: Buffer.from(altium) });
  await page.waitForFunction(() => {
    const lvp = (globalThis as unknown as { lvp: { sources: unknown[]; evaluating: boolean; evaluation: unknown } }).lvp;
    return lvp.sources.length === 1 && !lvp.evaluating && lvp.evaluation !== null;
  });
  // The preset finds Altium's top copper (SIGNAL,TOP) by itself.
  type Binding = { name: string; status: string; layer?: string };
  const copper = async () => ((await summary(page)).bindings as Binding[]).find((b) => b.name === "Copper")!;
  expect(await copper()).toMatchObject({ status: "matched", layer: "Top Layer" });
  for (const [layer, count] of [["Layer 1", 3], ["Layer 2", 2]] as const) {
    const row = page.locator(".source li").filter({ has: page.locator(".name", { hasText: new RegExp(`^${layer}`) }) });
    const rev = await page.evaluate(() => (globalThis as unknown as { lvp: { evaluation: { revision: number } } }).lvp.evaluation.revision);
    await row.locator("select.map").selectOption("src-copper");
    await page.waitForFunction(
      (r) => {
        const lvp = (globalThis as unknown as { lvp: { evaluating: boolean; evaluation: { revision: number } } }).lvp;
        return !lvp.evaluating && lvp.evaluation.revision > r;
      },
      rev,
    );
    expect(await copper()).toMatchObject({ status: "matched", layer });
    const ev = await summary(page);
    expect(ev.hasErrors).toBe(false);
    // Each trace is copper with its width, and so are Layer 1's via pad and Layer 2's
    // placed rectangle.
    expect(ev.layers.find((l) => l.name === "Copper")!.outers).toBe(count);
  }
});
