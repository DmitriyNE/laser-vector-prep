// Message types between the main thread and the geometry worker.
// Geometry crosses the boundary as interleaved Float64Array [x0, y0, x1, y1, ...] in mm.

export type Payload =
  | { kind: "version" }
  | {
      kind: "flattenArc";
      cx: number;
      cy: number;
      radius: number;
      startAngle: number;
      sweep: number;
      tolerance: number;
    };

export type Kind = Payload["kind"];

export type ResultOf<K extends Kind> = K extends "version"
  ? string
  : K extends "flattenArc"
    ? Float64Array
    : never;

/** What actually travels to the worker: a payload tagged with a request id. */
export type Request = Payload & { id: number };

export type Response =
  | { id: number; ok: true; result: unknown }
  | { id: number; ok: false; error: string };
