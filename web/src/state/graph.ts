// The model as a graph, left to right: imported files and their layers → profile slots →
// working layers → passes → output groups. Pure functions of the profile, the imported
// sources and the latest evaluation (no Svelte, no DOM), laid out by dagre.

import { Graph, layout as dagreLayout } from "@dagrejs/dagre";
import type { EvaluationSummary, GeometryKind, Operand, Profile, SourceSummary, WorkingLayer } from "../core/types.js";
import { opLabel, patternLabel, plural } from "./labels.js";

/** Worst diagnostic of a node; `idle` when it has no result (not evaluated, or blocked). */
export type Status = "ok" | "warning" | "error" | "idle";

export interface FileRow {
  name: string;
  /** The slot this layer is matched to, if any. */
  slot: string | null;
  state: "bound" | "ambiguous" | "unbound";
}

export interface FileData {
  kind: "file";
  index: number;
  name: string;
  format: string;
  rows: FileRow[];
}

export interface SlotData {
  kind: "slot";
  id: string;
  name: string;
  required: boolean;
  state: "matched" | "missing" | "ambiguous";
  detail: string;
}

export interface LayerData {
  kind: "layer";
  id: string;
  name: string;
  geometry: GeometryKind;
  op: string;
  /** The layer has a second operand group (B). */
  twoGroups: boolean;
  stats: string;
  status: Status;
  enabled: boolean;
  referenceOnly: boolean;
}

export interface PassData {
  kind: "pass";
  id: string;
  name: string;
  pattern: string;
  stats: string;
  status: Status;
  enabled: boolean;
}

export interface GroupData {
  kind: "group";
  id: string;
  name: string;
  color: [number, number, number];
  stats: string;
}

export type NodeData = FileData | SlotData | LayerData | PassData | GroupData;

export interface GraphNode {
  id: string;
  data: NodeData;
  width: number;
  height: number;
}

/**
 * `map`: an imported layer matched to a slot; `candidate`: one of several layers an
 * ambiguous slot matches; `a` / `b`: an input of operand group A or B; `pass`: a layer's
 * pass; `group`: the output group a pass marks in.
 */
export type EdgeKind = "map" | "candidate" | "a" | "b" | "pass" | "group";

export interface GraphEdge {
  id: string;
  kind: EdgeKind;
  source: string;
  sourceHandle: string;
  target: string;
  targetHandle: string;
}

export const NODE_WIDTH = 210;
/** A file node: its title, then one row per imported layer. */
export const HEADER_HEIGHT = 26;
export const ROW_HEIGHT = 22;
const HEIGHT = { file: HEADER_HEIGHT + 4, slot: 50, layer: 66, pass: 50, group: 50 } as const;

export const nodeId = {
  file: (index: number) => `file:${index}`,
  slot: (id: string) => `slot:${id}`,
  layer: (id: string) => `layer:${id}`,
  pass: (id: string) => `pass:${id}`,
  group: (id: string) => `group:${id}`,
};

/** Handle of a file node's row for an imported layer. */
export const rowHandle = (layer: string) => `row:${layer}`;

/** The graph node an operand refers to, and the handle edges leave it from. */
export function operandNode(o: Operand): { node: string; handle: string } {
  return "source" in o ? { node: nodeId.slot(o.source), handle: "out" } : { node: nodeId.layer(o.layer), handle: "out" };
}

function worst(diags: { severity: string }[]): Status {
  if (diags.some((d) => d.severity === "error")) return "error";
  if (diags.some((d) => d.severity === "warning")) return "warning";
  return "ok";
}

/** Whether a layer shows group B: any operation but a union, or B already in use. */
export function hasGroupB(l: WorkingLayer): boolean {
  return l.kind !== "envelope" && (l.op !== "union" || l.operands.length > 0);
}

