import init, { core_version, flatten_arc } from "../wasm/lvp_wasm.js";
import type { Request, Response } from "./protocol.js";

const ready = init();

function handle(req: Request): { result: unknown; transfer: Transferable[] } {
  switch (req.kind) {
    case "version":
      return { result: core_version(), transfer: [] };
    case "flattenArc": {
      const pts = flatten_arc(req.cx, req.cy, req.radius, req.startAngle, req.sweep, req.tolerance);
      return { result: pts, transfer: [pts.buffer] };
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
