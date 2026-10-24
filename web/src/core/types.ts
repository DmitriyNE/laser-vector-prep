// TypeScript mirror of the core's serde types (camelCase JSON). Keep in step with
// crates/lvp-core/src/model.rs and project.rs.

export type Severity = "info" | "warning" | "error";

export interface Diagnostic {
  severity: Severity;
  code: string;
  message: string;
  context: string | null;
}

export type Bounds = [number, number, number, number];

export interface SourceLayerSummary {
  name: string;
  color: number | null;
  closedPaths: number;
  openPaths: number;
  hatches: number;
  boundsMm: Bounds | null;
}

export interface SourceSummary {
  index: number;
  name: string;
  format: string;
  version: string | null;
  units: string | null;
  assumedUnits: string;
  boundsMm: Bounds | null;
  layers: SourceLayerSummary[];
  diagnostics: Diagnostic[];
}

export type GeometryKind = "region" | "linework";
export type LayerOp = "union" | "intersection" | "subtraction" | "xor";
export type LoopRule = "evenOdd" | "nonZero" | "mergeThenNest";
export type StrokeWidth = "none" | "original" | { fixed: { mm: number } };
export type Operand = { source: string } | { layer: string };
export type SourceFormat = "dxf" | "gerber" | "svg" | "pdf";

export type MatchRule =
  | { kind: "layerName"; format?: SourceFormat | null; name: string }
  | { kind: "fileLayer"; file: string; name: string };

export interface SourceBinding {
  id: string;
  name: string;
  required: boolean;
  rules: MatchRule[];
}

export type Pattern =
  | { kind: "passthrough"; simplifyMm?: number | null; reverse: boolean }
  | { kind: "contourFill"; pitchMm: number; firstOffsetMm?: number | null; gapFill: boolean };

export interface Pass {
  id: string;
  name: string;
  pattern: Pattern;
  group: string;
  enabled: boolean;
}

export interface WorkingLayer {
  id: string;
  name: string;
  kind: GeometryKind;
  inputs: Operand[];
  op: LayerOp;
  interpretation: { loopRule: LoopRule; strokeWidth: StrokeWidth };
  passes: Pass[];
  enabled: boolean;
  referenceOnly: boolean;
}

export interface OutputGroup {
  id: string;
  name: string;
  color: [number, number, number];
  strokeWidthMm?: number | null;
  export: boolean;
  visible: boolean;
}

export interface Profile {
  schemaVersion: number;
  profileId: string;
  name: string;
  sourceBindings: SourceBinding[];
  workingLayers: WorkingLayer[];
  outputGroups: OutputGroup[];
  export: { strokeWidthMm: number; marginMm: number; separateFiles: boolean; flattenToleranceMm: number };
  optimizer: { mode: "asGenerated" | "minimumJump" | "bleedAware" };
}

export interface BindingSummary {
  id: string;
  name: string;
  required: boolean;
  status: "matched" | "missing" | "ambiguous";
  file: string | null;
  layer: string | null;
  candidates: [string, string][];
}

export interface LayerSummary {
  id: string;
  name: string;
  kind: GeometryKind;
  outers: number;
  holes: number;
  paths: number;
  areaMm2: number | null;
  boundsMm: Bounds | null;
  cached: boolean;
  diagnostics: Diagnostic[];
}

export interface PassSummary {
  id: string;
  name: string;
  layer: string;
  group: string;
  paths: number;
  closed: number;
  open: number;
  lengthMm: number;
  cached: boolean;
  diagnostics: Diagnostic[];
}

export interface EvaluationSummary {
  revision: number;
  boundsMm: Bounds | null;
  bindings: BindingSummary[];
  unboundLayers: [string, string][];
  layers: LayerSummary[];
  passes: PassSummary[];
  diagnostics: Diagnostic[];
  hasErrors: boolean;
}

/** Flattened polylines: path i spans coords[2*starts[i] .. 2*starts[i+1]]. */
export interface Preview {
  coords: Float32Array;
  starts: Uint32Array;
  closed: Uint8Array;
}

export interface GroupPdf {
  group: string;
  name: string;
  bytes: Uint8Array;
}
