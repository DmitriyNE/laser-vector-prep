import { describe, expect, it } from "vitest";
import type { OrderPreview } from "../core/types.js";
import { BUCKET_COLORS, bucketOf, ORDER_BUCKETS, rampColor, seg, segBounds, segDistance, segPoint, Sequence } from "./sequence.js";

type Path = { pts: [number, number][]; bulges?: number[]; closed: boolean; pass?: number };

/** An order buffer as the worker sends it, with its origin at the lower-left corner. */
function order(paths: Path[], start: [number, number] | null = null, end: [number, number] | null = null): OrderPreview {
  const all = paths.flatMap((p) => p.pts);
  const ox = Math.min(...all.map((p) => p[0]));
  const oy = Math.min(...all.map((p) => p[1]));
  const starts = [0];
  for (const p of paths) starts.push(starts.at(-1)! + p.pts.length);
  return {
    paths: {
      originX: ox,
      originY: oy,
      coords: Float32Array.from(all.flatMap(([x, y]) => [x - ox, y - oy])),
      bulges: Float32Array.from(paths.flatMap((p) => p.bulges ?? p.pts.map(() => 0))),
      starts: Uint32Array.from(starts),
      closed: Uint8Array.from(paths.map((p) => (p.closed ? 1 : 0))),
    },
    passOf: Uint32Array.from(paths.map((p) => p.pass ?? 0)),
    passes: ["pass-a", "pass-b"],
    start,
    end,
  };
}

/** A small deterministic generator (mulberry32), so failures reproduce. */
function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const close = (a: number, b: number, eps = 1e-6) => expect(Math.abs(a - b)).toBeLessThan(eps);
const QUARTER = Math.tan(Math.PI / 8);

describe("segments", () => {
  it("measures, walks and bounds an arc in its direction of travel", () => {
    // Counter-clockwise quarter circle about the origin, from (1, 0) to (0, 1).
    const ccw = seg(1, 0, 0, 1, QUARTER);
    expect(ccw.arc).toBe(true);
    close(ccw.length, Math.PI / 2);
    const m = segPoint(ccw, Math.PI / 4);
    close(m.x, Math.SQRT1_2);
    close(m.y, Math.SQRT1_2);
    close(m.dx, -Math.SQRT1_2);
    close(m.dy, Math.SQRT1_2);
    // The same chord clockwise bends the other way, about (1, 1).
    const cw = segPoint(seg(1, 0, 0, 1, -QUARTER), Math.PI / 4);
    close(cw.x, 1 - Math.SQRT1_2);
    close(cw.y, 1 - Math.SQRT1_2);
    close(cw.dx, -Math.SQRT1_2);
    close(cw.dy, Math.SQRT1_2);
    // A half circle above the x axis reaches y = 1.
    const half = segBounds(seg(1, 0, -1, 0, 1));
    half.forEach((v, k) => close(v, [-1, 0, 1, 1][k]!));
    close(segDistance(seg(1, 0, -1, 0, 1), 0, 3), 2);
    close(segDistance(seg(1, 0, -1, 0, 1), 0, -0.5), Math.hypot(1, 0.5));
    close(segDistance(seg(0, 0, 2, 0, 0), 1, 1), 1);
  });
});

