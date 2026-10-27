// The marking sequence of one output group, for the preview's order inspector: path
// lengths and bounds, the jumps between paths, hit testing, and points along a path for
// direction marks. No canvas and no Svelte here, so all of it is unit tested.

import type { OrderPreview } from "../core/types.js";

export type Pt = [number, number];

/** A point on a path with the unit tangent of the direction of travel there. */
export interface Mark {
  x: number;
  y: number;
  dx: number;
  dy: number;
}

/**
 * Geometry of the segment from (x1, y1) to (x2, y2) with `bulge` (tan(sweep / 4), positive
 * counter-clockwise): a line, or an arc with centre, radius, start angle and signed sweep.
 */
export type Seg =
  | { arc: false; x1: number; y1: number; x2: number; y2: number; length: number }
  | { arc: true; x1: number; y1: number; x2: number; y2: number; length: number; cx: number; cy: number; r: number; a0: number; sweep: number };

export function seg(x1: number, y1: number, x2: number, y2: number, bulge: number): Seg {
  const chord = Math.hypot(x2 - x1, y2 - y1);
  if (bulge === 0 || chord === 0 || !Number.isFinite(bulge)) return { arc: false, x1, y1, x2, y2, length: chord };
  // Centre: chord midpoint plus the left normal times chord (1 - b^2) / (4 b).
  const off = (1 - bulge * bulge) / (4 * bulge);
  const cx = (x1 + x2) / 2 - (y2 - y1) * off;
  const cy = (y1 + y2) / 2 + (x2 - x1) * off;
  const r = Math.hypot(x1 - cx, y1 - cy);
  const sweep = 4 * Math.atan(bulge);
  return { arc: true, x1, y1, x2, y2, length: r * Math.abs(sweep), cx, cy, r, a0: Math.atan2(y1 - cy, x1 - cx), sweep };
}

/** The point at arc length `s` (0 ≤ s ≤ length) along a segment, with its tangent. */
export function segPoint(g: Seg, s: number): Mark {
  if (!g.arc) {
    const t = g.length > 0 ? s / g.length : 0;
    const dx = g.length > 0 ? (g.x2 - g.x1) / g.length : 1;
    const dy = g.length > 0 ? (g.y2 - g.y1) / g.length : 0;
    return { x: g.x1 + (g.x2 - g.x1) * t, y: g.y1 + (g.y2 - g.y1) * t, dx, dy };
  }
  const a = g.a0 + (g.length > 0 ? (s / g.length) * g.sweep : 0);
  const sign = Math.sign(g.sweep);
  return { x: g.cx + g.r * Math.cos(a), y: g.cy + g.r * Math.sin(a), dx: -sign * Math.sin(a), dy: sign * Math.cos(a) };
}

/** Distance from (x, y) to a segment. */
export function segDistance(g: Seg, x: number, y: number): number {
  if (!g.arc) {
    const vx = g.x2 - g.x1;
    const vy = g.y2 - g.y1;
    const l2 = vx * vx + vy * vy;
    const t = l2 > 0 ? Math.min(1, Math.max(0, ((x - g.x1) * vx + (y - g.y1) * vy) / l2)) : 0;
    return Math.hypot(x - (g.x1 + vx * t), y - (g.y1 + vy * t));
  }
  if (inSweep(g, Math.atan2(y - g.cy, x - g.cx))) return Math.abs(Math.hypot(x - g.cx, y - g.cy) - g.r);
  return Math.min(Math.hypot(x - g.x1, y - g.y1), Math.hypot(x - g.x2, y - g.y2));
}

/** Whether the direction at angle `a` from an arc's centre lies within its sweep. */
function inSweep(g: Seg & { arc: true }, a: number): boolean {
  const tau = 2 * Math.PI;
  // Angle travelled from the start in the arc's direction, in [0, 2π).
  const d = (((g.sweep > 0 ? a - g.a0 : g.a0 - a) % tau) + tau) % tau;
  return d <= Math.abs(g.sweep) + 1e-12;
}

