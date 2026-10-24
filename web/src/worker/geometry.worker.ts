import init, { bbox_of, core_version, flatten_arc, initThreadPool, thread_count } from "../wasm/lvp_wasm.js";
import type { Request, Response } from "./protocol.js";

// Shared-memory wasm: instantiate, then start the rayon pool (one Web Worker per thread).
const ready = (async () => {
  await init();
  await initThreadPool(navigator.hardwareConcurrency);
})();

function handle(req: Request): { result: unknown; transfer: Transferable[] } {
  switch (req.kind) {
    case "version":
      return { result: core_version(), transfer: [] };
    case "threads":
      return { result: thread_count(), transfer: [] };
    case "flattenArc": {
      const pts = flatten_arc(req.cx, req.cy, req.radius, req.startAngle, req.sweep, req.tolerance);
      return { result: pts, transfer: [pts.buffer] };
    }
    case "bbox": {
      const b = bbox_of(req.points);
      return { result: b, transfer: b ? [b.buffer] : [] };
    }
  }
}

self.onmessage = async (ev: MessageEvent<Request>) => {
  const req = ev.data;
  try {
    await ready;
    const { result, transfer } = handle(req);
    const res: Response = { id: req.id, ok: true, result };
    self.postMessage(res, { transfer });
  } catch (e) {
    const res: Response = { id: req.id, ok: false, error: e instanceof Error ? e.message : String(e) };
    self.postMessage(res);
  }
};
