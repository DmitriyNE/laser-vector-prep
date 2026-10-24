import "./style.css";
import { GeometryWorker, latestOnly } from "./worker/client.js";

const app = document.querySelector<HTMLElement>("#app")!;
app.innerHTML = `
  <header>
    <h1>Laser Vector Prep</h1>
    <dl>
      <dt>core</dt><dd id="core">…</dd>
      <dt>crossOriginIsolated</dt><dd id="coi">${String(globalThis.crossOriginIsolated)}</dd>
      <dt>SharedArrayBuffer</dt><dd>${typeof SharedArrayBuffer !== "undefined"}</dd>
    </dl>
  </header>
  <canvas id="preview"></canvas>
`;

const worker = new GeometryWorker();
const latest = latestOnly<Float64Array>();

worker
  .call({ kind: "version" })
  .then((v) => {
    document.querySelector("#core")!.textContent = v;
    console.info(`lvp core ${v}, crossOriginIsolated=${String(globalThis.crossOriginIsolated)}`);
  })
  .catch((e) => (document.querySelector("#core")!.textContent = `error: ${e.message}`));

const canvas = document.querySelector<HTMLCanvasElement>("#preview")!;

// Smoke test of the worker/wasm path: a flattened arc drawn 1 mm = 10 px.
async function draw() {
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
  if (!pts) return; // superseded

  const dpr = devicePixelRatio;
  canvas.width = canvas.clientWidth * dpr;
  canvas.height = canvas.clientHeight * dpr;
  const ctx = canvas.getContext("2d")!;
  ctx.scale(dpr, dpr);
  const s = 10; // px per mm; document +y up → canvas y down
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
  ctx.fillStyle = "currentColor";
  ctx.fillText(`${pts.length / 2} vertices`, 8, h - 8);
}

draw();
addEventListener("resize", () => void draw());
