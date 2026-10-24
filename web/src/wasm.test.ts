/// <reference types="node" />
import { readFile } from "node:fs/promises";
import { beforeAll, describe, expect, it } from "vitest";
import type { EvaluationSummary, Profile, SourceSummary } from "./core/types.js";

type Wasm = typeof import("./wasm/lvp_wasm.js");
let wasm: Wasm;

// Requires `just wasm` to have generated src/wasm (the justfile `test` recipe does this).
beforeAll(async () => {
  // The rayon pool helper registers a message listener on `self` at import time; Node has
  // no `self`, so give it an inert event target. The pool itself is never started here.
  (globalThis as { self?: EventTarget }).self ??= new EventTarget();
  wasm = await import("./wasm/lvp_wasm.js");
  const bytes = await readFile(new URL("./wasm/lvp_wasm_bg.wasm", import.meta.url));
  await wasm.default({ module_or_path: bytes });
});

const fixture = () => readFile(new URL("../../fixtures/dxf/board-islands.dxf", import.meta.url));

describe("lvp-wasm project API", () => {
  it("reports the core version and ships presets", () => {
    expect(wasm.core_version()).toMatch(/^\d+\.\d+\.\d+/);
    const presets = JSON.parse(wasm.builtin_presets_json()) as Profile[];
    expect(presets.map((p) => p.profileId)).toEqual([
      "builtin-pcb-positive-ldi",
      "builtin-pcb-negative-hatch",
      "builtin-general-marking",
      "builtin-boundary-cutting",
    ]);
    expect(presets.every((p) => p.schemaVersion === 1)).toBe(true);
    expect(presets[0]!.workingLayers[0]!.passes[0]!.pattern.kind).toBe("contourFill");
  });

  it("imports the board fixture, evaluates and exports a PDF", async () => {
    const summary = JSON.parse(wasm.add_dxf("board-islands.dxf", new Uint8Array(await fixture()))) as SourceSummary;
    expect(summary.units).toBe("Millimetre");
    expect(summary.layers.map((l) => l.name)).toContain("TOP_LAYER");

    // Coarser pitch keeps the test quick.
    const profile = JSON.parse(wasm.profile_json()) as Profile;
    const pattern = profile.workingLayers[0]!.passes[0]!.pattern;
    if (pattern.kind === "contourFill") pattern.pitchMm = 0.2;
    wasm.set_profile_json(JSON.stringify(profile));

    const ev = JSON.parse(wasm.evaluate()) as EvaluationSummary;
    expect(ev.hasErrors).toBe(false);
    expect(ev.bindings.every((b) => b.status === "matched")).toBe(true);
    expect(ev.layers[0]!.outers).toBe(10);
    expect(ev.passes.length).toBe(2);

    const pass = ev.passes[0]!;
    const preview = wasm.preview_pass(pass.id, 0.01)!;
    expect(preview.starts.length).toBe(pass.paths + 1);
    expect(preview.coords.length).toBe(preview.starts[pass.paths]! * 2);
    expect(preview.closed.length).toBe(pass.paths);
    preview.free();
    const jumps = wasm.preview_jumps(pass.id)!;
    expect(jumps.closed.length).toBe(pass.paths - 1);
    jumps.free();

    const pdf = wasm.export_pdf();
    expect(new TextDecoder().decode(pdf.slice(0, 8))).toBe("%PDF-1.4");
    const files = wasm.export_pdf_per_group();
    expect(files.map((f) => f.name)).toEqual(["Copper", "Outline"]);
    files.forEach((f) => f.free());
  });

  it("rejects bad input as exceptions and keeps state", () => {
    expect(() => wasm.add_dxf("x.dxf", new Uint8Array([1, 2, 3]))).toThrow();
    const before = wasm.profile_json();
    expect(() => wasm.set_profile_json('{"schemaVersion": 99}')).toThrow(/newer/);
    expect(wasm.profile_json()).toBe(before);
    expect(wasm.remove_source(0)).toBeUndefined();
    expect(JSON.parse(wasm.sources_json())).toEqual([]);
  });
});