/** Exact bounds of a segment, arc extremes included: [minX, minY, maxX, maxY]. */
export function segBounds(g: Seg): [number, number, number, number] {
  const b: [number, number, number, number] = [Math.min(g.x1, g.x2), Math.min(g.y1, g.y2), Math.max(g.x1, g.x2), Math.max(g.y1, g.y2)];
  if (g.arc) {
    for (let k = 0; k < 4; k++) {
      const a = (k * Math.PI) / 2;
      if (!inSweep(g, a)) continue;
      const x = g.cx + g.r * Math.cos(a);
      const y = g.cy + g.r * Math.sin(a);
      b[0] = Math.min(b[0], x);
      b[1] = Math.min(b[1], y);
      b[2] = Math.max(b[2], x);
      b[3] = Math.max(b[3], y);
    }
  }
  return b;
}

export class Sequence {
  readonly count: number;
  /** Marked length of each path (mm). */
  readonly lengths: Float64Array;
  /** Bounds of each path, four numbers per path: min x, min y, max x, max y (mm). */
  readonly bounds: Float64Array;
  /** Length of the jump into each path (mm): from the previous path's end, or from the
   * start point for the first; 0 for the first without one. */
  readonly jumps: Float64Array;
  /** Marked length of the paths up to and including each one. */
  readonly markedTo: Float64Array;
  /** Jump length up to and including the jump into each path. */
  readonly jumpedTo: Float64Array;
  #grid: Grid | null = null;

  constructor(readonly order: OrderPreview) {
    const p = order.paths;
    const n = p.closed.length;
    this.count = n;
    this.lengths = new Float64Array(n);
    this.bounds = new Float64Array(4 * n);
    this.jumps = new Float64Array(n);
    this.markedTo = new Float64Array(n);
    this.jumpedTo = new Float64Array(n);
    let marked = 0;
    let jumped = 0;
    for (let i = 0; i < n; i++) {
      let len = 0;
      const b = [Infinity, Infinity, -Infinity, -Infinity];
      for (const g of this.segments(i)) {
        len += g.length;
        const sb = segBounds(g);
        b[0] = Math.min(b[0]!, sb[0]);
        b[1] = Math.min(b[1]!, sb[1]);
        b[2] = Math.max(b[2]!, sb[2]);
        b[3] = Math.max(b[3]!, sb[3]);
      }
      this.lengths[i] = len;
      this.bounds.set(b, 4 * i);
      const j = this.jumpInto(i);
      this.jumps[i] = j ? Math.hypot(j[1][0] - j[0][0], j[1][1] - j[0][1]) : 0;
      marked += len;
      jumped += this.jumps[i]!;
      this.markedTo[i] = marked;
      this.jumpedTo[i] = jumped;
    }
  }

  /** Vertex `k` of the buffer in document mm. */
  vertex(k: number): Pt {
    const p = this.order.paths;
    return [p.originX + p.coords[2 * k]!, p.originY + p.coords[2 * k + 1]!];
  }

  /** Where path `i` starts: its seam if it is a loop. */
  first(i: number): Pt {
    return this.vertex(this.order.paths.starts[i]!);
  }

  /** Where path `i` ends: back at its seam if it is a loop. */
  last(i: number): Pt {
    const p = this.order.paths;
    return p.closed[i] === 1 ? this.first(i) : this.vertex(p.starts[i + 1]! - 1);
  }

  closed(i: number): boolean {
    return this.order.paths.closed[i] === 1;
  }

  /** Pass id of path `i`. */
  pass(i: number): string {
    return this.order.passes[this.order.passOf[i]!]!;
  }

  /** The straight jump into path `i`: from the previous path's end, or from the start point. */
  jumpInto(i: number): [Pt, Pt] | null {
    if (i < 0 || i >= this.count) return null;
    const from = i > 0 ? this.last(i - 1) : this.order.start;
    return from ? [from, this.first(i)] : null;
  }

