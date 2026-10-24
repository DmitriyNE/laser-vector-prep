import { expect, test } from "@playwright/test";
import { fixture, importDxf, summary, waitForApp } from "./helpers.js";

const url = "http://localhost:4190/";

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
