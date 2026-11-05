// The order inspector's drawing: one group's marking sequence up to the current path.
// The whole sequence is drawn faint under the scene's other layers (`drawOrderBase`, kept
// while stepping); over it, marked paths are coloured by their place in the order (or by
// group), chevrons along them give the direction of travel, and the current path stands out.

import { BUCKET_COLORS, bucketOf, ORDER_BUCKETS, segUpTo, type BeamAt, type Mark, type Pt, type Sequence } from "../state/sequence.js";
import { documentSpaceOk, extent, inDocument, project, segment, traceDocument, tracePath, viewBounds, type View } from "./canvas.js";

export interface OrderState {
  /** The current path; the ones before it are marked. */
  index: number;
  gradient: boolean;
  /** Group colour, for marked paths without the gradient. */
  color: string;
  /** Draw the jumps between marked paths (the one into the current path always shows). */
  jumps: boolean;
  /** Paths of hidden passes are left out; the numbering stays. */
  hidden: (i: number) => boolean;
  /** Changes whenever `hidden` does. */
  hiddenKey: string;
  /** Path under the pointer, or -1. */
  hover: number;
  /** Trajectory mode: where the beam is. Its path is the current one, marked up to it. */
  beam: BeamAt | null;
}

/** Screen distance between direction chevrons on marked paths (px). */
const SPACING = 64;
/** Most chevrons drawn on marked paths, the latest paths first. */
const MAX_MARKS = 6000;
const FAINT = "rgba(128, 128, 128, 0.3)";
const JUMP = "rgba(220, 0, 200, 0.8)";
const HALO = "rgba(255, 190, 0, 0.5)";
const HOVER = "rgba(31, 111, 235, 0.9)";
const BEAM = "rgb(255, 40, 40)";
const BEAM_GLOW = "rgba(255, 60, 60, 0.25)";

/** An open arrowhead of `size` px pointing along the travel at mark `m`. */
function chevron(path: Path2D, v: View, m: Mark, size: number) {
  const [x, y] = project(v, m.x, m.y);
  // Screen y points down.
  const dx = m.dx;
  const dy = -m.dy;
  const tx = x + (dx * size) / 2;
  const ty = y + (dy * size) / 2;
  const bx = tx - dx * size;
  const by = ty - dy * size;
  const w = size * 0.6;
  path.moveTo(bx - dy * w, by + dx * w);
  path.lineTo(tx, ty);
  path.lineTo(bx + dy * w, by - dx * w);
}

/** The sequence's paths in document space, one `Path2D` per colour bucket, and their extent. */
const documentPaths = new WeakMap<Sequence, { key: string; buckets: Path2D[]; extent: number }>();

function inDocumentSpace(seq: Sequence, st: OrderState) {
  const kept = documentPaths.get(seq);
  if (kept?.key === st.hiddenKey) return kept;
  const buckets = Array.from({ length: ORDER_BUCKETS }, () => new Path2D());
  for (let i = 0; i < seq.count; i++) if (!st.hidden(i)) traceDocument(buckets[bucketOf(i, seq.count)]!, seq.order.paths, i);
  const made = { key: st.hiddenKey, buckets, extent: extent(seq.order.paths) };
  documentPaths.set(seq, made);
  return made;
}

/** Stroke paths `from` .. `to` (exclusive) that are in view, `color(i)` each, one batch per colour. */
function strokeScreen(ctx: CanvasRenderingContext2D, v: View, seq: Sequence, st: OrderState, from: number, to: number, color: (i: number) => string) {
  const batches = new Map<string, Path2D>();
  const visible = inView(v, seq);
  for (let i = from; i < to; i++) {
    if (st.hidden(i) || !visible(i)) continue;
    const c = color(i);
    let path = batches.get(c);
    if (!path) batches.set(c, (path = new Path2D()));
    tracePath(path, v, seq.order.paths, i);
  }
  ctx.lineWidth = 1;
  for (const [c, path] of batches) {
    ctx.strokeStyle = c;
    ctx.stroke(path);
  }
}

