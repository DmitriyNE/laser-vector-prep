<script lang="ts">
  import type { Heatmap, Preview } from "../core/types.js";
  import { rgbCss, type ProjectStore } from "../state/project.svelte.js";
  import BusyIndicator from "./BusyIndicator.svelte";

  let { store }: { store: ProjectStore } = $props();

  let canvas: HTMLCanvasElement;
  // View: document mm at the canvas centre and pixels per mm.
  let cx = $state(0);
  let cy = $state(0);
  let ppm = $state(10);
  let fitted = false;
  let cursorMm: [number, number] | null = $state(null);

  type Item = { key: string; preview: Preview; color: string; width: number; dash: number[]; closedHoles?: boolean; fill?: string };
  let items: Item[] = [];
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
    return [w / 2 + (x - cx) * ppm, h / 2 - (y - cy) * ppm];
  }

  function toMm(px: number, py: number, w: number, h: number): [number, number] {
    return [cx + (px - w / 2) / ppm, cy - (py - h / 2) / ppm];
  }

  function fit() {
    const ev = store.evaluation;
    let b: [number, number, number, number] | null = null;
    const grow = (x: [number, number, number, number] | null) => {
      if (!x) return;
      b = b ? [Math.min(b[0], x[0]), Math.min(b[1], x[1]), Math.max(b[2], x[2]), Math.max(b[3], x[3])] : [...x];
    };
    for (const s of store.sources) grow(s.boundsMm);
    if (ev) grow(ev.boundsMm);
    const bb = b as [number, number, number, number] | null;
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
        if (p) next.push({ key: `layer:${l.id}`, preview: p, color: "rgba(60,60,60,0.9)", width: 1.5, dash: [], closedHoles: true });
      }
    }
    const nextHeat: HeatImage[] = [];
    if (ev) {
      for (const pass of ev.passes) {
        if (store.hiddenPasses.has(pass.id)) continue;
        const g = store.group(pass.group);
        if (g && !g.visible) continue;
        const p = await store.preview("pass", pass.id);
        if (p) next.push({ key: `pass:${pass.id}`, preview: p, color: g ? rgbCss(g.color) : "black", width: 1, dash: [] });
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
          if (j) next.push({ key: `jumps:${gs.id}`, preview: j, color: "rgba(220,0,200,0.8)", width: 1, dash: [4, 3] });
        }
        if (store.showHeatmap && gs.heatmap) {
          const h = await store.heatmap(gs.id);
          if (h) nextHeat.push(heatImage(`heat:${gs.id}`, h));
        }
      }
    }
    if (token !== fetchToken) return; // a newer fetch is in flight
    items = next;
    heatImages = nextHeat;
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
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    drawGrid(ctx, w, h);
    ctx.imageSmoothingEnabled = false;
    for (const hi of heatImages) {
      const [sx, sy] = toScreen(hi.x, hi.top, w, h);
      ctx.drawImage(hi.canvas, sx, sy, hi.wMm * ppm, hi.hMm * ppm);
    }
    const sel = store.selection;
    for (const it of items) {
      const selected =
        (sel.kind === "pass" && it.key === `pass:${sel.id}`) ||
        (sel.kind === "layer" && it.key === `layer:${sel.id}`) ||
        (sel.kind === "source" && it.key === `src:${sel.file}:${sel.layer}`);
      ctx.strokeStyle = it.color;
      ctx.lineWidth = selected ? it.width + 1.5 : it.width;
      ctx.setLineDash(it.dash);
      const { originX: ox, originY: oy, coords, bulges, starts, closed } = it.preview;
      // Offsets from the buffer's origin, added back in double precision.
      const x = (k: number) => ox + coords[2 * k]!;
      const y = (k: number) => oy + coords[2 * k + 1]!;
      if (it.fill) {
        // A region: all loops in one path, filled even-odd so holes stay open.
        ctx.beginPath();
        for (let i = 0; i + 1 < starts.length; i++) {
          const a = starts[i]!;
          const b = starts[i + 1]!;
          if (b - a < 2) continue;
          const [x0, y0] = toScreen(x(a), y(a), w, h);
          ctx.moveTo(x0, y0);
          for (let s = 0; s < b - a; s++) {
            const k = a + s;
            const n = s + 1 < b - a ? k + 1 : a;
            segment(ctx, x(k), y(k), x(n), y(n), bulges[k]!, w, h);
          }
          ctx.closePath();
        }
        ctx.fillStyle = it.fill;
        ctx.fill("evenodd");
        ctx.stroke();
        continue;
      }
      for (let i = 0; i + 1 < starts.length; i++) {
        const a = starts[i]!;
        const b = starts[i + 1]!;
        if (b - a < 2) continue;
        const isClosed = closed[i] === 1;
        ctx.beginPath();
        const [x0, y0] = toScreen(x(a), y(a), w, h);
        ctx.moveTo(x0, y0);
        const segments = isClosed ? b - a : b - a - 1;
        for (let s = 0; s < segments; s++) {
          const k = a + s;
          const n = s + 1 < b - a ? k + 1 : a; // the closing segment returns to the start
          segment(ctx, x(k), y(k), x(n), y(n), bulges[k]!, w, h);
        }
        if (isClosed) ctx.closePath();
        ctx.stroke();
      }
    }
    ctx.setLineDash([]);
    drawRulers(ctx, w, h);
  }

  /**
   * Add the segment from (x1, y1) to (x2, y2) mm with `bulge` to the current canvas path:
   * a native arc, or a line when the arc's sagitta is below a fraction of a pixel. The y
   * flip turns the model's counter-clockwise arcs into the canvas's anticlockwise ones.
   */
  function segment(ctx: CanvasRenderingContext2D, x1: number, y1: number, x2: number, y2: number, bulge: number, w: number, h: number) {
    const [sx2, sy2] = toScreen(x2, y2, w, h);
    const chord = Math.hypot(x2 - x1, y2 - y1);
    if (bulge === 0 || chord === 0 || (Math.abs(bulge) * chord * ppm) / 2 < 0.25) {
      ctx.lineTo(sx2, sy2);
      return;
    }
    // Centre: chord midpoint plus the left normal times chord (1 - b^2) / (4 b).
    const mx = (x1 + x2) / 2;
    const my = (y1 + y2) / 2;
    const off = (1 - bulge * bulge) / (4 * bulge);
    const cx = mx - (y2 - y1) * off;
    const cy = my + (x2 - x1) * off;
    const r = Math.hypot(x1 - cx, y1 - cy);
    const [scx, scy] = toScreen(cx, cy, w, h);
    const [sx1, sy1] = toScreen(x1, y1, w, h);
    const a0 = Math.atan2(sy1 - scy, sx1 - scx);
    const a1 = Math.atan2(sy2 - scy, sx2 - scx);
    ctx.arc(scx, scy, r * ppm, a0, a1, bulge > 0);
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
    }
  }

  function onPointerUp(e: PointerEvent) {
    dragging = null;
    canvas.releasePointerCapture(e.pointerId);
  }

  $effect(() => {
    // Re-fetch when geometry or visibility changes.
    void store.previewRevision;
    void store.showSources;
    void store.showLayers;
    void store.showJumps;
    void store.showHeatmap;
    void store.showAudit;
    void store.hiddenSourceLayers;
    void store.hiddenLayers;
    void store.hiddenPasses;
    void store.sources.length;
    void refetch();
  });

  $effect(() => {
    void store.selection;
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
    ondblclick={fit}
  ></canvas>
  <div class="hud">
    <button onclick={fit} title="Fit (double-click)">fit</button>
    <label><input type="checkbox" bind:checked={store.showSources} /> sources</label>
    <label><input type="checkbox" bind:checked={store.showLayers} /> regions</label>
    <label title="hypothetical straight beam-off jumps of the planned order; never exported"><input type="checkbox" bind:checked={store.showJumps} /> jumps</label>
    <label title="jump footprint coverage over protected regions (relative, not a dose)"><input type="checkbox" bind:checked={store.showHeatmap} /> heatmap</label>
    <label title="coverage audit: area the paths miss (red) and sweep outside the region (amber) at each group's assumed marking width"><input type="checkbox" bind:checked={store.showAudit} /> audit</label>
    {#if store.hiddenCount}<button onclick={() => store.showAll()} title="show every hidden source layer, region and pass">show all ({store.hiddenCount} hidden)</button>{/if}
    <span class="coords">
      {#if cursorMm}{cursorMm[0].toFixed(3)}, {cursorMm[1].toFixed(3)} mm{/if}
      · {ppm.toFixed(1)} px/mm
    </span>
    <BusyIndicator {store} />
  </div>
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
  .hud {
    position: absolute;
    top: 0.4rem;
    left: 0.4rem;
    display: flex;
    gap: 0.75rem;
    align-items: center;
    font-size: 12px;
    background: color-mix(in srgb, Canvas 80%, transparent);
    padding: 0.25rem 0.5rem;
    border-radius: 4px;
  }
  .coords {
    font-family: ui-monospace, monospace;
    opacity: 0.8;
  }
</style>
