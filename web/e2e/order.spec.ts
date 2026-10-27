import { expect, test, type Page } from "@playwright/test";
import { fixture, importDxf, waitForApp } from "./helpers.js";

const url = "http://localhost:4180/";

type Lvp = {
  evaluation: { groups: { id: string; name: string; paths: number }[] };
  orderIndex: number;
  previewOrder: (id: string) => Promise<{ paths: { originX: number; originY: number; coords: Float32Array; starts: Uint32Array } } | null>;
};
const fmt = (n: number) => n.toLocaleString("en-US");

/** Document point under the centre of the preview and the scale (the HUD shows the cursor's). */
async function view(page: Page) {
  const box = (await page.locator(".preview > canvas:not(.minimap)").boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  const text = (await page.locator(".preview .coords").textContent())!;
  const m = text.match(/(-?[\d.]+), (-?[\d.]+) mm · ([\d.]+) px\/mm/)!;
  return { box, x: Number(m[1]), y: Number(m[2]), ppm: Number(m[3]) };
}

test("the order inspector steps through a group's marking order", async ({ page }) => {
  await page.goto(url);
  await waitForApp(page);
  await importDxf(page, fixture("board-islands.dxf"));
  const groups = await page.evaluate(() => JSON.parse(JSON.stringify((globalThis as unknown as { lvp: Lvp }).lvp.evaluation.groups)) as Lvp["evaluation"]["groups"]);
  const copper = groups.find((g) => g.name === "Copper")!;
  const outline = groups.find((g) => g.name === "Outline")!;
  const n = copper.paths;
  expect(n).toBeGreaterThan(20);

  const timeline = page.locator(".timeline");
  await expect(timeline).toBeHidden();
  await page.locator(".hud").getByLabel("order").check();
  await expect(timeline).toBeVisible();
  const slider = timeline.getByRole("slider", { name: "current path" });
  const count = timeline.locator(".count");
  // The first group with paths opens fully marked.
  await expect(timeline.locator("select.group")).toHaveValue(copper.id);
  await expect(slider).toHaveAttribute("max", String(n - 1));
  await expect(count).toHaveText(`${fmt(n)} / ${fmt(n)}`);
  await expect(timeline.getByRole("button", { name: "next path" })).toBeDisabled();

  // Step back, jump to the start with the keyboard, step forward.
  await timeline.getByRole("button", { name: "previous path" }).click();
  await expect(count).toHaveText(`${fmt(n - 1)} / ${fmt(n)}`);
  await slider.focus();
  await page.keyboard.press("Home");
  await expect(count).toHaveText(`1 / ${fmt(n)}`);
  await expect(timeline.getByRole("button", { name: "previous path" })).toBeDisabled();
  await page.keyboard.press("ArrowRight");
  await expect(count).toHaveText(`2 / ${fmt(n)}`);
  await expect(timeline.locator(".readout")).toContainText(/(loop|open) [\d.]+ mm · jump [\d.]+ mm/);

  // Play advances on its own; pause stops it.
  await timeline.getByRole("button", { name: "play" }).click();
  await page.waitForFunction(() => (globalThis as unknown as { lvp: Lvp }).lvp.orderIndex > 10);
  await timeline.getByRole("button", { name: "pause" }).click();
  const paused = await page.evaluate(() => (globalThis as unknown as { lvp: Lvp }).lvp.orderIndex);
  await page.waitForTimeout(200);
  expect(await page.evaluate(() => (globalThis as unknown as { lvp: Lvp }).lvp.orderIndex)).toBe(paused);

  // A click on a path makes it the current one: the first point of a path halfway along.
  const k = Math.floor(n / 2);
  const at = await page.evaluate(async ([id, k]) => {
    const o = (await (globalThis as unknown as { lvp: Lvp }).lvp.previewOrder(id as string))!;
    const v = o.paths.starts[k as number]!;
    return [o.paths.originX + o.paths.coords[2 * v]!, o.paths.originY + o.paths.coords[2 * v + 1]!];
  }, [copper.id, k] as const);
  const screen = (v: Awaited<ReturnType<typeof view>>) => [v.box.x + v.box.width / 2 + (at[0]! - v.x) * v.ppm, v.box.y + v.box.height / 2 - (at[1]! - v.y) * v.ppm] as const;
  // Zoom in on it first (the wheel zooms about the pointer), so neighbouring paths are well apart.
  const fitted = await view(page);
  await page.mouse.move(...screen(fitted));
  await page.mouse.wheel(0, -1200);
  await expect.poll(async () => (await view(page)).ppm).toBeGreaterThan(fitted.ppm * 4);
  const [px, py] = screen(await view(page));
  await page.mouse.move(px, py);
  await expect(page.locator(".hover-label")).toHaveText(`#${fmt(k + 1)}`);
  await page.mouse.click(px, py);
  await expect(count).toHaveText(`${fmt(k + 1)} / ${fmt(n)}`);

  // Another group is stepped through on its own.
  await timeline.locator("select.group").selectOption(outline.id);
  await expect(count).toHaveText(`${fmt(outline.paths)} / ${fmt(outline.paths)}`);
  // The order panel opens a group in the inspector too.
  await page.locator(".panel.order").getByRole("button", { name: "Copper" }).click();
  await expect(timeline.locator("select.group")).toHaveValue(copper.id);
  await expect(count).toHaveText(`${fmt(n)} / ${fmt(n)}`);

  // Off again: the timeline goes away.
  await page.locator(".hud").getByLabel("order").uncheck();
  await expect(timeline).toBeHidden();
});