/** A test of whether a path's bounds come within a few pixels of the view. */
function inView(v: View, seq: Sequence): (i: number) => boolean {
  const vb = viewBounds(v);
  const pad = 8 / v.ppm;
  const b = seq.bounds;
  return (i) => b[4 * i]! <= vb[2] + pad && b[4 * i + 2]! >= vb[0] - pad && b[4 * i + 1]! <= vb[3] + pad && b[4 * i + 3]! >= vb[1] - pad;
}

/** The whole sequence, faint: under the scene's other layers, kept while stepping. */
export function drawOrderBase(ctx: CanvasRenderingContext2D, v: View, seq: Sequence, st: OrderState) {
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  const doc = inDocumentSpace(seq, st);
  if (documentSpaceOk(ctx, v, doc.extent)) {
    inDocument(ctx, v, seq.order.paths, (px) => {
      ctx.lineWidth = px;
      ctx.strokeStyle = FAINT;
      for (const path of doc.buckets) ctx.stroke(path);
    });
  } else strokeScreen(ctx, v, seq, st, 0, seq.count, () => FAINT);
  ctx.lineCap = "butt";
  ctx.lineJoin = "miter";
}

function dot(ctx: CanvasRenderingContext2D, v: View, p: [number, number], r: number, fill: string | null, stroke: string) {
  const [x, y] = project(v, p[0], p[1]);
  ctx.beginPath();
  ctx.arc(x, y, r, 0, 2 * Math.PI);
  if (fill) {
    ctx.fillStyle = fill;
    ctx.fill();
  }
  ctx.strokeStyle = stroke;
  ctx.lineWidth = 1.5;
  ctx.stroke();
}

/** Path `i` from its start to arc length `s`, in screen space. */
function traceTo(v: View, seq: Sequence, i: number, s: number): Path2D {
  const path = new Path2D();
  const [x0, y0] = project(v, ...seq.first(i));
  path.moveTo(x0, y0);
  let base = 0;
  for (const g of seq.segments(i)) {
    if (base >= s) break;
    const end = segUpTo(g, s - base);
    segment(path, v, g.x1, g.y1, end.x, end.y, end.bulge);
    base += g.length;
  }
  return path;
}

/** A jump from `a` to `b`, travelled as far as `to`: done solid, the rest dashed. */
function partJump(ctx: CanvasRenderingContext2D, v: View, a: Pt, to: Pt, b: Pt) {
  const [ax, ay] = project(v, ...a);
  const [ex, ey] = project(v, ...to);
  const [bx, by] = project(v, ...b);
  ctx.beginPath();
  ctx.moveTo(ex, ey);
  ctx.lineTo(bx, by);
  ctx.setLineDash([4, 3]);
  ctx.lineWidth = 1;
  ctx.strokeStyle = JUMP;
  ctx.stroke();
  ctx.setLineDash([]);
  ctx.beginPath();
  ctx.moveTo(ax, ay);
  ctx.lineTo(ex, ey);
  ctx.lineWidth = 2;
  ctx.stroke();
}