  /** The jump from the last path to the end point, if there is one. */
  finalJump(): [Pt, Pt] | null {
    return this.count && this.order.end ? [this.last(this.count - 1), this.order.end] : null;
  }

  /** Segments of path `i` in the direction of travel. */
  *segments(i: number): Generator<Seg> {
    const p = this.order.paths;
    const a = p.starts[i]!;
    const n = p.starts[i + 1]! - a;
    const count = p.closed[i] === 1 ? n : n - 1;
    for (let s = 0; s < count; s++) {
      const k = a + s;
      const m = s + 1 < n ? k + 1 : a;
      const [x1, y1] = this.vertex(k);
      const [x2, y2] = this.vertex(m);
      yield seg(x1, y1, x2, y2, p.bulges[k]!);
    }
  }

  /**
   * Points along path `i` at arc lengths `first`, `first + spacing`, … (at most `limit`),
   * with the direction of travel there.
   */
  marks(i: number, first: number, spacing: number, limit = Infinity): Mark[] {
    const out: Mark[] = [];
    if (!(spacing > 0)) return out;
    let s = first;
    let base = 0;
    for (const g of this.segments(i)) {
      while (s <= base + g.length && out.length < limit) {
        out.push(segPoint(g, Math.max(0, s - base)));
        s += spacing;
      }
      if (out.length >= limit) break;
      base += g.length;
    }
    return out;
  }

  /** The path nearest to (x, y) within `tol` mm, or -1; the later of equally near ones. */
  pathAt(x: number, y: number, tol: number): number {
    const grid = (this.#grid ??= new Grid(this));
    let best = -1;
    let bestD = tol;
    for (const k of grid.near(x, y, tol)) {
      const i = grid.pathOf[k]!;
      const d = segDistance(grid.seg(k), x, y);
      if (d < bestD || (d === bestD && i > best)) {
        best = i;
        bestD = d;
      }
    }
    return best;
  }
}

/**
 * Uniform grid of segment bounds over the sequence, built on the first hit test. Segments
 * are short against a path's bounds (a ring's bounds cover its whole inside), so they
 * are what is indexed. Cells list segments in compressed rows: `cell[c] .. cell[c + 1]`.
 */
class Grid {
  readonly pathOf: Uint32Array;
  readonly #segs: Seg[] = [];
  readonly #x0: number;
  readonly #y0: number;
  readonly #cell: number;
  readonly #nx: number;
  readonly #ny: number;
  readonly #rows: Uint32Array;
  readonly #items: Uint32Array;

