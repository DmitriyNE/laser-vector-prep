// Drawing preview buffers on a 2D canvas: the view transform and paths with native arcs.

import type { Preview } from "../core/types.js";

/** A view of the document: mm at the centre of a w × h pixel area, and pixels per mm. */
export type View = { cx: number; cy: number; ppm: number; w: number; h: number };

export function project(v: View, x: number, y: number): [number, number] {
  return [v.w / 2 + (x - v.cx) * v.ppm, v.h / 2 - (y - v.cy) * v.ppm];
}

/** The view's rectangle in document mm: [min x, min y, max x, max y]. */
export function viewBounds(v: View): [number, number, number, number] {
  const hw = v.w / 2 / v.ppm;
  const hh = v.h / 2 / v.ppm;
  return [v.cx - hw, v.cy - hh, v.cx + hw, v.cy + hh];
}

/**
 * Add the segment from (x1, y1) to (x2, y2) mm with `bulge` to the current path: a native
 * arc, or a line when the arc's sagitta is below a fraction of a pixel. The y flip turns
 * the model's counter-clockwise arcs into the canvas's anticlockwise ones.
 */
export function segment(ctx: CanvasPath, v: View, x1: number, y1: number, x2: number, y2: number, bulge: number) {
  const [sx2, sy2] = project(v, x2, y2);
  const chord = Math.hypot(x2 - x1, y2 - y1);
  if (bulge === 0 || chord === 0 || (Math.abs(bulge) * chord * v.ppm) / 2 < 0.25) {
    ctx.lineTo(sx2, sy2);
    return;
  }
  // Centre: chord midpoint plus the left normal times chord (1 - b^2) / (4 b).
  const mx = (x1 + x2) / 2;
  const my = (y1 + y2) / 2;
  const off = (1 - bulge * bulge) / (4 * bulge);
  const cx = mx - (y2 - y1) * off;
  const cy = my + (x2 - x1) * off;
  const r = Math.hypot(x1 - cx, y1 - cy);
  const [scx, scy] = project(v, cx, cy);
  const [sx1, sy1] = project(v, x1, y1);
  const a0 = Math.atan2(sy1 - scy, sx1 - scx);
  const a1 = Math.atan2(sy2 - scy, sx2 - scx);
  ctx.arc(scx, scy, r * v.ppm, a0, a1, bulge > 0);
}

/** Add path `i` of a buffer to `ctx` as one subpath (closed loops closed). */
export function tracePath(ctx: CanvasPath, v: View, p: Preview, i: number) {
  const a = p.starts[i]!;
  const b = p.starts[i + 1]!;
  if (b - a < 2) return;
  const x = (k: number) => p.originX + p.coords[2 * k]!;
  const y = (k: number) => p.originY + p.coords[2 * k + 1]!;
  const isClosed = p.closed[i] === 1;
  const [x0, y0] = project(v, x(a), y(a));
  ctx.moveTo(x0, y0);
  const segments = isClosed ? b - a : b - a - 1;
  for (let s = 0; s < segments; s++) {
    const k = a + s;
    const n = s + 1 < b - a ? k + 1 : a; // the closing segment returns to the start
    segment(ctx, v, x(k), y(k), x(n), y(n), p.bulges[k]!);
  }
  if (isClosed) ctx.closePath();
}

// ---- paths kept in document space ------------------------------------------------------
//
// Tracing every path into screen coordinates on every frame costs hundreds of milliseconds
// on a large board. A path traced once in document mm (relative to its buffer's origin, y
// up) is drawn through a transform instead, which the browser draws much faster. Canvas
// paths and transforms are single precision, though, so at extreme zoom on a large drawing
// the screen-space tracing above takes over again (`documentSpaceOk`).

/** Device coordinate (px) below which single precision stays within a quarter pixel. */
const FLOAT_SAFE = 4e6;

/** Largest coordinate of a buffer relative to its origin (mm). */
export function extent(p: Preview): number {
  let m = 0;
  for (const c of p.coords) m = Math.max(m, Math.abs(c));
  return m;
}

/** Whether paths `extentMm` across, kept in document space, draw precisely through `v`. */
export function documentSpaceOk(ctx: CanvasRenderingContext2D, v: View, extentMm: number): boolean {
  return v.ppm * ctx.getTransform().a * extentMm < FLOAT_SAFE;
}

/** Add path `i` of a buffer in document mm relative to the buffer's origin, y up. */
export function traceDocument(path: Path2D, p: Preview, i: number) {
  const a = p.starts[i]!;
  const b = p.starts[i + 1]!;
  if (b - a < 2) return;
  const c = p.coords;
  path.moveTo(c[2 * a]!, c[2 * a + 1]!);
  const isClosed = p.closed[i] === 1;
  const segments = isClosed ? b - a : b - a - 1;
  for (let s = 0; s < segments; s++) {
    const k = a + s;
    const n = s + 1 < b - a ? k + 1 : a;
    const x1 = c[2 * k]!;
    const y1 = c[2 * k + 1]!;
    const x2 = c[2 * n]!;
    const y2 = c[2 * n + 1]!;
    const bulge = p.bulges[k]!;
    const chord = Math.hypot(x2 - x1, y2 - y1);
    // Below a nanometre of sagitta an arc is drawn as its chord.
    if (bulge === 0 || chord === 0 || (Math.abs(bulge) * chord) / 2 < 1e-6) {
      path.lineTo(x2, y2);
      continue;
    }
    const off = (1 - bulge * bulge) / (4 * bulge);
    const cx = (x1 + x2) / 2 - (y2 - y1) * off;
    const cy = (y1 + y2) / 2 + (x2 - x1) * off;
    // Counter-clockwise (positive bulge) is increasing angle with y up.
    path.arc(cx, cy, Math.hypot(x1 - cx, y1 - cy), Math.atan2(y1 - cy, x1 - cx), Math.atan2(y2 - cy, x2 - cx), bulge < 0);
  }
  if (isClosed) path.closePath();
}

/** Every path of a buffer in document space. */
export function documentPath(p: Preview): Path2D {
  const path = new Path2D();
  for (let i = 0; i + 1 < p.starts.length; i++) traceDocument(path, p, i);
  return path;
}

/**
 * Run `draw` with `ctx` mapping a buffer's document space through view `v`; `draw` gets the
 * size of a screen pixel in document mm, for line widths and dashes.
 */
export function inDocument(ctx: CanvasRenderingContext2D, v: View, p: Preview, draw: (px: number) => void) {
  ctx.save();
  ctx.transform(v.ppm, 0, 0, -v.ppm, v.w / 2 + (p.originX - v.cx) * v.ppm, v.h / 2 + (v.cy - p.originY) * v.ppm);
  draw(1 / v.ppm);
  ctx.restore();
}
