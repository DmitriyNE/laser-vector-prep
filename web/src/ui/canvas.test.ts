import { describe, expect, it } from "vitest";
import type { Preview } from "../core/types.js";
import { seg, segPoint } from "../state/sequence.js";
import { documentSpaceOk, project, traceDocument, tracePath, type View } from "./canvas.js";

/** Records the arcs a tracer adds, standing in for a canvas path. */
class Recorder {
  arcs: { x: number; y: number; r: number; a0: number; a1: number; ccw: boolean }[] = [];
  moveTo() {}
  lineTo() {}
  closePath() {}
  arc(x: number, y: number, r: number, a0: number, a1: number, ccw = false) {
    this.arcs.push({ x, y, r, a0, a1, ccw });
  }
}

/** Middle of a recorded arc, as the canvas would draw it (in that arc's own space). */
function middle(a: Recorder["arcs"][number]): [number, number] {
  const tau = 2 * Math.PI;
  const sweep = ((((a.ccw ? a.a0 - a.a1 : a.a1 - a.a0) % tau) + tau) % tau) * (a.ccw ? -1 : 1);
  const m = a.a0 + sweep / 2;
  return [a.x + a.r * Math.cos(m), a.y + a.r * Math.sin(m)];
}

function buffer(x1: number, y1: number, x2: number, y2: number, bulge: number, ox: number, oy: number): Preview {
  return {
    originX: ox,
    originY: oy,
    coords: Float32Array.from([x1 - ox, y1 - oy, x2 - ox, y2 - oy]),
    bulges: Float32Array.from([bulge, 0]),
    starts: Uint32Array.from([0, 2]),
    closed: Uint8Array.from([0]),
  };
}

describe("tracing arcs", () => {
  it("draws each arc through its true middle, in screen and in document space", () => {
    let seed = 3;
    const next = () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
    const v: View = { cx: 5, cy: -3, ppm: 40, w: 800, h: 600 };
    for (let k = 0; k < 500; k++) {
      const [ox, oy] = [next() * 20 - 10, next() * 20 - 10];
      const x1 = ox + next() * 4;
      const y1 = oy + next() * 4;
      const x2 = ox + next() * 4;
      const y2 = oy + next() * 4;
      let bulge = next() * 2 - 1;
      if (Math.abs(bulge) < 0.05) bulge = 0.05 * Math.sign(bulge || 1);
      const p = buffer(x1, y1, x2, y2, bulge, ox, oy);
      // The model's middle, from the (f32) coordinates the buffer holds.
      const [px1, py1] = [ox + p.coords[0]!, oy + p.coords[1]!];
      const [px2, py2] = [ox + p.coords[2]!, oy + p.coords[3]!];
      if (Math.hypot(px2 - px1, py2 - py1) < 0.5) continue; // screen tracing draws flat arcs as lines
      const g = seg(px1, py1, px2, py2, p.bulges[0]!);
      const want = segPoint(g, g.length / 2);
      const [wx, wy] = project(v, want.x, want.y);

      const screen = new Recorder();
      tracePath(screen as unknown as CanvasPath, v, p, 0);
      expect(screen.arcs).toHaveLength(1);
      const [sx, sy] = middle(screen.arcs[0]!);
      expect(Math.hypot(sx - wx, sy - wy)).toBeLessThan(1e-6);

      const doc = new Recorder();
      traceDocument(doc as unknown as Path2D, p, 0);
      expect(doc.arcs).toHaveLength(1);
      const [dx, dy] = middle(doc.arcs[0]!);
      const [tx, ty] = project(v, ox + dx, oy + dy);
      expect(Math.hypot(tx - wx, ty - wy)).toBeLessThan(1e-6);
    }
  });

  it("keeps paths in document space only while single precision is fine enough", () => {
    const ctx = { getTransform: () => ({ a: 2 }) } as unknown as CanvasRenderingContext2D;
    const v = (ppm: number): View => ({ cx: 0, cy: 0, ppm, w: 100, h: 100 });
    expect(documentSpaceOk(ctx, v(100), 50)).toBe(true);
    expect(documentSpaceOk(ctx, v(1e5), 50)).toBe(false);
  });
});
