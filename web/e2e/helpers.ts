import { expect, type Page } from "@playwright/test";
import { fileURLToPath } from "node:url";

export const fixture = (name: string) => fileURLToPath(new URL(`../../fixtures/dxf/${name}`, import.meta.url));

/** Wait for the worker to be up (store exposed as globalThis.lvp by App.svelte). */
export async function waitForApp(page: Page) {
  await page.waitForFunction(() => (globalThis as { lvp?: { ready: boolean } }).lvp?.ready === true, null, { timeout: 60_000 });
}

export async function importDxf(page: Page, file: string) {
  const before = await page.evaluate(() => (globalThis as unknown as { lvp: { sources: unknown[] } }).lvp.sources.length);
  await page.locator('input[type="file"][accept*="dxf"]').setInputFiles(file);
  await page.waitForFunction(
    (n) => {
      const lvp = (globalThis as unknown as { lvp: { sources: unknown[]; evaluating: boolean; evaluation: { revision: number } | null } }).lvp;
      return lvp.sources.length > n && !lvp.evaluating && lvp.evaluation !== null;
    },
    before,
    { timeout: 60_000 },
  );
  await expect(page.locator(".source")).toHaveCount(before + 1);
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
