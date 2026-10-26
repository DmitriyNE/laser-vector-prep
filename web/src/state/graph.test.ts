import { describe, expect, it } from "vitest";
import type { EvaluationSummary, Profile, SourceSummary, WorkingLayer } from "../core/types.js";
import { buildGraph, connectionEdit, dependencies, edgeHint, edgeRemoval, globMatch, layout, nodeId, nodeRemoval, rowHandle, structureKey } from "./graph.js";

function layer(id: string, extra: Partial<WorkingLayer>): WorkingLayer {
  return {
    id,
    name: id,
    kind: "region",
    inputs: [],
    operands: [],
    op: "union",
    envelope: null,
    interpretation: { loopRule: "mergeThenNest", strokeWidth: "original" },
    passes: [],
    enabled: true,
    referenceOnly: false,
    ...extra,
  };
}

const profile: Profile = {
  schemaVersion: 3,
  profileId: "t",
  name: "t",
  sourceBindings: [
    { id: "src-cu", name: "Copper", required: true, rules: [] },
    { id: "src-edge", name: "Outline", required: true, rules: [] },
    { id: "src-spare", name: "Spare", required: false, rules: [] },
  ],
  workingLayers: [
    layer("wl-cu", { inputs: [{ source: "src-cu" }], passes: [{ id: "p-fill", name: "Fill", group: "g-a", enabled: true, pattern: { kind: "contourFill", pitchMm: 0.02, gapFill: true } }] }),
    layer("wl-edge", { inputs: [{ source: "src-edge" }] }),
    layer("wl-box", { kind: "envelope", envelope: { provider: "boundingBox", marginMm: 1 }, inputs: [{ layer: "wl-edge" }], referenceOnly: true }),
    layer("wl-neg", {
      op: "subtraction",
      inputs: [{ layer: "wl-box" }],
      operands: [{ layer: "wl-cu" }],
      passes: [{ id: "p-hatch", name: "Hatch", group: "g-a", enabled: true, pattern: { kind: "hatch", pitchMm: 0.05, angleDeg: 45, phaseMm: 0, serpentine: true, insetMm: 0 } }],
    }),
  ],
  outputGroups: [
    { id: "g-a", name: "A", color: [0, 0, 255], export: true, visible: true },
    { id: "g-unused", name: "Unused", color: [255, 0, 0], export: true, visible: true },
  ],
  export: { strokeWidthMm: 0.0254, marginMm: 5, separateFiles: false, flattenToleranceMm: 0.001 },
  optimizer: { mode: "minimumJump", footprintMm: 0.1, bleedWeight: 1, protected: [], cellMm: 0.05, maxSweeps: 20 },
};

const layerSummary = (name: string) => ({ name, color: null, function: null, closedPaths: 1, openPaths: 0, hatches: 0, boundsMm: null });
const sources: SourceSummary[] = [
  {
    index: 0,
    name: "board.dxf",
    format: "dxf",
    version: null,
    units: null,
    assumedUnits: "mm",
    boundsMm: null,
    layers: [layerSummary("COPPER"), layerSummary("EDGE"), layerSummary("EDGE2"), { ...layerSummary("EMPTY"), closedPaths: 0 }],
    diagnostics: [],
  },
];

const evaluation: EvaluationSummary = {
  revision: 1,
  boundsMm: null,
  bindings: [
    { id: "src-cu", name: "Copper", required: true, status: "matched", file: "board.dxf", layer: "COPPER", candidates: [] },
    { id: "src-edge", name: "Outline", required: true, status: "ambiguous", file: null, layer: null, candidates: [["board.dxf", "EDGE"], ["board.dxf", "EDGE2"]] },
    { id: "src-spare", name: "Spare", required: false, status: "missing", file: null, layer: null, candidates: [] },
  ],
  unboundLayers: [],
  layers: [
    { id: "wl-cu", name: "wl-cu", kind: "region", outers: 3, holes: 1, paths: 0, areaMm2: 10, boundsMm: null, cached: false, diagnostics: [] },
    { id: "wl-neg", name: "wl-neg", kind: "region", outers: 1, holes: 3, paths: 0, areaMm2: 5, boundsMm: null, cached: false, diagnostics: [{ severity: "warning", code: "x", message: "", context: null }] },
  ],
  passes: [],
  groups: [],
  diagnostics: [{ severity: "error", code: "eval.binding_ambiguous", message: "", context: "src-edge" }],
  hasErrors: true,
};