describe("Sequence", () => {
  const square: Path = { pts: [[0, 0], [1, 0], [1, 1], [0, 1]], closed: true };
  const line: Path = { pts: [[3, 0], [3, 4]], closed: false, pass: 1 };

  it("measures paths and the jumps between them", () => {
    const s = new Sequence(order([square, line], [-1, 0], [10, 10]));
    expect(s.count).toBe(2);
    close(s.lengths[0]!, 4);
    close(s.lengths[1]!, 4);
    expect(s.first(0)).toEqual([0, 0]);
    expect(s.last(0)).toEqual([0, 0]); // a loop ends at its seam
    expect(s.last(1)).toEqual([3, 4]);
    expect(s.jumpInto(0)).toEqual([[-1, 0], [0, 0]]);
    expect(s.jumpInto(1)).toEqual([[0, 0], [3, 0]]);
    expect(s.finalJump()).toEqual([[3, 4], [10, 10]]);
    close(s.jumps[0]!, 1);
    close(s.jumps[1]!, 3);
    close(s.markedTo[1]!, 8);
    close(s.jumpedTo[1]!, 4);
    expect([s.pass(0), s.pass(1)]).toEqual(["pass-a", "pass-b"]);
    expect([...s.bounds.slice(0, 4)]).toEqual([0, 0, 1, 1]);
    // Without assumed points the first path has no jump into it and the last none out.
    const bare = new Sequence(order([square, line]));
    expect(bare.jumpInto(0)).toBeNull();
    expect(bare.finalJump()).toBeNull();
    close(bare.jumpedTo[1]!, 3);
  });

  it("places direction marks along the path, in its direction", () => {
    const s = new Sequence(order([{ pts: [[0, 0], [10, 0]], closed: false }]));
    const m = s.marks(0, 1, 2);
    expect(m.map((p) => Math.round(p.x * 1e6) / 1e6)).toEqual([1, 3, 5, 7, 9]);
    expect(m.every((p) => p.dx === 1 && p.dy === 0)).toBe(true);
    expect(s.marks(0, 1, 2, 3)).toHaveLength(3);
    // A clockwise unit circle as two half arcs: marks lie on it and turn clockwise.
    const circle = new Sequence(order([{ pts: [[1, 0], [-1, 0]], bulges: [-1, -1], closed: true }]));
    close(circle.lengths[0]!, 2 * Math.PI);
    const marks = circle.marks(0, 0.1, 0.5);
    expect(marks).toHaveLength(13);
    for (const p of marks) {
      close(Math.hypot(p.x, p.y), 1);
      close(p.x * p.dx + p.y * p.dy, 0); // tangent
      expect(p.x * p.dy - p.y * p.dx).toBeLessThan(0); // clockwise
    }
  });

  it("finds the nearest path under the pointer as a brute-force search does", () => {
    const next = rng(7);
    for (let trial = 0; trial < 60; trial++) {
      const paths: Path[] = Array.from({ length: 1 + Math.floor(next() * 40) }, () => {
        const n = 2 + Math.floor(next() * 5);
        const x0 = next() * 50;
        const y0 = next() * 20;
        const size = next() < 0.2 ? 20 : 2;
        const pts = Array.from({ length: n }, () => [x0 + next() * size, y0 + next() * size] as [number, number]);
        return { pts, bulges: pts.map(() => (next() < 0.5 ? 0 : next() * 2 - 1)), closed: next() < 0.5 };
      });
      const s = new Sequence(order(paths));
      for (let q = 0; q < 50; q++) {
        const x = next() * 60 - 5;
        const y = next() * 30 - 5;
        const tol = next() * 2;
        let want = Infinity;
        for (let i = 0; i < s.count; i++) for (const g of s.segments(i)) want = Math.min(want, segDistance(g, x, y));
        const got = s.pathAt(x, y, tol);
        if (want >= tol) expect(got).toBe(-1);
        else {
          expect(got).toBeGreaterThanOrEqual(0);
          const d = Math.min(...[...s.segments(got)].map((g) => segDistance(g, x, y)));
          close(d, want, 1e-9);
        }
      }
    }
  });
});

describe("order colours", () => {
  it("run from the first ramp stop to the last, in order", () => {
    expect(rampColor(0)).toEqual([59, 76, 192]);
    expect(rampColor(1)).toEqual([214, 39, 40]);
    expect(bucketOf(0, 1000)).toBe(0);
    expect(bucketOf(999, 1000)).toBe(ORDER_BUCKETS - 1);
    expect(bucketOf(0, 1)).toBe(0);
    let prev = 0;
    for (let i = 0; i < 1000; i++) {
      const b = bucketOf(i, 1000);
      expect(b).toBeGreaterThanOrEqual(prev);
      prev = b;
    }
    expect(new Set(BUCKET_COLORS).size).toBe(ORDER_BUCKETS);
  });
});
