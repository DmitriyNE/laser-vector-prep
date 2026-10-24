import { transferables, type Payload, type Request, type Response, type ResultOf } from "./protocol.js";

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void };

/** Thrown when the wasm instance trapped; the worker has been replaced automatically. */
export class WorkerCrashed extends Error {
  constructor(message: string) {
    super(`geometry worker crashed and was restarted: ${message}`);
    this.name = "WorkerCrashed";
  }
}

/**
 * Promise façade over the geometry worker. Each request carries its own id; responses for
 * unknown ids (e.g. after `restart()`) are dropped, so a stale computation can never
 * resolve a newer call. A fatal worker error (wasm trap) rejects everything in flight with
 * `WorkerCrashed`, replaces the worker, and notifies `onCrash` so the owner can re-import.
 */
export class GeometryWorker {
  #worker!: Worker;
  #pending = new Map<number, Pending>();
  #nextId = 1;
  onCrash: ((message: string) => void) | null = null;

  constructor() {
    this.#spawn();
  }

  #spawn() {
    this.#worker = new Worker(new URL("./geometry.worker.ts", import.meta.url), { type: "module" });
    this.#worker.onmessage = (ev: MessageEvent<Response>) => {
      const res = ev.data;
      if (!res.ok && res.fatal) {
        const msg = res.error;
        this.#worker.terminate();
        this.#failAll(new WorkerCrashed(msg));
        this.#spawn();
        this.onCrash?.(msg);
        return;
      }
      const p = this.#pending.get(res.rid);
      if (!p) return; // stale or cancelled
      this.#pending.delete(res.rid);
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
    const rid = this.#nextId++;
    const req: Request = { rid, payload };
    const transfer = transferables(payload);
    return new Promise((resolve, reject) => {
      this.#pending.set(rid, { resolve: resolve as (v: unknown) => void, reject });
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
