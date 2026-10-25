import init, * as wasm from "../wasm/lvp_wasm.js";
import type { Heatmap, Preview } from "../core/types.js";
import { CANCELLED, type Payload, type Request, type Response } from "./protocol.js";

// Shared-memory wasm: instantiate, then start the rayon pool (one Web Worker per thread).
let memory: WebAssembly.Memory;
const ready = (async () => {
  memory = (await init()).memory;
  await wasm.initThreadPool(navigator.hardwareConcurrency);
  wasm.enable_parallelism();
})();

function preview(p: wasm.Preview | undefined): { result: Preview | null; transfer: Transferable[] } {
  if (!p) return { result: null, transfer: [] };
  const out: Preview = { coords: p.coords, bulges: p.bulges, starts: p.starts, closed: p.closed };
  p.free();
  return { result: out, transfer: [out.coords.buffer, out.bulges.buffer, out.starts.buffer, out.closed.buffer] };
}

function handle(req: Payload): { result: unknown; transfer: Transferable[] } {
  switch (req.kind) {
    case "version":
      return { result: wasm.core_version(), transfer: [] };
    case "threads":
      return { result: wasm.thread_count(), transfer: [] };
    case "control": {
      const buffer = memory.buffer;
      if (!(buffer instanceof SharedArrayBuffer)) throw new Error("wasm memory is not shared");
      return { result: { buffer, offset: wasm.control_block() }, transfer: [] };
    }
    case "presets":
      return { result: JSON.parse(wasm.builtin_presets_json()), transfer: [] };
    case "addSource":
      return { result: JSON.parse(wasm.add_source(req.name, req.bytes, req.epoch)), transfer: [] };
    case "replaceSource":
      return { result: JSON.parse(wasm.replace_source(req.index, req.bytes, req.epoch)), transfer: [] };
    case "acceptedExtensions":
      return { result: JSON.parse(wasm.accepted_extensions_json()), transfer: [] };
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
      return { result: JSON.parse(wasm.evaluate(req.epoch)), transfer: [] };
    case "previewPass":
      return preview(wasm.preview_pass(req.id));
    case "previewLayer":
      return preview(wasm.preview_layer(req.id));
    case "previewSource":
      return preview(wasm.preview_source(req.file, req.layer));
    case "previewJumps":
      return preview(wasm.preview_jumps(req.id));
    case "heatmap": {
      const h = wasm.heatmap(req.id);
      if (!h) return { result: null, transfer: [] };
      const out: Heatmap = { originX: h.origin_x, originY: h.origin_y, cellMm: h.cell_mm, width: h.width, height: h.height, counts: h.counts, mask: h.mask };
      h.free();
      return { result: out, transfer: [out.counts.buffer, out.mask.buffer] };
    }
    case "exportPdf": {
      const bytes = wasm.export_pdf();
      return { result: bytes, transfer: [bytes.buffer] };
    }
    case "exportSvg": {
      const bytes = wasm.export_svg();
      return { result: bytes, transfer: [bytes.buffer] };
    }
    case "exportDxf": {
      const bytes = wasm.export_dxf();
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
  const { rid, payload } = ev.data;
  try {
    await ready;
    const { result, transfer } = handle(payload);
    const res: Response = { rid, ok: true, result };
    self.postMessage(res, { transfer });
  } catch (e) {
    // Full detail (including wasm stack frames) stays in the worker console. A wasm trap
    // (RuntimeError) leaves the instance in an unknown state: tell the client to replace it.
    const error = e instanceof Error ? e.message : String(e);
    if (!CANCELLED.includes(error)) console.error(`geometry worker: ${payload.kind} failed`, e);
    const fatal = e instanceof WebAssembly.RuntimeError;
    const res: Response = { rid, ok: false, error, fatal };
    self.postMessage(res);
  }
};
