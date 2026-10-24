import init, * as wasm from "../wasm/lvp_wasm.js";
import type { Preview } from "../core/types.js";
import type { Request, Response } from "./protocol.js";

// Shared-memory wasm: instantiate, then start the rayon pool (one Web Worker per thread).
const ready = (async () => {
  await init();
  await wasm.initThreadPool(navigator.hardwareConcurrency);
})();

function preview(p: wasm.Preview | undefined): { result: Preview | null; transfer: Transferable[] } {
  if (!p) return { result: null, transfer: [] };
  const out: Preview = { coords: p.coords, starts: p.starts, closed: p.closed };
  p.free();
  return { result: out, transfer: [out.coords.buffer, out.starts.buffer, out.closed.buffer] };
}

function handle(req: Request): { result: unknown; transfer: Transferable[] } {
  switch (req.kind) {
    case "version":
      return { result: wasm.core_version(), transfer: [] };
    case "threads":
      return { result: wasm.thread_count(), transfer: [] };
    case "presets":
      return { result: JSON.parse(wasm.builtin_presets_json()), transfer: [] };
    case "addDxf":
      return { result: JSON.parse(wasm.add_dxf(req.name, req.bytes)), transfer: [] };
    case "replaceDxf":
      return { result: JSON.parse(wasm.replace_dxf(req.index, req.bytes)), transfer: [] };
    case "removeSource":
      wasm.remove_source(req.index);
      return { result: undefined, transfer: [] };
    case "sources":
      return { result: JSON.parse(wasm.sources_json()), transfer: [] };
    case "setProfile":
      wasm.set_profile_json(JSON.stringify(req.profile));
      return { result: undefined, transfer: [] };
    case "getProfile":
      return { result: JSON.parse(wasm.profile_json()), transfer: [] };
    case "validateProfile":
      return { result: JSON.parse(wasm.validate_profile_json(JSON.stringify(req.profile))), transfer: [] };
    case "evaluate":
      return { result: JSON.parse(wasm.evaluate()), transfer: [] };
    case "previewPass":
      return preview(wasm.preview_pass(req.id, req.toleranceMm));
    case "previewLayer":
      return preview(wasm.preview_layer(req.id, req.toleranceMm));
    case "previewSource":
      return preview(wasm.preview_source(req.file, req.layer, req.toleranceMm));
    case "previewJumps":
      return preview(wasm.preview_jumps(req.id));
    case "exportPdf": {
      const bytes = wasm.export_pdf();
      return { result: bytes, transfer: [bytes.buffer] };
    }
    case "exportPdfPerGroup": {
      const files = wasm.export_pdf_per_group().map((f) => {
        const out = { group: f.group, name: f.name, bytes: f.bytes };
        f.free();
        return out;
      });
      return { result: files, transfer: files.map((f) => f.bytes.buffer) };
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
