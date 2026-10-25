// The model as a graph, left to right: imported files and their layers → profile slots →
// working layers → passes → output groups. Pure functions of the profile, the imported
// sources and the latest evaluation (no Svelte, no DOM), laid out by dagre.

import { Graph, layout as dagreLayout } from "@dagrejs/dagre";
import type { EvaluationSummary, GeometryKind, MatchRule, Operand, Profile, SourceSummary, WorkingLayer } from "../core/types.js";
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

// ---- editing on the graph -------------------------------------------------------------

/** A node id taken apart: its kind and the id of the profile object (a file's index). */
export function parseNodeId(id: string): { kind: NodeData["kind"]; key: string } | null {
  const i = id.indexOf(":");
  const kind = id.slice(0, i);
  return ["file", "slot", "layer", "pass", "group"].includes(kind) ? { kind: kind as NodeData["kind"], key: id.slice(i + 1) } : null;
}

function operandOf(nodeIdText: string): Operand | null {
  const n = parseNodeId(nodeIdText);
  if (n?.kind === "slot") return { source: n.key };
  if (n?.kind === "layer") return { layer: n.key };
  return null;
}

const sameOperand = (a: Operand, b: Operand) => ("source" in a && "source" in b && a.source === b.source) || ("layer" in a && "layer" in b && a.layer === b.layer);

/** Layers `id` reads, directly or through other layers. */
export function dependencies(profile: Profile, id: string): Set<string> {
  const seen = new Set<string>();
  const visit = (lid: string) => {
    const l = profile.workingLayers.find((x) => x.id === lid);
    for (const o of [...(l?.inputs ?? []), ...(l?.operands ?? [])]) {
      if ("layer" in o && !seen.has(o.layer)) {
        seen.add(o.layer);
        visit(o.layer);
      }
    }
  };
  visit(id);
  return seen;
}

/** Why `from` cannot become an input of group A or B of a layer, or null if it can. */
export function operandProblem(profile: Profile, from: Operand, layerId: string, group: "a" | "b"): string | null {
  const layer = profile.workingLayers.find((l) => l.id === layerId);
  if (!layer) return "no such layer";
  if ("source" in from && !profile.sourceBindings.some((b) => b.id === from.source)) return "no such slot";
  if ("layer" in from) {
    if (from.layer === layerId) return "a layer cannot read itself";
    if (!profile.workingLayers.some((l) => l.id === from.layer)) return "no such layer";
    if (dependencies(profile, from.layer).has(layerId)) return "that would make a cycle";
  }
  if (group === "b" && !hasGroupB(layer)) return "this layer has one input group";
  if ((group === "a" ? layer.inputs : layer.operands).some((o) => sameOperand(o, from))) return "already an input";
  return null;
}

/** A profile edit drawn on the graph. */
export type Edit =
  | { kind: "map"; slot: string; file: string; layer: string }
  | { kind: "operand"; from: Operand; layer: string; group: "a" | "b" }
  | { kind: "movePass"; pass: string; layer: string }
  | { kind: "passGroup"; pass: string; group: string };

/** The edit a connection from one handle to another stands for, or why it makes none. */
export function connectionEdit(
  profile: Profile,
  sources: SourceSummary[],
  c: { source: string; sourceHandle?: string | null; target: string; targetHandle?: string | null },
): Edit | string {
  const from = parseNodeId(c.source);
  const to = parseNodeId(c.target);
  if (!from || !to) return "unknown node";
  if (from.kind === "file" && to.kind === "slot") {
    const file = sources.find((s) => String(s.index) === from.key);
    const layer = c.sourceHandle?.startsWith("row:") ? c.sourceHandle.slice(4) : null;
    if (!file || !layer) return "unknown imported layer";
    return { kind: "map", slot: to.key, file: file.name, layer };
  }
  if ((from.kind === "slot" || from.kind === "layer") && to.kind === "layer") {
    const group = c.targetHandle === "b" ? "b" : "a";
    const operand = operandOf(c.source)!;
    return operandProblem(profile, operand, to.key, group) ?? { kind: "operand", from: operand, layer: to.key, group };
  }
  if (from.kind === "layer" && to.kind === "pass") {
    const owner = profile.workingLayers.find((l) => l.passes.some((p) => p.id === to.key));
    if (!profile.workingLayers.some((l) => l.id === from.key)) return "no such layer";
    return owner?.id === from.key ? "the pass is already on this layer" : { kind: "movePass", pass: to.key, layer: from.key };
  }
  if (from.kind === "pass" && to.kind === "group") {
    const pass = profile.workingLayers.flatMap((l) => l.passes).find((p) => p.id === from.key);
    if (!pass) return "no such pass";
    return pass.group === to.key ? "the pass already marks in this group" : { kind: "passGroup", pass: from.key, group: to.key };
  }
  return "these cannot be connected";
}

