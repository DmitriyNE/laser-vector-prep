<script lang="ts">
  import type { Heatmap, Preview } from "../core/types.js";
  import { rgbCss, type ProjectStore } from "../state/project.svelte.js";
  import { Sequence } from "../state/sequence.js";
  import { documentPath, documentSpaceOk, extent, inDocument, project, segment, tracePath, type View } from "./canvas.js";
  import { drawOrder, drawOrderBase, type OrderState } from "./orderDraw.js";
  import OrderTimeline from "./OrderTimeline.svelte";

  let { store }: { store: ProjectStore } = $props();

  let canvas: HTMLCanvasElement;
  // View: document mm at the canvas centre and pixels per mm.
  let cx = $state(0);
  let cy = $state(0);
  let ppm = $state(10);
  let fitted = false;
  let cursorMm: [number, number] | null = $state(null);

  /**
   * `group`: the output group a pass's paths or jumps belong to. `doc` and `extent`: the
   * paths in document space and their extent, made on first draw (see canvas.ts).
   */
  type Item = { key: string; preview: Preview; color: string; width: number; dash: number[]; fill?: string; group?: string; doc?: Path2D; extent?: number };
  let items: Item[] = [];
  /** The order inspector's sequence, and the path under the pointer (-1: none). */
  let order: { group: string; seq: Sequence; revision: number } | null = $state.raw(null);
  let hover = $state(-1);
  let hoverAt: { x: number; y: number } | null = $state(null);
  /** The scene under the order overlay while only the overlay changes, and what it shows. */
  let base: { canvas: HTMLCanvasElement; key: string } | null = null;
  let itemsVersion = 0;
  /** The group the inspector steps through: the chosen one, else the first with paths. */
  const orderGroup = $derived.by(() => {
    const gs = store.evaluation?.groups ?? [];
    return gs.find((g) => g.id === store.orderGroup && g.paths > 0)?.id ?? gs.find((g) => g.paths > 0)?.id ?? null;
  });
  const inspecting = () => (store.showOrder && order ? order : null);
  /** Heatmap images in document mm: drawn under the geometry when enabled. */
  type HeatImage = { key: string; canvas: HTMLCanvasElement; x: number; top: number; wMm: number; hMm: number };
  let heatImages: HeatImage[] = [];
  let fetchToken = 0;

  /** Turn a coverage raster into an image: protected cells tinted, coverage from amber to red. */
  function heatImage(key: string, h: Heatmap): HeatImage {
    const canvas = document.createElement("canvas");
    canvas.width = h.width;
    canvas.height = h.height;
    const ctx = canvas.getContext("2d")!;
    const img = ctx.createImageData(h.width, h.height);
    let peak = 1;
    for (let i = 0; i < h.counts.length; i++) if (h.mask[i] && h.counts[i]! > peak) peak = h.counts[i]!;
    for (let iy = 0; iy < h.height; iy++) {
      const row = h.height - 1 - iy; // raster y goes up, image rows go down
      for (let ix = 0; ix < h.width; ix++) {
        const i = iy * h.width + ix;
        const o = (row * h.width + ix) * 4;
        const c = h.counts[i]!;
        if (h.mask[i]) {
          if (c > 0) {
            const t = Math.min(1, c / peak);
            img.data[o] = 255;
            img.data[o + 1] = Math.round(180 * (1 - t));
            img.data[o + 2] = 0;
            img.data[o + 3] = Math.round(110 + 120 * t);
          } else {
            img.data[o] = 255;
            img.data[o + 1] = 200;
            img.data[o + 2] = 0;
            img.data[o + 3] = 28;
          }
        } else if (c > 0) {
          img.data[o] = 128;
          img.data[o + 1] = 128;
          img.data[o + 2] = 128;
          img.data[o + 3] = 22;
        }
      }
    }
    ctx.putImageData(img, 0, 0);
    return { key, canvas, x: h.originX, top: h.originY + h.height * h.cellMm, wMm: h.width * h.cellMm, hMm: h.height * h.cellMm };
  }

  function toScreen(x: number, y: number, w: number, h: number): [number, number] {
    return project({ cx, cy, ppm, w, h }, x, y);
  }

  function toMm(px: number, py: number, w: number, h: number): [number, number] {
    return [cx + (px - w / 2) / ppm, cy - (py - h / 2) / ppm];
  }

  type Bounds = [number, number, number, number];

  /** Bounds of everything imported and evaluated (mm). */
  function sceneBounds(): Bounds | null {
    let b: Bounds | null = null;
    const grow = (x: Bounds | null) => {
      if (!x) return;
      b = b ? [Math.min(b[0], x[0]), Math.min(b[1], x[1]), Math.max(b[2], x[2]), Math.max(b[3], x[3])] : [...x];
    };
    for (const s of store.sources) grow(s.boundsMm);
    if (store.evaluation) grow(store.evaluation.boundsMm);
    return b;
  }

  function fit() {
    const bb = sceneBounds();
    if (!bb || !canvas) return;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    const bw = Math.max(bb[2] - bb[0], 1e-3);
    const bh = Math.max(bb[3] - bb[1], 1e-3);
    ppm = Math.min(w / bw, h / bh) * 0.9;
    cx = (bb[0] + bb[2]) / 2;
    cy = (bb[1] + bb[3]) / 2;
    fitted = true;
    draw();
  }

  async function refetch() {
    const token = ++fetchToken;
    const revision = store.previewRevision;
    const next: Item[] = [];
    const ev = store.evaluation;
    if (store.showSources) {
      for (const s of store.sources) {
        for (const l of s.layers) {
          const key = `${s.index}:${l.name}`;
          if (store.hiddenSourceLayers.has(key) || l.closedPaths + l.openPaths + l.hatches === 0) continue;
          const p = await store.previewSource(s.index, l.name);
          if (p) next.push({ key: `src:${key}`, preview: p, color: "rgba(128,128,128,0.9)", width: 1, dash: [] });
        }
      }
    }
    if (ev && store.showLayers) {
      for (const l of ev.layers) {
        if (store.hiddenLayers.has(l.id)) continue;
        const p = await store.preview("layer", l.id);
        if (p) next.push({ key: `layer:${l.id}`, preview: p, color: "rgba(60,60,60,0.9)", width: 1.5, dash: [] });
      }
    }
    const nextHeat: HeatImage[] = [];
    if (ev) {
      for (const pass of ev.passes) {
        if (store.hiddenPasses.has(pass.id)) continue;
        const g = store.group(pass.group);
        if (g && !g.visible) continue;
        const p = await store.preview("pass", pass.id);
        if (p) next.push({ key: `pass:${pass.id}`, preview: p, color: g ? rgbCss(g.color) : "black", width: 1, dash: [], group: pass.group });
      }
      // Coverage audit: missed area and spill of each visible audited pass, under the paths.
      if (store.showAudit) {
        for (const pass of ev.passes) {
          if (!pass.audit || store.hiddenPasses.has(pass.id)) continue;
          const g = store.group(pass.group);
          if (g && !g.visible) continue;
          const missed = await store.previewAudit(pass.id, false);
          if (missed) next.unshift({ key: `missed:${pass.id}`, preview: missed, color: "rgba(220,30,30,0.9)", width: 0.5, dash: [], fill: "rgba(230,40,40,0.45)" });
          const spill = await store.previewAudit(pass.id, true);
          if (spill) next.unshift({ key: `spill:${pass.id}`, preview: spill, color: "rgba(230,140,0,0.9)", width: 0.5, dash: [], fill: "rgba(240,160,0,0.45)" });
        }
      }
      // Jumps and footprint heatmaps are per output group (planned order), preview only.
      for (const gs of ev.groups) {
        const g = store.group(gs.id);
        if (g && !g.visible) continue;
        if (store.showJumps) {
          const j = await store.previewJumps(gs.id);
          if (j) next.push({ key: `jumps:${gs.id}`, preview: j, color: "rgba(220,0,200,0.8)", width: 1, dash: [4, 3], group: gs.id });
        }
        if (store.showHeatmap && gs.heatmap) {
          const h = await store.heatmap(gs.id);
          if (h) nextHeat.push(heatImage(`heat:${gs.id}`, h));
        }
      }
    }
    let nextOrder: typeof order = null;
    const og = orderGroup;
    if (store.showOrder && og) {
      const o = order?.group === og && order.revision === revision ? order : null;
      const fetched = o ? null : await store.previewOrder(og);
      nextOrder = o ?? (fetched ? { group: og, seq: new Sequence(fetched), revision } : null);
    }
    if (token !== fetchToken) return; // a newer fetch is in flight
    items = next;
    itemsVersion++;
    heatImages = nextHeat;
    order = nextOrder;
    renderMiniScene();
    if (!fitted) fit();
    draw();
  }

  function draw() {
    if (!canvas) return;
    const dpr = devicePixelRatio;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    if (canvas.width !== w * dpr || canvas.height !== h * dpr) {
      canvas.width = w * dpr;
      canvas.height = h * dpr;
    }
    const ctx = canvas.getContext("2d")!;
    const v = { cx, cy, ppm, w, h };
    const ord = inspecting();
    if (ord) {
      // Stepping through the order redraws only the overlay, over the scene kept from before.
      const key = JSON.stringify([cx, cy, ppm, w, h, dpr, itemsVersion, store.selection, ord.group]);
      if (base?.key !== key) {
        const off = base?.canvas ?? document.createElement("canvas");
        off.width = w * dpr;
        off.height = h * dpr;
        const bctx = off.getContext("2d")!;
        bctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        drawGrid(bctx, w, h);
        drawOrderBase(bctx, v, ord.seq, orderState(ord));
        drawScene(bctx, v, true);
        base = { canvas: off, key };
      }
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(base.canvas, 0, 0);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      drawOrder(ctx, v, ord.seq, orderState(ord));
    } else {
      base = null;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      drawGrid(ctx, w, h);
      drawScene(ctx, v, true);
    }
    drawRulers(ctx, w, h);
    drawMinimap();
  }

  /** How the order inspector draws the inspected sequence now. */
  function orderState(ord: NonNullable<typeof order>): OrderState {
    const g = store.group(ord.group);
    const hidden = Uint8Array.from(ord.seq.order.passes, (id) => (store.hiddenPasses.has(id) ? 1 : 0));
    return {
      index: store.orderIndex,
      gradient: store.orderGradient,
      color: g ? rgbCss(g.color) : "black",
      jumps: store.showJumps,
      hidden: (i) => hidden[ord.seq.order.passOf[i]!] === 1,
      hiddenKey: hidden.join(""),
      hover,
    };
  }

  /** Heatmaps and every visible item through view `v`; `emphasis` thickens the selection. */
  function drawScene(ctx: CanvasRenderingContext2D, v: View, emphasis: boolean) {
    ctx.imageSmoothingEnabled = false;
    for (const hi of heatImages) {
      const [sx, sy] = project(v, hi.x, hi.top);
      ctx.drawImage(hi.canvas, sx, sy, hi.wMm * v.ppm, hi.hMm * v.ppm);
    }
    const sel = store.selection;
    // The order inspector draws its group itself and leaves the other groups faint.
    const ord = emphasis ? inspecting() : null;
    for (const it of items) {
      if (ord && it.group === ord.group) continue;
      const selected =
        emphasis &&
        ((sel.kind === "pass" && it.key === `pass:${sel.id}`) ||
          (sel.kind === "layer" && it.key === `layer:${sel.id}`) ||
          (sel.kind === "source" && it.key === `src:${sel.file}:${sel.layer}`));
      ctx.globalAlpha = ord && it.group ? 0.3 : 1;
      ctx.strokeStyle = it.color;
      const width = selected ? it.width + 1.5 : it.width;
      if (documentSpaceOk(ctx, v, (it.extent ??= extent(it.preview)))) {
        const path = (it.doc ??= documentPath(it.preview));
        inDocument(ctx, v, it.preview, (px) => {
          ctx.lineWidth = width * px;
          ctx.setLineDash(it.dash.map((d) => d * px));
          if (it.fill) {
            ctx.fillStyle = it.fill;
            ctx.fill(path, "evenodd");
          }
          ctx.stroke(path);
        });
        continue;
      }
      ctx.lineWidth = width;
      ctx.setLineDash(it.dash);
      const { originX: ox, originY: oy, coords, bulges, starts } = it.preview;
      ctx.beginPath();
      if (it.fill) {
        // A region: all loops in one path, filled even-odd so holes stay open.
        // Offsets from the buffer's origin, added back in double precision.
        const x = (k: number) => ox + coords[2 * k]!;
        const y = (k: number) => oy + coords[2 * k + 1]!;
        for (let i = 0; i + 1 < starts.length; i++) {
          const a = starts[i]!;
          const b = starts[i + 1]!;
          if (b - a < 2) continue;
          const [x0, y0] = project(v, x(a), y(a));
          ctx.moveTo(x0, y0);
          for (let s = 0; s < b - a; s++) {
            const k = a + s;
            const n = s + 1 < b - a ? k + 1 : a;
            segment(ctx, v, x(k), y(k), x(n), y(n), bulges[k]!);
          }
          ctx.closePath();
        }
        ctx.fillStyle = it.fill;
        ctx.fill("evenodd");
        ctx.stroke();
        continue;
      }
      for (let i = 0; i + 1 < starts.length; i++) tracePath(ctx, v, it.preview, i);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
    ctx.setLineDash([]);
  }

  function niceStep(pxPerMm: number): number {
    const target = 80 / pxPerMm; // mm per ~80 px
    const pow = Math.pow(10, Math.floor(Math.log10(target)));
    for (const m of [1, 2, 5, 10]) if (m * pow >= target) return m * pow;
    return 10 * pow;
  }

  function drawGrid(ctx: CanvasRenderingContext2D, w: number, h: number) {
    const step = niceStep(ppm);
    const [x0, y1] = toMm(0, 0, w, h);
    const [x1, y0] = toMm(w, h, w, h);
    ctx.strokeStyle = "rgba(128,128,128,0.15)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let x = Math.floor(x0 / step) * step; x <= x1; x += step) {
      const [sx] = toScreen(x, 0, w, h);
      ctx.moveTo(sx, 0);
      ctx.lineTo(sx, h);
    }
    for (let y = Math.floor(y0 / step) * step; y <= y1; y += step) {
      const [, sy] = toScreen(0, y, w, h);
      ctx.moveTo(0, sy);
      ctx.lineTo(w, sy);
    }
    ctx.stroke();
    // Origin axes.
    ctx.strokeStyle = "rgba(128,128,128,0.5)";
    ctx.beginPath();
    const [ox, oy] = toScreen(0, 0, w, h);
    ctx.moveTo(ox, 0);
    ctx.lineTo(ox, h);
    ctx.moveTo(0, oy);
    ctx.lineTo(w, oy);
    ctx.stroke();
  }

  function drawRulers(ctx: CanvasRenderingContext2D, w: number, h: number) {
    const step = niceStep(ppm);
    const [x0, y1] = toMm(0, 0, w, h);
    const [x1, y0] = toMm(w, h, w, h);
    ctx.fillStyle = "rgba(128,128,128,0.9)";
    ctx.font = "10px ui-monospace, monospace";
    const digits = Math.max(0, -Math.floor(Math.log10(step)));
    for (let x = Math.floor(x0 / step) * step; x <= x1; x += step) {
      const [sx] = toScreen(x, 0, w, h);
      ctx.fillText(x.toFixed(digits), sx + 2, h - 4);
    }
    for (let y = Math.floor(y0 / step) * step; y <= y1; y += step) {
      const [, sy] = toScreen(0, y, w, h);
      ctx.fillText(y.toFixed(digits), 2, sy - 2);
    }
  }

  // Interaction: wheel zoom at the cursor, drag to pan, double-click to fit.
  let dragging: { x: number; y: number; cx: number; cy: number } | null = null;

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    const [mx, my] = toMm(e.offsetX, e.offsetY, w, h);
    const factor = Math.exp(-e.deltaY * 0.0015);
    ppm = Math.min(Math.max(ppm * factor, 0.01), 1e5);
    const [nx, ny] = toMm(e.offsetX, e.offsetY, w, h);
    cx += mx - nx;
    cy += my - ny;
    draw();
  }

  /** Zoom about the centre of the view, by Svelte Flow's step (the graph's controls). */
  function zoomBy(factor: number) {
    ppm = Math.min(Math.max(ppm * factor, 0.01), 1e5);
    draw();
  }

  function onPointerDown(e: PointerEvent) {
    dragging = { x: e.clientX, y: e.clientY, cx, cy };
    canvas.setPointerCapture(e.pointerId);
  }

  function onPointerMove(e: PointerEvent) {
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    cursorMm = toMm(e.offsetX, e.offsetY, w, h);
    if (dragging) {
      cx = dragging.cx - (e.clientX - dragging.x) / ppm;
      cy = dragging.cy + (e.clientY - dragging.y) / ppm;
      draw();
      return;
    }
    // The order inspector: the path under the pointer, within a few pixels.
    const ord = inspecting();
    const next = ord ? ord.seq.pathAt(cursorMm[0], cursorMm[1], 5 / ppm) : -1;
    hoverAt = next >= 0 ? { x: e.offsetX, y: e.offsetY } : null;
    if (next !== hover) {
      hover = next;
      draw();
    }
  }

  function onPointerUp(e: PointerEvent) {
    // A click (not a drag) on a path of the inspected sequence makes it the current one.
    const click = dragging && Math.hypot(e.clientX - dragging.x, e.clientY - dragging.y) < 4;
    dragging = null;
    canvas.releasePointerCapture(e.pointerId);
    if (click && hover >= 0 && inspecting()) store.orderIndex = hover;
  }

  function onPointerLeave() {
    hoverAt = null;
    if (hover >= 0) {
      hover = -1;
      draw();
    }
  }

  // ---- minimap: the whole drawing with the view marked, like the graph's ---------------

  const MINI_W = 200;
  const MINI_H = 150;
  let mini: HTMLCanvasElement;
  /** The drawing rendered once per fetch (redrawing a large panel on every pan would not
   * keep up) and the document bounds it covers. */
  let miniScene: { canvas: HTMLCanvasElement; bounds: Bounds } | null = $state.raw(null);
  const dark = globalThis.matchMedia?.("(prefers-color-scheme: dark)");

  function renderMiniScene() {
    const b = sceneBounds();
    if (!b) {
      miniScene = null;
      return;
    }
    const bw = Math.max(b[2] - b[0], 1e-3);
    const bh = Math.max(b[3] - b[1], 1e-3);
    const scale = Math.min(MINI_W / bw, MINI_H / bh) * devicePixelRatio;
    const off = miniScene?.canvas ?? document.createElement("canvas");
    off.width = Math.max(1, Math.ceil(bw * scale));
    off.height = Math.max(1, Math.ceil(bh * scale));
    const ctx = off.getContext("2d")!;
    ctx.clearRect(0, 0, off.width, off.height);
    drawScene(ctx, { cx: (b[0] + b[2]) / 2, cy: (b[1] + b[3]) / 2, ppm: scale, w: off.width, h: off.height }, false);
    miniScene = { canvas: off, bounds: b };
  }

  /** The main view's rectangle in document mm. */
  function viewBounds(): Bounds {
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    const [x0, y1] = toMm(0, 0, w, h);
    const [x1, y0] = toMm(w, h, w, h);
    return [x0, y0, x1, y1];
  }

  /** The minimap's own view: the drawing and the main view, fitted with a margin. */
  function miniView(): View | null {
    if (!miniScene || !canvas) return null;
    const b = miniScene.bounds;
    const vb = viewBounds();
    const e: Bounds = [Math.min(b[0], vb[0]), Math.min(b[1], vb[1]), Math.max(b[2], vb[2]), Math.max(b[3], vb[3])];
    const scale = Math.min(MINI_W / Math.max(e[2] - e[0], 1e-3), MINI_H / Math.max(e[3] - e[1], 1e-3)) / 1.1;
    return { cx: (e[0] + e[2]) / 2, cy: (e[1] + e[3]) / 2, ppm: scale, w: MINI_W, h: MINI_H };
  }

  /** The main view's rectangle on the minimap, in its pixels. */
  function viewRect(v: View): Bounds {
    const vb = viewBounds();
    const [x0, y0] = project(v, vb[0], vb[3]);
    const [x1, y1] = project(v, vb[2], vb[1]);
    return [x0, y0, x1, y1];
  }

  function drawMinimap() {
    if (!mini) return;
    const dpr = devicePixelRatio;
    if (mini.width !== MINI_W * dpr || mini.height !== MINI_H * dpr) {
      mini.width = MINI_W * dpr;
      mini.height = MINI_H * dpr;
    }
    const ctx = mini.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, MINI_W, MINI_H);
    const v = miniView();
    if (!v || !miniScene) return;
    const b = miniScene.bounds;
    const [sx, sy] = project(v, b[0], b[3]);
    ctx.drawImage(miniScene.canvas, sx, sy, (b[2] - b[0]) * v.ppm, (b[3] - b[1]) * v.ppm);
    // Everything outside the main view is masked, as on Svelte Flow's minimap.
    const [x0, y0, x1, y1] = viewRect(v);
    ctx.beginPath();
    ctx.rect(0, 0, MINI_W, MINI_H);
    ctx.rect(x0, y0, x1 - x0, y1 - y0);
    ctx.fillStyle = dark?.matches ? "rgba(60, 60, 60, 0.6)" : "rgba(240, 240, 240, 0.6)";
    ctx.fill("evenodd");
  }

  // Drag the view rectangle to pan (pressing outside it jumps there first); wheel zooms.
  let miniDrag: { x: number; y: number; cx: number; cy: number; ppm: number } | null = null;

  function onMiniDown(e: PointerEvent) {
    const v = miniView();
    if (!v) return;
    const [x0, y0, x1, y1] = viewRect(v);
    if (e.offsetX < x0 || e.offsetX > x1 || e.offsetY < y0 || e.offsetY > y1) {
      cx = v.cx + (e.offsetX - v.w / 2) / v.ppm;
      cy = v.cy - (e.offsetY - v.h / 2) / v.ppm;
      draw();
    }
    // The minimap's scale moves with the view; the drag keeps the scale it started with.
    miniDrag = { x: e.clientX, y: e.clientY, cx, cy, ppm: miniView()!.ppm };
    mini.setPointerCapture(e.pointerId);
  }

  function onMiniMove(e: PointerEvent) {
    if (!miniDrag) return;
    cx = miniDrag.cx + (e.clientX - miniDrag.x) / miniDrag.ppm;
    cy = miniDrag.cy - (e.clientY - miniDrag.y) / miniDrag.ppm;
    draw();
  }

  function onMiniUp(e: PointerEvent) {
    miniDrag = null;
    mini.releasePointerCapture(e.pointerId);
  }

  function onMiniWheel(e: WheelEvent) {
    e.preventDefault();
    ppm = Math.min(Math.max(ppm * Math.exp(-e.deltaY * 0.0015), 0.01), 1e5);
    draw();
  }

  $effect(() => {
    const redraw = () => drawMinimap();
    dark?.addEventListener("change", redraw);
    return () => dark?.removeEventListener("change", redraw);
  });

  $effect(() => {
    // Re-fetch when geometry or visibility changes.
    void store.previewRevision;
    void store.showSources;
    void store.showLayers;
    void store.showJumps;
    void store.showHeatmap;
    void store.showAudit;
    void store.showOrder;
    void orderGroup;
    void store.hiddenSourceLayers;
    void store.hiddenLayers;
    void store.hiddenPasses;
    void store.sources.length;
    void refetch();
  });

  $effect(() => {
    void store.selection;
    void store.orderIndex;
    void store.orderGradient;
    void order;
    draw();
  });

  $effect(() => {
    const ro = new ResizeObserver(() => draw());
    ro.observe(canvas);
    return () => ro.disconnect();
  });
