import { expect, type Page } from "@playwright/test";
import { fileURLToPath } from "node:url";

/** Absolute path of a fixture: a bare name is a DXF, `gerber/x.gbr` style paths are used as given. */
export const fixture = (name: string) =>
  fileURLToPath(new URL(`../../fixtures/${name.includes("/") ? name : `dxf/${name}`}`, import.meta.url));

/** Wait for the worker to be up (store exposed as globalThis.lvp by App.svelte). */
export async function waitForApp(page: Page) {
  await page.waitForFunction(() => (globalThis as { lvp?: { ready: boolean } }).lvp?.ready === true, null, { timeout: 60_000 });
}

export async function importDxf(page: Page, file: string | string[]) {
  const files = Array.isArray(file) ? file : [file];
  const before = await page.evaluate(() => (globalThis as unknown as { lvp: { sources: unknown[] } }).lvp.sources.length);
  await page.locator('input[type="file"][accept*="dxf"]').setInputFiles(files);
  await page.waitForFunction(
    (n) => {
      const lvp = (globalThis as unknown as { lvp: { sources: unknown[]; evaluating: boolean; evaluation: { revision: number } | null } }).lvp;
      return lvp.sources.length >= n && !lvp.evaluating && lvp.evaluation !== null;
    },
    before + files.length,
    { timeout: 60_000 },
  );
  await expect(page.locator(".source")).toHaveCount(before + files.length);
}

/** Snapshot of the evaluation summary from the store. */
export function summary(page: Page) {
  return page.evaluate(() => {
    const lvp = (globalThis as unknown as { lvp: { evaluation: unknown } }).lvp;
    return JSON.parse(JSON.stringify(lvp.evaluation)) as {
      hasErrors: boolean;
      bindings: { name: string; status: string }[];
      layers: { name: string; kind: string; outers: number; holes: number; paths: number }[];
      passes: { name: string; paths: number; closed: number; open: number; lengthMm: number }[];
    };
  });
}
