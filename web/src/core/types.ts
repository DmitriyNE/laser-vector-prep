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
  /** Declared layer function (Gerber X2 file function, IPC-2581 layer function), if any. */
  function: string | null;
  closedPaths: number;
  openPaths: number;
  hatches: number;
  boundsMm: Bounds | null;
}

export interface SourceSummary {
  index: number;
  name: string;
  format: SourceFormat;
  version: string | null;
  units: string | null;
  assumedUnits: string;
  boundsMm: Bounds | null;
  layers: SourceLayerSummary[];
  diagnostics: Diagnostic[];
}

export type GeometryKind = "region" | "linework" | "envelope";

export type EnvelopeProvider =
  | { provider: "namedBoundary" }
  | { provider: "boundingBox"; marginMm: number }
  | { provider: "boundingCircle"; marginMm: number }
  | { provider: "buffer"; distanceMm: number }
  | { provider: "convexHull"; distanceMm: number };

export type OffsetSide = "inside" | "on" | "outside";
export type LayerOp = "union" | "intersection" | "subtraction" | "xor";
export type LoopRule = "evenOdd" | "nonZero" | "mergeThenNest";
export type StrokeWidth = "none" | "original" | { fixed: { mm: number } };
export type Operand = { source: string } | { layer: string };
export type SourceFormat = "dxf" | "gerber" | "svg" | "pdf" | "gds" | "ipc2581";

export type MatchRule =
  | { kind: "layerName"; format?: SourceFormat | null; name: string }
  | { kind: "fileLayer"; file: string; name: string }
  | { kind: "fileName"; format?: SourceFormat | null; pattern: string }
  | { kind: "function"; pattern: string };

export interface SourceBinding {
  id: string;
  name: string;
  required: boolean;
  rules: MatchRule[];
}

export type Pattern =
  | { kind: "passthrough"; simplifyMm?: number | null; reverse: boolean }
  | { kind: "contourFill"; pitchMm: number; firstOffsetMm?: number | null; gapFill: boolean }
  | {
      kind: "boundaryTrace";
      side: OffsetSide;
      offsetMm: number;
      kerfMm: number;
      rings: number;
      ringSpacingMm: number;
      reverse: boolean;
      innerFirst: boolean;
    }
  | { kind: "hatch"; pitchMm: number; angleDeg: number; phaseMm: number; serpentine: boolean; insetMm: number }
  | { kind: "crossHatch"; pitchMm: number; anglesDeg: number[]; phaseMm: number; serpentine: boolean; insetMm: number }
  | {
      kind: "boundaryHatch";
      shells: number;
      shellPitchMm: number;
      insetMm: number;
      pitchMm: number;
      anglesDeg: number[];
      phaseMm: number;
      serpentine: boolean;
    };

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
  /** Group A, combined by union. */
  inputs: Operand[];
  /** Group B, combined by union; right-hand side of `op`. Unused for union. */
  operands: Operand[];
  op: LayerOp;
  /** Required when kind is "envelope". */
  envelope?: EnvelopeProvider | null;
  interpretation: { loopRule: LoopRule; strokeWidth: StrokeWidth };
  passes: Pass[];
  enabled: boolean;
  referenceOnly: boolean;
}

export type OptimizerMode = "asGenerated" | "minimumJump" | "bleedAware";

export interface OutputGroup {
  id: string;
  name: string;
  color: [number, number, number];
  strokeWidthMm?: number | null;
  export: boolean;
  visible: boolean;
  /** Per-group path-order mode; null follows the profile's optimizer mode. */
  orderMode?: OptimizerMode | null;
}

/** Path-order planning (spec §7). Geometric prediction only, never a machine setting. */
export interface OptimizerSettings {
  mode: OptimizerMode;
  footprintMm: number;
  bleedWeight: number;
  protected: string[];
  cellMm: number;
  startMm?: [number, number] | null;
  endMm?: [number, number] | null;
  maxSweeps: number;
}

export interface Profile {
  schemaVersion: number;
  profileId: string;
  name: string;
  sourceBindings: SourceBinding[];
  workingLayers: WorkingLayer[];
  outputGroups: OutputGroup[];
  export: { strokeWidthMm: number; marginMm: number; separateFiles: boolean; flattenToleranceMm: number };
  optimizer: OptimizerSettings;
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

/** Statistics of a path sequence under the straight-jump assumption. */
export interface OrderStats {
  paths: number;
  jumps: number;
  jumpLengthMm: number;
  longestJumpMm: number;
  crossings: number;
  coveredMm2?: number;
  overlapIntegral?: number;
  peakCoverage?: number;
  objective: number;
}

export interface GroupSummary {
  id: string;
  name: string;
  mode: OptimizerMode;
  paths: number;
  before: OrderStats;
  after: OrderStats;
  heatmap: boolean;
}

export interface EvaluationSummary {
  revision: number;
  boundsMm: Bounds | null;
  bindings: BindingSummary[];
  unboundLayers: [string, string][];
  layers: LayerSummary[];
  passes: PassSummary[];
  groups: GroupSummary[];
  diagnostics: Diagnostic[];
  hasErrors: boolean;
}

/** Jump footprint coverage raster over protected regions; row-major from the origin, y up. */
export interface Heatmap {
  originX: number;
  originY: number;
  cellMm: number;
  width: number;
  height: number;
  counts: Uint16Array;
  mask: Uint8Array;
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
