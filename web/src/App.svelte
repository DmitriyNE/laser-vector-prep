<script lang="ts">
  import { onMount } from "svelte";
  import { GeometryWorker, latestOnly } from "./worker/client.js";

  // Shared-memory wasm cannot start without cross-origin isolation. On static hosting the
  // coi-serviceworker reloads the page once to get there; until then we just wait.
  const isolated = globalThis.crossOriginIsolated === true;

  let core = $state("…");
  let threads: number | null = $state(null);
  let error: string | null = $state(null);
  let vertices = $state(0);
  let bbox: string | null = $state(null);
  let canvas: HTMLCanvasElement;

  onMount(() => {
    if (!isolated) return;
    const worker = new GeometryWorker();
    const latest = latestOnly<Float64Array>();

    worker
      .call({ kind: "version" })
      .then((v) => {
        core = v;
        console.info(`lvp core ${v}, crossOriginIsolated=${String(isolated)}`);
      })
      .catch((e: Error) => (error = e.message));
    worker
      .call({ kind: "threads" })
      .then((n) => {
        threads = n;
        console.info(`lvp threads ${n}`);
      })
      .catch((e: Error) => (error = e.message));

    // Smoke test of the parallel path: a buffer large enough to cross the rayon threshold.
    const n = 200_000;
    const pts = new Float64Array(2 * n);
    for (let i = 0; i < n; i++) {
      pts[2 * i] = 50 * Math.cos(i) - 7;
      pts[2 * i + 1] = 20 * Math.sin(i) + 3;
    }
    worker
      .call({ kind: "bbox", points: pts })
      .then((b) => {
        bbox = b ? Array.from(b, (v) => v.toFixed(3)).join(", ") : "empty";
        console.info(`lvp bbox ${bbox}`);
      })
      .catch((e: Error) => (error = e.message));

    // Smoke test of the worker/wasm path: a flattened arc drawn at 10 px per mm.
    const draw = async () => {
      const pts = await latest(
        worker.call({
          kind: "flattenArc",
          cx: 20,
          cy: 20,
          radius: 15,
          startAngle: 0,
          sweep: (3 * Math.PI) / 2,
          tolerance: 0.05,
        }),
      );
      if (pts) render(pts);
    };
    void draw();
    const ro = new ResizeObserver(() => void draw());
    ro.observe(canvas);
    return () => {
      ro.disconnect();
      worker.dispose();
    };
  });

  function render(pts: Float64Array) {
    const dpr = devicePixelRatio;
    canvas.width = canvas.clientWidth * dpr;
    canvas.height = canvas.clientHeight * dpr;
    const ctx = canvas.getContext("2d")!;
    ctx.scale(dpr, dpr);
    const s = 10; // px per mm; document +y is up, canvas y is down
    const h = canvas.clientHeight;
    ctx.lineWidth = 1;
    ctx.strokeStyle = "#1f6feb";
    ctx.beginPath();
    for (let i = 0; i < pts.length; i += 2) {
      const x = pts[i]! * s;
      const y = h - pts[i + 1]! * s;
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
    vertices = pts.length / 2;
  }
</script>

<div class="app">
  <header>
    <h1>Laser Vector Prep</h1>
    <dl>
      <dt>core</dt>
      <dd id="core">{core}</dd>
      <dt>threads</dt>
      <dd id="threads">{threads ?? "…"}</dd>
      <dt>crossOriginIsolated</dt>
      <dd id="coi">{String(isolated)}</dd>
      <dt>SharedArrayBuffer</dt>
      <dd>{typeof SharedArrayBuffer !== "undefined"}</dd>
    </dl>
    {#if error}<span class="error">{error}</span>{/if}
    {#if !isolated}<span class="error">waiting for cross-origin isolation (page reloads once)</span>{/if}
  </header>
  <canvas bind:this={canvas}></canvas>
  <footer>{vertices} vertices · parallel bbox of 200k points: {bbox ?? "…"}</footer>
</div>

<style>
  .app {
    display: grid;
    grid-template-rows: auto 1fr auto;
    height: 100%;
  }

  header {
    display: flex;
    gap: 1.5rem;
    align-items: baseline;
    padding: 0.5rem 1rem;
    border-bottom: 1px solid color-mix(in srgb, currentColor 20%, transparent);
  }

  h1 {
    font-size: 1rem;
    margin: 0;
  }

  dl {
    display: flex;
    gap: 1rem;
    margin: 0;
    font-family: ui-monospace, monospace;
  }

  dt {
    opacity: 0.6;
    display: inline;
  }

  dd {
    display: inline;
    margin: 0 0 0 0.25rem;
  }

  .error {
    color: #d33;
  }

  canvas {
    width: 100%;
    height: 100%;
    display: block;
  }

  footer {
    padding: 0.25rem 1rem;
    font-family: ui-monospace, monospace;
    opacity: 0.6;
  }
</style>