export function buildGraph(
  profile: Profile,
  sources: SourceSummary[],
  evaluation: EvaluationSummary | null,
): { nodes: GraphNode[]; edges: GraphEdge[] } {
  const nodes: GraphNode[] = [];
  const edges: GraphEdge[] = [];
  const bindings = evaluation?.bindings ?? [];
  const edge = (kind: EdgeKind, source: string, sourceHandle: string, target: string, targetHandle: string) =>
    edges.push({ id: `${kind}:${source}/${sourceHandle}>${target}/${targetHandle}`, kind, source, sourceHandle, target, targetHandle });

  for (const s of sources) {
    const rows: FileRow[] = s.layers
      .filter((l) => l.closedPaths + l.openPaths + l.hatches > 0)
      .map((l) => {
        const bound = bindings.find((b) => b.status === "matched" && b.file === s.name && b.layer === l.name);
        const ambiguous = bindings.some((b) => b.status === "ambiguous" && b.candidates.some(([f, n]) => f === s.name && n === l.name));
        return { name: l.name, slot: bound?.id ?? null, state: bound ? "bound" : ambiguous ? "ambiguous" : "unbound" };
      });
    nodes.push({
      id: nodeId.file(s.index),
      data: { kind: "file", index: s.index, name: s.name, format: s.format, rows },
      width: NODE_WIDTH,
      height: HEIGHT.file + ROW_HEIGHT * rows.length,
    });
  }
  const fileIndex = (name: string) => sources.find((s) => s.name === name)?.index;

  for (const b of profile.sourceBindings) {
    const summary = bindings.find((x) => x.id === b.id);
    const state = summary?.status ?? "missing";
    const detail =
      state === "matched" ? `${summary!.file} · ${summary!.layer}` : state === "ambiguous" ? `${summary!.candidates.length} candidates` : b.required ? "required, unmatched" : "optional, unmatched";
    nodes.push({
      id: nodeId.slot(b.id),
      data: { kind: "slot", id: b.id, name: b.name, required: b.required, state, detail },
      width: NODE_WIDTH,
      height: HEIGHT.slot,
    });
    if (summary?.status === "matched" && summary.file && summary.layer) {
      const f = fileIndex(summary.file);
      if (f !== undefined) edge("map", nodeId.file(f), rowHandle(summary.layer), nodeId.slot(b.id), "in");
    }
    if (summary?.status === "ambiguous") {
      for (const [file, layer] of summary.candidates) {
        const f = fileIndex(file);
        if (f !== undefined) edge("candidate", nodeId.file(f), rowHandle(layer), nodeId.slot(b.id), "in");
      }
    }
  }

  const groups = new Set(profile.outputGroups.map((g) => g.id));
  const known = new Set([...profile.sourceBindings.map((b) => nodeId.slot(b.id)), ...profile.workingLayers.map((l) => nodeId.layer(l.id))]);
  for (const l of profile.workingLayers) {
    const summary = evaluation?.layers.find((x) => x.id === l.id);
    const profileDiags = evaluation?.diagnostics.filter((d) => d.context === l.id) ?? [];
    const stats = !summary
      ? "not evaluated"
      : l.kind === "linework"
        ? plural(summary.paths, "path")
        : `${plural(summary.outers, "island")}, ${plural(summary.holes, "hole")}`;
    nodes.push({
      id: nodeId.layer(l.id),
      data: {
        kind: "layer",
        id: l.id,
        name: l.name,
        geometry: l.kind,
        op: opLabel(l),
        twoGroups: hasGroupB(l),
        stats,
        status: summary ? worst([...summary.diagnostics, ...profileDiags]) : profileDiags.length ? worst(profileDiags) : "idle",
        enabled: l.enabled,
        referenceOnly: l.referenceOnly,
      },
      width: NODE_WIDTH,
      height: HEIGHT.layer,
    });
    for (const [ops, handle] of [
      [l.inputs, "a"],
      [l.operands, "b"],
    ] as const) {
      for (const o of ops) {
        const from = operandNode(o);
        if (known.has(from.node)) edge(handle, from.node, from.handle, nodeId.layer(l.id), handle);
      }
    }
    for (const p of l.passes) {
      const ps = evaluation?.passes.find((x) => x.id === p.id);
      const audit = ps?.audit && ps.audit.regionMm2 > 0 ? ` · missed ${((100 * ps.audit.missedMm2) / ps.audit.regionMm2).toFixed(2)}%` : "";
      nodes.push({
        id: nodeId.pass(p.id),
        data: {
          kind: "pass",
          id: p.id,
          name: p.name,
          pattern: patternLabel(p),
          stats: ps ? `${plural(ps.paths, "path")}${audit}` : p.enabled ? "not evaluated" : "disabled",
          status: ps ? worst(ps.diagnostics) : "idle",
          enabled: p.enabled,
        },
        width: NODE_WIDTH,
        height: HEIGHT.pass,
      });
      edge("pass", nodeId.layer(l.id), "out", nodeId.pass(p.id), "in");
      if (groups.has(p.group)) edge("group", nodeId.pass(p.id), "out", nodeId.group(p.group), "in");
    }
  }

  for (const g of profile.outputGroups) {
    const gs = evaluation?.groups.find((x) => x.id === g.id);
    nodes.push({
      id: nodeId.group(g.id),
      data: {
        kind: "group",
        id: g.id,
        name: g.name,
        color: g.color,
        stats: gs ? `${plural(gs.paths, "path")} · jumps ${gs.after.jumpLengthMm.toFixed(0)} mm` : "no paths",
      },
      width: NODE_WIDTH,
      height: HEIGHT.group,
    });
  }
  return { nodes, edges };
}