describe("model graph", () => {
  const { nodes, edges } = buildGraph(profile, sources, evaluation);
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const kinds = (k: string) => edges.filter((e) => e.kind === k).map((e) => `${e.source}>${e.target}`);

  it("has a node per file, slot, layer, pass and group, and rows for layers with content", () => {
    expect(nodes.map((n) => n.id).sort()).toEqual(
      [
        "file:0",
        "slot:src-cu",
        "slot:src-edge",
        "slot:src-spare",
        "layer:wl-cu",
        "layer:wl-edge",
        "layer:wl-box",
        "layer:wl-neg",
        "pass:p-fill",
        "pass:p-hatch",
        "group:g-a",
        "group:g-unused",
      ].sort(),
    );
    const file = byId.get("file:0")!.data;
    expect(file.kind === "file" && file.rows.map((r) => [r.name, r.state])).toEqual([
      ["COPPER", "bound"],
      ["EDGE", "ambiguous"],
      ["EDGE2", "ambiguous"],
    ]);
  });

  it("connects matches, candidates, both operand groups, passes and groups", () => {
    expect(kinds("map")).toEqual(["file:0>slot:src-cu"]);
    expect(edges.find((e) => e.kind === "map")!.sourceHandle).toBe(rowHandle("COPPER"));
    expect(kinds("candidate")).toEqual(["file:0>slot:src-edge", "file:0>slot:src-edge"]);
    expect(kinds("a")).toEqual(["slot:src-cu>layer:wl-cu", "slot:src-edge>layer:wl-edge", "layer:wl-edge>layer:wl-box", "layer:wl-box>layer:wl-neg"]);
    expect(kinds("b")).toEqual(["layer:wl-cu>layer:wl-neg"]);
    expect(kinds("pass")).toEqual(["layer:wl-cu>pass:p-fill", "layer:wl-neg>pass:p-hatch"]);
    expect(kinds("group")).toEqual(["pass:p-fill>group:g-a", "pass:p-hatch>group:g-a"]);
    expect(new Set(edges.map((e) => e.id)).size).toBe(edges.length);
  });

  it("reports status from the evaluation", () => {
    const status = (id: string) => {
      const d = byId.get(id)!.data;
      return "status" in d ? d.status : undefined;
    };
    expect(status(nodeId.layer("wl-cu"))).toBe("ok");
    expect(status(nodeId.layer("wl-neg"))).toBe("warning");
    expect(status(nodeId.layer("wl-box"))).toBe("idle");
    const slot = byId.get(nodeId.slot("src-spare"))!.data;
    expect(slot.kind === "slot" && slot.state).toBe("missing");
    const neg = byId.get(nodeId.layer("wl-neg"))!.data;
    expect(neg.kind === "layer" && neg.twoGroups).toBe(true);
    const cu = byId.get(nodeId.layer("wl-cu"))!.data;
    expect(cu.kind === "layer" && cu.twoGroups).toBe(false);
  });

  it("lays out columns left to right without overlaps", () => {
    const pos = layout(nodes, edges);
    const x = (id: string) => pos.get(id)!.x;
    expect(x("file:0")).toBeLessThan(x("slot:src-cu"));
    expect(x("slot:src-cu")).toBeLessThan(x("layer:wl-cu"));
    expect(x("layer:wl-edge")).toBeLessThan(x("layer:wl-box"));
    expect(x("layer:wl-box")).toBeLessThan(x("layer:wl-neg"));
    expect(x("layer:wl-neg")).toBeLessThan(x("pass:p-hatch"));
    expect(x("pass:p-hatch")).toBeLessThan(x("group:g-a"));
    // Each kind keeps its column: the unused group with the used one, every pass together.
    expect(x("group:g-unused")).toBe(x("group:g-a"));
    expect(x("pass:p-fill")).toBe(x("pass:p-hatch"));
    expect(x("slot:src-spare")).toBe(x("slot:src-cu"));
    for (const a of nodes) {
      for (const b of nodes) {
        if (a.id >= b.id) continue;
        const pa = pos.get(a.id)!;
        const pb = pos.get(b.id)!;
        const apart = pa.x + a.width <= pb.x || pb.x + b.width <= pa.x || pa.y + a.height <= pb.y || pb.y + b.height <= pa.y;
        expect(apart, `${a.id} overlaps ${b.id}`).toBe(true);
      }
    }
  });

  it("changes its structure key with structure only", () => {
    const again = buildGraph(profile, sources, { ...evaluation, layers: [] });
    expect(structureKey(again.nodes, again.edges)).toBe(structureKey(nodes, edges));
    const fewer = buildGraph({ ...profile, outputGroups: profile.outputGroups.slice(0, 1) }, sources, evaluation);
    expect(structureKey(fewer.nodes, fewer.edges)).not.toBe(structureKey(nodes, edges));
  });
});