  constructor(s: Sequence) {
    const paths: number[] = [];
    let bx0 = Infinity;
    let by0 = Infinity;
    let bx1 = -Infinity;
    let by1 = -Infinity;
    for (let i = 0; i < s.count; i++) {
      for (const g of s.segments(i)) {
        this.#segs.push(g);
        paths.push(i);
      }
      bx0 = Math.min(bx0, s.bounds[4 * i]!);
      by0 = Math.min(by0, s.bounds[4 * i + 1]!);
      bx1 = Math.max(bx1, s.bounds[4 * i + 2]!);
      by1 = Math.max(by1, s.bounds[4 * i + 3]!);
    }
    this.pathOf = Uint32Array.from(paths);
    const m = this.#segs.length;
    const w = Math.max(bx1 - bx0, 1e-9);
    const h = Math.max(by1 - by0, 1e-9);
    // About four segments per cell, at most 1024 cells a side.
    this.#cell = Math.max(Math.sqrt((w * h) / Math.max(m / 4, 1)), w / 1024, h / 1024);
    this.#x0 = m ? bx0 : 0;
    this.#y0 = m ? by0 : 0;
    this.#nx = Math.max(1, Math.ceil(w / this.#cell));
    this.#ny = Math.max(1, Math.ceil(h / this.#cell));
    const cells = this.#nx * this.#ny;
    const ranges = this.#segs.map((g) => this.#range(segBounds(g), 0));
    const counts = new Uint32Array(cells + 1);
    for (const [x0, y0, x1, y1] of ranges) for (let cy = y0; cy <= y1; cy++) for (let cx = x0; cx <= x1; cx++) counts[cy * this.#nx + cx]!++;
    this.#rows = new Uint32Array(cells + 1);
    for (let c = 0; c < cells; c++) this.#rows[c + 1] = this.#rows[c]! + counts[c]!;
    this.#items = new Uint32Array(this.#rows[cells]!);
    const fill = this.#rows.slice(0, cells);
    ranges.forEach(([x0, y0, x1, y1], k) => {
      for (let cy = y0; cy <= y1; cy++) for (let cx = x0; cx <= x1; cx++) this.#items[fill[cy * this.#nx + cx]!++] = k;
    });
  }

  seg(k: number): Seg {
    return this.#segs[k]!;
  }

  /** Cells covered by bounds grown by `pad`, clamped to the grid: [x0, y0, x1, y1]. */
  #range(b: [number, number, number, number], pad: number): [number, number, number, number] {
    const cx = (v: number) => Math.min(this.#nx - 1, Math.max(0, Math.floor((v - this.#x0) / this.#cell)));
    const cy = (v: number) => Math.min(this.#ny - 1, Math.max(0, Math.floor((v - this.#y0) / this.#cell)));
    return [cx(b[0] - pad), cy(b[1] - pad), cx(b[2] + pad), cy(b[3] + pad)];
  }

  /** Segments in the cells within `tol` of (x, y); a segment may come more than once. */
  *near(x: number, y: number, tol: number): Generator<number> {
    if (!this.#segs.length) return;
    const [x0, y0, x1, y1] = this.#range([x, y, x, y], tol);
    for (let cy = y0; cy <= y1; cy++) {
      for (let cx = x0; cx <= x1; cx++) {
        const c = cy * this.#nx + cx;
        for (let j = this.#rows[c]!; j < this.#rows[c + 1]!; j++) yield this.#items[j]!;
      }
    }
  }
}

// ---- colours by place in the order -----------------------------------------------------

/** Stops of the order ramp, early to late: blue, cyan, green, orange, red. Bright enough on
 * the light and the dark canvas, and away from the jumps' magenta. */
const RAMP: [number, number, number][] = [
  [59, 76, 192],
  [31, 158, 201],
  [44, 162, 95],
  [240, 138, 36],
  [214, 39, 40],
];

/** Number of distinct colours the finished paths are drawn in (one batch each). */
export const ORDER_BUCKETS = 48;

/** Ramp colour at `t` in [0, 1]. */
export function rampColor(t: number): [number, number, number] {
  const u = Math.min(1, Math.max(0, t)) * (RAMP.length - 1);
  const k = Math.min(RAMP.length - 2, Math.floor(u));
  const f = u - k;
  const a = RAMP[k]!;
  const b = RAMP[k + 1]!;
  return [0, 1, 2].map((c) => Math.round(a[c]! + (b[c]! - a[c]!) * f)) as [number, number, number];
}

/** Bucket of path `i` of `count`: its place in the order, early (0) to late. */
export function bucketOf(i: number, count: number): number {
  return count > 1 ? Math.min(ORDER_BUCKETS - 1, Math.floor((i / (count - 1)) * ORDER_BUCKETS)) : 0;
}

/** CSS colour of each bucket. */
export const BUCKET_COLORS: string[] = Array.from({ length: ORDER_BUCKETS }, (_, b) => {
  const [r, g, bl] = rampColor(ORDER_BUCKETS > 1 ? b / (ORDER_BUCKETS - 1) : 0);
  return `rgb(${r}, ${g}, ${bl})`;
});

/** The ramp as a CSS gradient, for the timeline's track. */
export const RAMP_CSS = `linear-gradient(to right, ${RAMP.map((c) => `rgb(${c.join(", ")})`).join(", ")})`;