</script>

<div class="preview">
  <canvas
    bind:this={canvas}
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointerleave={onPointerLeave}
    ondblclick={fit}
  ></canvas>
  {#if hoverAt && hover >= 0 && order}
    <div class="hover-label" style:left="{hoverAt.x + 12}px" style:top="{hoverAt.y + 12}px">#{(hover + 1).toLocaleString("en-US")}</div>
  {/if}
  {#if store.showOrder}
    <OrderTimeline {store} seq={order?.seq ?? null} group={orderGroup} />
  {/if}
  <div class="hud overlay-bar">
    <label><input type="checkbox" bind:checked={store.showSources} /> sources</label>
    <label><input type="checkbox" bind:checked={store.showLayers} /> regions</label>
    <label title="hypothetical straight beam-off jumps of the planned order; never exported"><input type="checkbox" bind:checked={store.showJumps} /> jumps</label>
    <label title="jump footprint coverage over protected regions (relative, not a dose)"><input type="checkbox" bind:checked={store.showHeatmap} /> heatmap</label>
    <label title="coverage audit: area the paths miss (red) and sweep outside the region (amber) at each group's assumed marking width"><input type="checkbox" bind:checked={store.showAudit} /> audit</label>
    <label title="step through one group's marking order as the export writes it: marked paths shade from early to late, chevrons show the direction, a click on a path makes it the current one"><input type="checkbox" bind:checked={store.showOrder} /> order</label>
    {#if store.hiddenCount}<button onclick={() => store.showAll()} title="show every hidden source layer, region and pass">show all ({store.hiddenCount} hidden)</button>{/if}
    <span class="coords">
      {#if cursorMm}{cursorMm[0].toFixed(3)}, {cursorMm[1].toFixed(3)} mm{/if}
      · {ppm.toFixed(1)} px/mm
    </span>
  </div>
  <!-- Zoom controls as in the graph tab (Svelte Flow's Controls, with its icons). -->
  <div class="controls" role="group" aria-label="Zoom controls">
    <button title="Zoom In" aria-label="Zoom In" onclick={() => zoomBy(1.2)}>
      <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><path d="M32 18.133H18.133V32h-4.266V18.133H0v-4.266h13.867V0h4.266v13.867H32z" /></svg>
    </button>
    <button title="Zoom Out" aria-label="Zoom Out" onclick={() => zoomBy(1 / 1.2)}>
      <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 5"><path d="M0 0h32v4.2H0z" /></svg>
    </button>
    <button title="Fit View (double-click)" aria-label="Fit View" onclick={fit}>
      <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 30"
        ><path
          d="M3.692 4.63c0-.53.4-.938.939-.938h5.215V0H4.708C2.13 0 0 2.054 0 4.63v5.216h3.692V4.631zM27.354 0h-5.2v3.692h5.17c.53 0 .984.4.984.939v5.215H32V4.631A4.624 4.624 0 0027.354 0zm.954 24.83c0 .532-.4.94-.939.94h-5.215v3.768h5.215c2.577 0 4.631-2.13 4.631-4.707v-5.139h-3.692v5.139zm-23.677.94c-.531 0-.939-.4-.939-.94v-5.138H0v5.139c0 2.577 2.13 4.707 4.708 4.707h5.138V25.77H4.631z"
        /></svg
      >
    </button>
  </div>
  <canvas
    class="minimap"
    class:empty={!miniScene}
    bind:this={mini}
    aria-label="overview: drag the view, wheel to zoom"
    onpointerdown={onMiniDown}
    onpointermove={onMiniMove}
    onpointerup={onMiniUp}
    onwheel={onMiniWheel}
  ></canvas>
</div>

<style>
  .preview {
    position: relative;
    height: 100%;
    min-height: 240px;
  }
  canvas {
    width: 100%;
    height: 100%;
    display: block;
    cursor: grab;
    background: color-mix(in srgb, Canvas 97%, CanvasText 3%);
  }
  .coords {
    font-family: ui-monospace, monospace;
    opacity: 0.8;
  }
  .hover-label {
    position: absolute;
    z-index: 6;
    pointer-events: none;
    padding: 1px 4px;
    font: 11px ui-monospace, monospace;
    background: color-mix(in srgb, Canvas 90%, transparent);
    border-radius: 3px;
    box-shadow: 0 0 2px 1px rgba(0, 0, 0, 0.1);
  }
  /* Size, place and colours of Svelte Flow's minimap in the graph tab (both get the
     controls' shadow, so the edge shows on the preview's grey too). */
  .minimap {
    position: absolute;
    right: 15px;
    bottom: 15px;
    width: 200px;
    height: 150px;
    background: #fff;
    box-shadow: 0 0 2px 1px rgba(0, 0, 0, 0.08);
    cursor: grab;
  }
  .minimap.empty {
    display: none;
  }
  /* Svelte Flow's Controls: a column of 26 px buttons with 12 px icons. */
  .controls {
    position: absolute;
    left: 15px;
    bottom: 15px;
    display: flex;
    flex-direction: column;
    box-shadow: 0 0 2px 1px rgba(0, 0, 0, 0.08);
  }
  .controls button {
    display: flex;
    justify-content: center;
    align-items: center;
    box-sizing: border-box;
    width: 26px;
    height: 26px;
    padding: 4px;
    border: none;
    border-bottom: 1px solid #eee;
    background: #fefefe;
    color: inherit;
    cursor: pointer;
    user-select: none;
  }
  .controls button:hover {
    background: #f4f4f4;
  }
  .controls svg {
    width: 100%;
    max-width: 12px;
    max-height: 12px;
    fill: currentColor;
  }
  @media (prefers-color-scheme: dark) {
    .minimap {
      background: #141414;
    }
    .controls button {
      background: #2b2b2b;
      border-bottom-color: #5b5b5b;
      color: #f8f8f8;
    }
    .controls button:hover {
      background: #3e3e3e;
      color: #fff;
    }
  }
</style>