export function drawOrder(ctx: CanvasRenderingContext2D, v: View, seq: Sequence, st: OrderState) {
  if (!seq.count) return;
  const beam = st.beam;
  const index = beam ? beam.index : Math.min(Math.max(st.index, 0), seq.count - 1);
  const p = seq.order.paths;
  const visible = inView(v, seq);
  const colorOf = (i: number) => (st.gradient ? BUCKET_COLORS[bucketOf(i, seq.count)]! : st.color);
  ctx.setLineDash([]);
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  // Marked paths: the buckets before the current path's whole, then its own bucket's
  // paths before it; chevrons where they are long enough on screen.
  const current = bucketOf(index, seq.count);
  const bucketColor = (k: number) => (st.gradient ? BUCKET_COLORS[k]! : st.color);
  const doc = inDocumentSpace(seq, st);
  let first = 0; // the first marked path not drawn from the buckets
  if (documentSpaceOk(ctx, v, doc.extent)) {
    inDocument(ctx, v, p, (px) => {
      ctx.lineWidth = px;
      doc.buckets.slice(0, current).forEach((path, k) => {
        ctx.strokeStyle = bucketColor(k);
        ctx.stroke(path);
      });
    });
    while (first < index && bucketOf(first, seq.count) < current) first++;
  }
  strokeScreen(ctx, v, seq, st, first, index, colorOf);
  const arrows = Array.from({ length: st.gradient ? ORDER_BUCKETS : 1 }, () => new Path2D());
  const spacing = SPACING / v.ppm;
  let marks = 0;
  for (let i = index - 1; i >= 0 && marks < MAX_MARKS; i--) {
    if (seq.lengths[i]! < spacing || st.hidden(i) || !visible(i)) continue;
    const into = arrows[st.gradient ? bucketOf(i, seq.count) : 0]!;
    for (const m of seq.marks(i, spacing / 2, spacing, MAX_MARKS - marks)) {
      chevron(into, v, m, 6);
      marks++;
    }
  }
  ctx.lineWidth = 1;
  arrows.forEach((path, k) => {
    ctx.strokeStyle = bucketColor(k);
    ctx.stroke(path);
  });

  // Jumps between marked paths.
  if (st.jumps) {
    const jumps = new Path2D();
    for (let i = 0; i < index; i++) {
      const j = seq.jumpInto(i);
      if (!j) continue;
      const [ax, ay] = project(v, j[0][0], j[0][1]);
      const [bx, by] = project(v, j[1][0], j[1][1]);
      if (Math.max(ax, bx) < 0 || Math.min(ax, bx) > v.w || Math.max(ay, by) < 0 || Math.min(ay, by) > v.h) continue;
      jumps.moveTo(ax, ay);
      jumps.lineTo(bx, by);
    }
    ctx.setLineDash([4, 3]);
    ctx.strokeStyle = JUMP;
    ctx.stroke(jumps);
    ctx.setLineDash([]);
  }

  if (st.hover >= 0 && st.hover < seq.count) {
    const h = new Path2D();
    tracePath(h, v, p, st.hover);
    ctx.lineWidth = 3;
    ctx.strokeStyle = HOVER;
    ctx.stroke(h);
  }

  // The current path: a halo, the jump into it, denser chevrons, and where it starts and ends.
  const cur = new Path2D();
  tracePath(cur, v, p, index);
  ctx.lineWidth = 8;
  ctx.strokeStyle = HALO;
  ctx.stroke(cur);
  // Following the beam, the jump and the path are drawn as far as it has come.
  const jump = seq.jumpInto(index);
  if (jump) partJump(ctx, v, jump[0], beam?.phase === "jump" ? beam.point : jump[1], jump[1]);
  const length = seq.lengths[index]!;
  const marked = !beam || beam.phase === "end" ? length : beam.phase === "mark" ? beam.s : 0;
  const color = colorOf(index);
  const step = Math.min(40, (length * v.ppm) / 3) / v.ppm;
  const count = marked >= length ? 400 : step > 0 ? Math.min(400, Math.max(0, Math.floor((marked - step / 2) / step) + 1)) : 0;
  const heads = new Path2D();
  for (const m of seq.marks(index, step / 2, step, count)) chevron(heads, v, m, 8);
  ctx.lineWidth = 2.5;
  ctx.strokeStyle = color;
  if (marked > 0) ctx.stroke(marked >= length ? cur : traceTo(v, seq, index, marked));
  ctx.lineWidth = 2;
  ctx.stroke(heads);
  if (!seq.closed(index)) dot(ctx, v, seq.last(index), 3.5, null, color);
  dot(ctx, v, seq.first(index), 4, color, "white");
  if (beam) {
    const end = seq.finalJump();
    if (beam.phase === "end" && end) partJump(ctx, v, end[0], beam.point, end[1]);
    const [bx, by] = project(v, ...beam.point);
    ctx.beginPath();
    ctx.arc(bx, by, 9, 0, 2 * Math.PI);
    ctx.fillStyle = BEAM_GLOW;
    ctx.fill();
    dot(ctx, v, beam.point, 4.5, BEAM, "white");
  }
  ctx.lineCap = "butt";
  ctx.lineJoin = "miter";
}