/** What the layout depends on: the nodes, their sizes and the edges, not their status. */
export function structureKey(nodes: GraphNode[], edges: GraphEdge[]): string {
  return [...nodes.map((n) => `${n.id}:${n.height}`), ...edges.map((e) => e.id)].join("\n");
}

/** Column of each kind of node: files, slots, then layers by depth, passes and groups. */
function columns(nodes: GraphNode[], edges: GraphEdge[]): Map<string, number> {
  const depth = new Map<string, number>();
  const layers = nodes.filter((n) => n.data.kind === "layer").map((n) => n.id);
  // Longest path from any slot; edges between layers only (cycles are cut by the bound).
  for (const id of layers) depth.set(id, 0);
  for (let round = 0; round < layers.length; round++) {
    let changed = false;
    for (const e of edges) {
      if ((e.kind === "a" || e.kind === "b") && depth.has(e.source)) {
        const d = depth.get(e.source)! + 1;
        if (d > depth.get(e.target)! && d < layers.length) {
          depth.set(e.target, d);
          changed = true;
        }
      }
    }
    if (!changed) break;
  }
  const maxLayer = Math.max(0, ...depth.values());
  const col = new Map<string, number>();
  for (const n of nodes) {
    const k = n.data.kind;
    col.set(n.id, k === "file" ? 0 : k === "slot" ? 1 : k === "layer" ? 2 + depth.get(n.id)! : k === "pass" ? 3 + maxLayer : 4 + maxLayer);
  }
  return col;
}

/** Horizontal and vertical gaps between nodes. */
const COLUMN_GAP = 60;
const NODE_GAP = 12;

/**
 * Top-left positions, left to right: one column per kind of node (layers by depth), so an
 * unused group still sits with the groups. dagre orders the nodes of each column to keep
 * edges short and uncrossed; within a column nodes keep that order and only move down as
 * far as needed not to overlap.
 */
export function layout(nodes: GraphNode[], edges: GraphEdge[]): Map<string, { x: number; y: number }> {
  const g = new Graph({ multigraph: true });
  g.setGraph({ rankdir: "LR", nodesep: NODE_GAP, ranksep: COLUMN_GAP });
  g.setDefaultEdgeLabel(() => ({}));
  const col = columns(nodes, edges);
  for (const n of nodes) g.setNode(n.id, { width: n.width, height: n.height });
  for (const e of edges) {
    const len = col.get(e.target)! - col.get(e.source)!;
    if (len > 0) g.setEdge(e.source, e.target, { minlen: len, weight: e.kind === "candidate" ? 1 : 4 }, e.id);
  }
  dagreLayout(g);
  const byColumn = new Map<number, GraphNode[]>();
  for (const n of nodes) {
    const c = col.get(n.id)!;
    byColumn.set(c, [...(byColumn.get(c) ?? []), n]);
  }
  const out = new Map<string, { x: number; y: number }>();
  for (const [c, members] of byColumn) {
    const top = (n: GraphNode) => (g.node(n.id) as { y: number }).y - n.height / 2;
    members.sort((a, b) => top(a) - top(b) || a.id.localeCompare(b.id));
    let next = -Infinity;
    for (const n of members) {
      const y = Math.max(top(n), next);
      out.set(n.id, { x: c * (NODE_WIDTH + COLUMN_GAP), y });
      next = y + n.height + NODE_GAP;
    }
  }
  return out;
}
