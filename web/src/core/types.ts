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

/** Corner joins of an offset; omitted means round. */
export type JoinStyle = { kind: "round" } | { kind: "miter"; limit: number } | { kind: "bevel" };

export type EnvelopeProvider =
  | { provider: "namedBoundary" }
  | { provider: "boundingBox"; marginMm: number }
  | { provider: "boundingCircle"; marginMm: number }
  | { provider: "buffer"; distanceMm: number; join?: JoinStyle }
  | { provider: "convexHull"; distanceMm: number; join?: JoinStyle };

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

export type RingOrder = "any" | "inward" | "outward";
export type Lattice = "square" | "hex" | "stipple";
export type DotShape = "circle" | "dash";

export type Pattern =
  | { kind: "passthrough"; simplifyMm?: number | null; reverse: boolean }
  | {
      kind: "contourFill";
      pitchMm: number;
      firstOffsetMm?: number | null;
      gapFill: boolean;
      /** Where gap fill starts beyond the last ring; null = pitch/2. */
      endSpacingMm?: number | null;
      /** Ring simplifying tolerance; null = off. */
      simplifyMm?: number | null;
      /** Order the rings are marked in; absent = the optimizer's choice. */
      ringOrder?: RingOrder;
      /** Join nested rings into one path with connectors a pitch long. */
      link?: boolean;
    }
  | {
      kind: "boundaryTrace";
      side: OffsetSide;
      offsetMm: number;
      kerfMm: number;
      rings: number;
      ringSpacingMm: number;
      reverse: boolean;
      innerFirst: boolean;
      join?: JoinStyle;
    }
  | {
      kind: "dots";
      lattice: Lattice;
      pitchMm: number;
      angleDeg: number;
      phaseMm: number;
      shape: DotShape;
      /** Circle diameter or dash length. */
      sizeMm: number;
      insetMm: number;
    }
  | {
      kind: "centreline";
      /** Leave out centrelines shorter than this; null = keep all. */
      minLengthMm?: number | null;
      simplifyMm?: number | null;
    }
  | { kind: "hatch"; pitchMm: number; angleDeg: number; phaseMm: number; serpentine: boolean; insetMm: number; linkMm?: number | null }
  | { kind: "crossHatch"; pitchMm: number; anglesDeg: number[]; phaseMm: number; serpentine: boolean; insetMm: number; linkMm?: number | null }
  | {
      kind: "boundaryHatch";
      shells: number;
      shellPitchMm: number;
      insetMm: number;
      pitchMm: number;
      anglesDeg: number[];
      phaseMm: number;
      serpentine: boolean;
      /** Join rows along the boundary where the link is at most this long; null = off. */
      linkMm?: number | null;
    };

/** Dashes along every path, or tabs (gaps) in every closed path. */
export type Breaks = { kind: "dashes"; markMm: number; gapMm: number; offsetMm?: number } | { kind: "tabs"; count: number; widthMm: number };

export interface Pass {
  id: string;
  name: string;
  pattern: Pattern;
  breaks?: Breaks | null;
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
  /** Effective marking width (mm) the coverage audit assumes; absent turns it off (schema v2). */
  auditWidthMm?: number | null;
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

/** How the profile's slots resolve against the sources, before any evaluation. */
export interface BindingsSummary {
  bindings: BindingSummary[];
  unboundLayers: [string, string][];
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
  /** Coverage audit against the group's assumed marking width (spec §10). */
  audit?: AuditSummary;
}

/** Paths swept at the assumed width versus the region they fill; a geometric surrogate. */
export interface AuditSummary {
  widthMm: number;
  regionMm2: number;
  /** Region area no path sweeps. */
  missedMm2: number;
  /** Swept area outside the region. */
  spillMm2: number;
  /** Paths shorter than the width. */
  shortSegments: number;
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

/**
 * Polylines with unflattened arcs: path i has vertices starts[i] .. starts[i+1]; vertex k is
 * (originX + coords[2k], originY + coords[2k+1]) and bulges[k] is tan(sweep/4) of the
 * segment leaving it (a closed path's last vertex closes it). Millimetres, y up. The f32
 * coordinates are offsets from the buffer's own origin so they stay precise far from (0, 0).
 */
export interface Preview {
  originX: number;
  originY: number;
  coords: Float32Array;
  bulges: Float32Array;
  starts: Uint32Array;
  closed: Uint8Array;
}

/**
 * An output group's marking sequence as exported: its paths in order and direction, the
 * pass each came from (an index into `passes`), and the optimizer's assumed start and end
 * points, if set.
 */
export interface OrderPreview {
  paths: Preview;
  passOf: Uint32Array;
  passes: string[];
  start: [number, number] | null;
  end: [number, number] | null;
}

export interface GroupPdf {
  group: string;
  name: string;
  bytes: Uint8Array;
}
