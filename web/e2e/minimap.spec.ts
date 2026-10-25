import { expect, test, type Page } from "@playwright/test";
import { fixture, importDxf, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

/** The document point under the centre of the preview (the HUD shows the cursor's). */
async function centre(page: Page) {
  const box = (await page.locator(".preview > canvas:not(.minimap)").boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  const text = (await page.locator(".preview .coords").textContent())!;
  const m = text.match(/(-?[\d.]+), (-?[\d.]+) mm · ([\d.]+) px\/mm/)!;
  return { x: Number(m[1]), y: Number(m[2]), ppm: Number(m[3]) };
}

test("the preview's minimap shows the drawing and pans and zooms the view", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  const minimap = page.locator("canvas.minimap");
  await expect(minimap).toBeHidden(); // nothing to show yet
  await importDxf(page, [fixture("gerber/board-F_Cu.gbr"), fixture("gerber/board-Edge_Cuts.gbr")]);
  await expect(minimap).toBeVisible();
  const box = (await minimap.boundingBox())!;
  expect([Math.round(box.width), Math.round(box.height)]).toEqual([200, 150]);
  // It drew the board: plenty of painted pixels.
  const painted = await minimap.evaluate((c: HTMLCanvasElement) => {
    const data = c.getContext("2d")!.getImageData(0, 0, c.width, c.height).data;
    let n = 0;
    for (let i = 3; i < data.length; i += 4) if (data[i]! > 0) n++;
    return n / (c.width * c.height);
  });
  expect(painted).toBeGreaterThan(0.2);

  // Dragging the view on the minimap moves the main view the same way.
  const before = await centre(page);
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 30, box.y + box.height / 2 + 10, { steps: 6 });
  await page.mouse.up();
  const panned = await centre(page);
  expect(panned.x).toBeGreaterThan(before.x + 1);
  expect(panned.y).toBeLessThan(before.y);
  expect(panned.ppm).toBe(before.ppm);

  // The wheel over the minimap zooms the main view.
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, -300);
  await expect.poll(async () => (await centre(page)).ppm).toBeGreaterThan(panned.ppm * 1.2);
});