/** Something deleted on the graph. */
export type Removal =
  | { kind: "operand"; from: Operand; layer: string; group: "a" | "b" }
  | { kind: "unmap"; slot: string; rules: number[] }
  | { kind: "layer" | "pass" | "slot" | "group"; id: string }
  | { kind: "file"; index: number };

/** The imported layer a mapping edge starts at. */
function mappedLayer(e: GraphEdge, sources: SourceSummary[]) {
  const file = sources.find((s) => nodeId.file(s.index) === e.source);
  const name = e.sourceHandle.startsWith("row:") ? e.sourceHandle.slice(4) : null;
  return file && name ? { file, layer: file.layers.find((l) => l.name === name), name } : null;
}

/** `pattern` matches `text`: `*` any run, `?` one character, case-insensitive (the core's
 * `source::glob_match`). */
export function globMatch(pattern: string, text: string): boolean {
  const p = [...pattern.toLowerCase()];
  const t = [...text.toLowerCase()];
  let [pi, ti] = [0, 0];
  let star: [number, number] | null = null;
  while (ti < t.length) {
    if (pi < p.length && (p[pi] === "?" || p[pi] === t[ti])) {
      pi++;
      ti++;
    } else if (pi < p.length && p[pi] === "*") {
      star = [pi, ti];
      pi++;
    } else if (star) {
      pi = star[0] + 1;
      ti = star[1] + 1;
      star = [star[0], star[1] + 1];
    } else {
      return false;
    }
  }
  while (pi < p.length && p[pi] === "*") pi++;
  return pi === p.length;
}

/** Whether a slot's rule matches an imported layer, as the core decides it
 * (`eval::resolve_bindings`). */
export function ruleMatches(r: MatchRule, file: SourceSummary, layer: string): boolean {
  const l = file.layers.find((x) => x.name === layer);
  if (!l) return false;
  const format = (f: string | null | undefined) => !f || f === file.format;
  switch (r.kind) {
    case "layerName":
      return format(r.format) && r.name === layer;
    case "fileLayer":
      return r.file === file.name && r.name === layer;
    case "fileName":
      return format(r.format) && l.closedPaths + l.openPaths + l.hatches > 0 && globMatch(r.pattern, file.name);
    case "function":
      return l.function !== null && globMatch(r.pattern, l.function);
  }
}

/** Rules of a slot that match an imported layer: without them the slot cannot take it. */
export function rulesMatching(profile: Profile, slot: string, file: SourceSummary, layer: string): number[] {
  const rules = profile.sourceBindings.find((b) => b.id === slot)?.rules ?? [];
  return rules.flatMap((r, i) => (ruleMatches(r, file, layer) ? [i] : []));
}

/** What removing an edge removes: an operand edge the input, a mapping every rule of the
 * slot that matches the layer (patterns too, so the slot really lets go of it). Other edges
 * cannot go; `edgeHint` says how to change them. */
export function edgeRemoval(e: GraphEdge, profile: Profile, sources: SourceSummary[]): Removal | null {
  if (e.kind === "a" || e.kind === "b") {
    const from = operandOf(e.source);
    const layer = parseNodeId(e.target);
    return from && layer ? { kind: "operand", from, layer: layer.key, group: e.kind } : null;
  }
  if (e.kind === "map") {
    const m = mappedLayer(e, sources);
    const slot = parseNodeId(e.target)?.key;
    if (!m || !slot) return null;
    const rules = rulesMatching(profile, slot, m.file, m.name);
    return rules.length ? { kind: "unmap", slot, rules } : null;
  }
  return null;
}

/** How an edge can be changed, for an edge that cannot simply be removed. */
export function edgeHint(e: GraphEdge): string {
  switch (e.kind) {
    case "map":
      return "drag another imported layer onto the slot to map that one instead";
    case "candidate":
      return "one of several layers this slot matches: drag the one you want onto the slot";
    case "pass":
      return "a pass belongs to its layer: drag from another layer to move it, or delete the pass";
    case "group":
      return "a pass always marks in one group: drag from the pass to another group";
    default:
      return "click × to remove this input";
  }
}

/** What deleting a node removes. Slots and groups still in use stay (their users would be
 * left dangling); a layer takes its passes along and is removed from its readers' inputs. */
export function nodeRemoval(profile: Profile, data: NodeData): Removal | null {
  switch (data.kind) {
    case "file":
      return { kind: "file", index: data.index };
    case "slot":
      return profile.workingLayers.some((l) => [...l.inputs, ...l.operands].some((o) => "source" in o && o.source === data.id)) ? null : { kind: "slot", id: data.id };
    case "group":
      return profile.workingLayers.some((l) => l.passes.some((p) => p.group === data.id)) ? null : { kind: "group", id: data.id };
    default:
      return { kind: data.kind, id: data.id };
  }
}
