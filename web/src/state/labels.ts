// Short labels for profile objects, shared by the layer list and the graph view.

import type { MatchRule, Pass, WorkingLayer } from "../core/types.js";

export function patternLabel(p: Pass): string {
  const b = p.breaks;
  const breaks = !b ? "" : b.kind === "tabs" ? ` · ${b.count} tab${b.count === 1 ? "" : "s"}` : ` · dashed ${b.markMm}/${b.gapMm}`;
  return baseLabel(p) + breaks;
}

function baseLabel(p: Pass): string {
  const pt = p.pattern;
  switch (pt.kind) {
    case "contourFill":
      return `contour ${pt.pitchMm} mm${pt.gapFill ? " +gap" : ""}${pt.ringOrder && pt.ringOrder !== "any" ? ` ${pt.ringOrder}` : ""}${pt.link ? " linked" : ""}`;
    case "boundaryTrace":
      return `trace ${pt.side}${pt.offsetMm ? ` ${pt.offsetMm} mm` : ""}${pt.kerfMm ? ` kerf ${pt.kerfMm}` : ""}${pt.rings > 1 ? ` ×${pt.rings}` : ""}${pt.join && pt.join.kind !== "round" ? ` ${pt.join.kind}` : ""}`;
    case "dots":
      return `${pt.lattice} ${pt.shape === "dash" ? "dashes" : "dots"} ${pt.pitchMm} mm`;
    case "centreline":
      return `centreline${pt.minLengthMm ? ` ≥ ${pt.minLengthMm} mm` : ""}`;
    case "hatch":
      return `hatch ${pt.pitchMm} mm @ ${pt.angleDeg}°${pt.linkMm ? " linked" : ""}`;
    case "crossHatch":
      return `crosshatch ${pt.pitchMm} mm @ ${pt.anglesDeg.join("/")}°${pt.linkMm ? " linked" : ""}`;
    case "boundaryHatch":
      return `${pt.shells} shell${pt.shells === 1 ? "" : "s"} + hatch ${pt.pitchMm} mm${pt.linkMm ? " linked" : ""}`;
    default:
      return "passthrough";
  }
}

export function envelopeLabel(l: WorkingLayer): string {
  const e = l.envelope;
  if (!e) return "envelope";
  switch (e.provider) {
    case "namedBoundary":
      return "named boundary";
    case "boundingBox":
      return `box +${e.marginMm} mm`;
    case "boundingCircle":
      return `circle +${e.marginMm} mm`;
    case "buffer":
      return `buffer ${e.distanceMm} mm${e.join && e.join.kind !== "round" ? ` ${e.join.kind}` : ""}`;
    case "convexHull":
      return `hull +${e.distanceMm} mm${e.join && e.join.kind !== "round" ? ` ${e.join.kind}` : ""}`;
  }
}

export function opLabel(l: WorkingLayer): string {
  if (l.kind === "envelope") return envelopeLabel(l);
  return l.op === "union" ? "union" : l.op === "intersection" ? "A ∩ B" : l.op === "subtraction" ? "A − B" : "A xor B";
}

export const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

export function ruleText(r: MatchRule): string {
  switch (r.kind) {
    case "layerName":
      return `name "${r.name}"${r.format ? ` (${r.format})` : ""}`;
    case "fileLayer":
      return `${r.file} : ${r.name}`;
    case "fileName":
      return `file ${r.pattern}${r.format ? ` (${r.format})` : ""}`;
    case "function":
      return `function ${r.pattern}`;
  }
}
