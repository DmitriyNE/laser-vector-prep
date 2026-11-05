import { expect, test, type Page } from "@playwright/test";
import { fixture, importDxf, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

type Tape = { a: [number, number]; b: [number, number] | null; fixed: boolean } | null;
const state = (page: Page) =>
  page.evaluate(() => {
    const lvp = (globalThis as unknown as { lvp: { measuring: boolean; tape: Tape } }).lvp;
    return { measuring: lvp.measuring, tape: JSON.parse(JSON.stringify(lvp.tape)) as Tape };
  });

/** The document point under canvas pixel (x, y), read from the coordinates readout. */
async function mmAt(page: Page, x: number, y: number): Promise<[number, number]> {
  const box = (await page.locator(".preview > canvas").first().boundingBox())!;
  await page.mouse.move(box.x + x, box.y + y);
  const m = /(-?\d+\.\d+), (-?\d+\.\d+) mm/.exec((await page.locator(".preview .coords").textContent()) ?? "");
  return [Number(m![1]), Number(m![2])];
}

test("the tape measures between two clicks, snapping to vertices unless Shift is held", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  const canvas = page.locator(".preview > canvas").first();
  await page.getByRole("button", { name: "Measure" }).click();
  expect((await state(page)).measuring).toBe(true);

  // Where the outline's vertex (1, 11.3) is on screen (top edge, after its corner arc).
  const [x0, y0] = await mmAt(page, 300, 200);
  const [x1] = await mmAt(page, 400, 200);
  const ppm = 100 / (x1 - x0);
  const sx = 300 + (1 - x0) * ppm;
  const sy = 200 - (11.3 - y0) * ppm;

  // A click a few pixels off snaps to the vertex; a Shift-click lands where it is.
  await canvas.click({ position: { x: sx + 3, y: sy + 2 } });
  let { tape } = await state(page);
  expect(tape!.a[0]).toBeCloseTo(1, 5);
  expect(tape!.a[1]).toBeCloseTo(11.3, 5);
  expect(tape!.fixed).toBe(false);
  await canvas.click({ position: { x: sx + 103, y: sy + 2 }, modifiers: ["Shift"] });
  ({ tape } = await state(page));
  expect(tape!.fixed).toBe(true);
  expect(tape!.b![0] - tape!.a[0]).toBeCloseTo(103 / ppm, 1);
  expect(tape!.a[1] - tape!.b![1]).toBeCloseTo(2 / ppm, 1);

  // Esc clears the tape, then turns the tool off; M turns it back on.
  await page.keyboard.press("Escape");
  expect(await state(page)).toEqual({ measuring: true, tape: null });
  await page.keyboard.press("Escape");
  expect((await state(page)).measuring).toBe(false);
  await page.keyboard.press("m");
  expect((await state(page)).measuring).toBe(true);
});
