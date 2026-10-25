// Short labels for profile objects, shared by the layer list and the graph view.

import type { Pass, WorkingLayer } from "../core/types.js";

export function patternLabel(p: Pass): string {
  const pt = p.pattern;
  switch (pt.kind) {
    case "contourFill":
      return `contour ${pt.pitchMm} mm${pt.gapFill ? " +gap" : ""}`;
    case "boundaryTrace":
      return `trace ${pt.side}${pt.offsetMm ? ` ${pt.offsetMm} mm` : ""}${pt.kerfMm ? ` kerf ${pt.kerfMm}` : ""}${pt.rings > 1 ? ` ×${pt.rings}` : ""}${pt.join && pt.join.kind !== "round" ? ` ${pt.join.kind}` : ""}`;
    case "hatch":
      return `hatch ${pt.pitchMm} mm @ ${pt.angleDeg}°`;
    case "crossHatch":
      return `crosshatch ${pt.pitchMm} mm @ ${pt.anglesDeg.join("/")}°`;
    case "boundaryHatch":
      return `${pt.shells} shell${pt.shells === 1 ? "" : "s"} + hatch ${pt.pitchMm} mm`;
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