describe("editing on the graph", () => {
  const c = (source: string, sourceHandle: string, target: string, targetHandle: string) => connectionEdit(profile, sources, { source, sourceHandle, target, targetHandle });

  it("turns connections into profile edits", () => {
    expect(c("file:0", rowHandle("EDGE2"), "slot:src-edge", "in")).toEqual({ kind: "map", slot: "src-edge", file: "board.dxf", layer: "EDGE2" });
    expect(c("slot:src-spare", "out", "layer:wl-neg", "b")).toEqual({ kind: "operand", from: { source: "src-spare" }, layer: "wl-neg", group: "b" });
    expect(c("layer:wl-edge", "out", "layer:wl-cu", "a")).toEqual({ kind: "operand", from: { layer: "wl-edge" }, layer: "wl-cu", group: "a" });
    expect(c("layer:wl-neg", "out", "pass:p-fill", "in")).toEqual({ kind: "movePass", pass: "p-fill", layer: "wl-neg" });
    expect(c("pass:p-fill", "out", "group:g-unused", "in")).toEqual({ kind: "passGroup", pass: "p-fill", group: "g-unused" });
  });

  it("refuses connections that make no sense", () => {
    expect(c("layer:wl-neg", "out", "layer:wl-edge", "a")).toBe("that would make a cycle");
    expect(c("layer:wl-cu", "out", "layer:wl-cu", "a")).toBe("a layer cannot read itself");
    expect(c("slot:src-cu", "out", "layer:wl-cu", "a")).toBe("already an input");
    expect(c("slot:src-spare", "out", "layer:wl-cu", "b")).toBe("this layer has one input group");
    expect(c("layer:wl-cu", "out", "pass:p-fill", "in")).toBe("the pass is already on this layer");
    expect(c("pass:p-fill", "out", "group:g-a", "in")).toBe("the pass already marks in this group");
    expect(typeof c("slot:src-cu", "out", "group:g-a", "in")).toBe("string");
    expect(dependencies(profile, "wl-neg")).toEqual(new Set(["wl-box", "wl-edge", "wl-cu"]));
  });

  it("deletes operand edges, layers, passes, files, and only unused slots and groups", () => {
    const { nodes, edges } = buildGraph(profile, sources, evaluation);
    const data = (id: string) => nodes.find((n) => n.id === id)!.data;
    const b = edges.find((e) => e.kind === "b")!;
    expect(edgeRemoval(b, profile, sources)).toEqual({ kind: "operand", from: { layer: "wl-cu" }, layer: "wl-neg", group: "b" });
    expect(edgeRemoval(edges.find((e) => e.kind === "pass")!, profile, sources)).toBeNull();
    expect(edgeHint(edges.find((e) => e.kind === "group")!)).toMatch(/drag from the pass/);
    // The copper slot has no rules (it was matched in an older evaluation): nothing to remove.
    expect(edgeRemoval(edges.find((e) => e.kind === "map")!, profile, sources)).toBeNull();
    expect(nodeRemoval(profile, data("slot:src-cu"))).toBeNull();
    expect(nodeRemoval(profile, data("slot:src-spare"))).toEqual({ kind: "slot", id: "src-spare" });
    expect(nodeRemoval(profile, data("group:g-a"))).toBeNull();
    expect(nodeRemoval(profile, data("group:g-unused"))).toEqual({ kind: "group", id: "g-unused" });
    expect(nodeRemoval(profile, data("layer:wl-box"))).toEqual({ kind: "layer", id: "wl-box" });
    expect(nodeRemoval(profile, data("file:0"))).toEqual({ kind: "file", index: 0 });
  });
});

describe("unmapping on the graph", () => {
  it("matches globs as the core does", () => {
    expect(globMatch("*-F_Cu.gbr", "board-f_cu.GBR")).toBe(true);
    expect(globMatch("copper,*,top", "Copper,L1,Top")).toBe(true);
    expect(globMatch("b?ard*", "board.dxf")).toBe(true);
    expect(globMatch("*.gko", "board.gm1")).toBe(false);
    expect(globMatch("", "")).toBe(true);
    expect(globMatch("a*", "")).toBe(false);
  });

  it("removes every rule of the slot that matches the layer", () => {
    const mapped: Profile = structuredClone(profile);
    mapped.sourceBindings[0]!.rules = [
      { kind: "fileName", format: null, pattern: "*.dxf" },
      { kind: "layerName", format: "dxf", name: "COPPER" },
      { kind: "fileLayer", file: "board.dxf", name: "COPPER" },
      { kind: "layerName", format: "gerber", name: "COPPER" },
    ];
    const { edges } = buildGraph(mapped, sources, evaluation);
    const map = edges.find((e) => e.kind === "map")!;
    // The pattern matches the file; the gerber-only name does not match a DXF.
    expect(edgeRemoval(map, mapped, sources)).toEqual({ kind: "unmap", slot: "src-cu", rules: [0, 1, 2] });
  });
});
