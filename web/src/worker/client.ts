import type { Payload, Request, Response, ResultOf } from "./protocol.js";

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void };

/**
 * Promise façade over the geometry worker. Each request carries an id; responses for
 * unknown ids (e.g. after `restart()`) are dropped, so a stale computation can never
 * resolve a newer call. Cancellation is by `restart()`, which terminates the worker.
 */
export class GeometryWorker {
  #worker!: Worker;
  #pending = new Map<number, Pending>();
  #nextId = 1;

  constructor() {
    this.#spawn();
  }

  #spawn() {
    this.#worker = new Worker(new URL("./geometry.worker.ts", import.meta.url), { type: "module" });
    this.#worker.onmessage = (ev: MessageEvent<Response>) => {
      const res = ev.data;
      const p = this.#pending.get(res.id);
      if (!p) return; // stale or cancelled
      this.#pending.delete(res.id);
      if (res.ok) p.resolve(res.result);
      else p.reject(new Error(res.error));
    };
    this.#worker.onerror = (ev) => {
      this.#failAll(new Error(ev.message || "geometry worker crashed"));
    };
  }

  #failAll(err: Error) {
    for (const p of this.#pending.values()) p.reject(err);
    this.#pending.clear();
  }

  call<P extends Payload>(payload: P): Promise<ResultOf<P["kind"]>> {
    const id = this.#nextId++;
    const req: Request = { ...payload, id };
    // Large input buffers are transferred, not copied.
    const transfer = "points" in req ? [req.points.buffer] : [];
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.#worker.postMessage(req, { transfer });
    });
  }

  /** Abort everything in flight and start a fresh worker. */
  restart() {
    this.#worker.terminate();
    this.#failAll(new Error("cancelled"));
    this.#spawn();
  }

  dispose() {
    this.#worker.terminate();
    this.#failAll(new Error("disposed"));
  }
}

/**
 * Keeps only the most recent async result: calls superseded before they resolve are
 * reported as `undefined`. Use this for preview updates so an older computation can
 * never replace a newer one.
 */
export function latestOnly<T>() {
  let current = 0;
  return async (p: Promise<T>): Promise<T | undefined> => {
    const token = ++current;
    const v = await p;
    return token === current ? v : undefined;
  };
}
