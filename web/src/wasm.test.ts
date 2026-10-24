/// <reference types="node" />
import { readFile } from "node:fs/promises";
import { describe, expect, it, beforeAll } from "vitest";

type Wasm = typeof import("./wasm/lvp_wasm.js");
let { bbox_of, core_version, flatten_arc } = {} as Wasm;

// Requires `just wasm` to have generated src/wasm (the justfile `test` recipe does this).
beforeAll(async () => {
  // The rayon pool helper registers a message listener on `self` at import time; Node has
  // no `self`, so give it an inert event target. The pool itself is never started here.
  (globalThis as { self?: EventTarget }).self ??= new EventTarget();
  const wasm: Wasm = await import("./wasm/lvp_wasm.js");
  ({ bbox_of, core_version, flatten_arc } = wasm);
  const bytes = await readFile(new URL("./wasm/lvp_wasm_bg.wasm", import.meta.url));
  await wasm.default({ module_or_path: bytes });
});

describe("lvp-wasm", () => {
  it("reports the core version", () => {
    expect(core_version()).toMatch(/^\d+\.\d+\.\d+/);
  });

  it("flattens a quarter arc with exact endpoints", () => {
    const pts = flatten_arc(0, 0, 10, 0, Math.PI / 2, 0.01);
    expect(pts).toBeInstanceOf(Float64Array);
    expect(pts.length % 2).toBe(0);
    expect(pts.length).toBeGreaterThanOrEqual(4);
    expect(pts[0]).toBeCloseTo(10, 9);
    expect(pts[1]).toBeCloseTo(0, 9);
    expect(pts[pts.length - 2]).toBeCloseTo(0, 9);
    expect(pts[pts.length - 1]).toBeCloseTo(10, 9);
  });

  it("surfaces core errors as JS exceptions", () => {
    expect(() => flatten_arc(0, 0, -1, 0, 1, 0.01)).toThrow(/radius/);
  });

  // Inputs stay below the parallel threshold: rayon's pool needs browser Workers, not Node.
  it("computes bounding boxes and rejects odd buffers", () => {
    expect(bbox_of(new Float64Array([1, 2, -3, 4, 5, -6]))).toEqual(new Float64Array([-3, -6, 5, 4]));
    expect(bbox_of(new Float64Array([]))).toBeUndefined();
    expect(() => bbox_of(new Float64Array([1]))).toThrow(/even/);
  });
});
