<script lang="ts">
  import type { OptimizerMode, OrderStats } from "../core/types.js";
  import type { ProjectStore } from "../state/project.svelte.js";

  let { store }: { store: ProjectStore } = $props();
  const touch = () => store.update(() => {});
  const opt = $derived(store.profile.optimizer);
  const groups = $derived(store.evaluation?.groups ?? []);
  const candidates = $derived(store.profile.workingLayers.filter((l) => l.kind !== "linework"));

  function setMode(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value as OptimizerMode;
    store.update((p) => {
      p.optimizer.mode = v;
    });
  }

  function toggleProtected(id: string) {
    store.update((p) => {
      const i = p.optimizer.protected.indexOf(id);
      if (i >= 0) p.optimizer.protected.splice(i, 1);
      else p.optimizer.protected.push(id);
    });
  }

  function setPoint(which: "startMm" | "endMm", on: boolean) {
    store.update((p) => {
      p.optimizer[which] = on ? (p.optimizer[which] ?? [0, 0]) : null;
    });
  }

  function setCoord(which: "startMm" | "endMm", axis: 0 | 1, e: Event) {
    const v = Number((e.currentTarget as HTMLInputElement).value);
    store.update((p) => {
      const pt = p.optimizer[which];
      if (pt && Number.isFinite(v)) pt[axis] = v;
    });
  }

  const mm = (v: number) => (v >= 100 ? v.toFixed(0) : v.toFixed(1));
  const pair = (a: number, b: number, f: (v: number) => string = mm) => (Math.abs(a - b) < 1e-9 ? f(a) : `${f(a)} → ${f(b)}`);
  const cov = (s: OrderStats) => (s.coveredMm2 === undefined ? "–" : s.coveredMm2.toFixed(2));
</script>

<section class="panel order">
  <h2>Path order</h2>
  <label>Mode
    <select value={opt.mode} onchange={setMode}>
      <option value="asGenerated">as generated</option>
      <option value="minimumJump">minimum jump length</option>
      <option value="bleedAware">bleed-aware (protected regions)</option>
    </select>
  </label>
  {#if opt.mode === "bleedAware" || opt.protected.length}
    <div class="grid">
      <label>Footprint width (mm) <input type="number" step="0.01" min="0.001" bind:value={opt.footprintMm} oninput={touch} /></label>
      <label>Bleed weight <input type="number" step="0.1" min="0" bind:value={opt.bleedWeight} oninput={touch} /></label>
      <label>Raster cell (mm) <input type="number" step="0.01" min="0.005" bind:value={opt.cellMm} oninput={touch} /></label>
      <label>Max sweeps <input type="number" step="1" min="0" bind:value={opt.maxSweeps} oninput={touch} /></label>
    </div>
  {/if}
  <fieldset>
    <legend>Protected regions (keep-clear for jump footprints)</legend>
    {#if candidates.length === 0}<p class="hint">No region or envelope layers to protect.</p>{/if}
    {#each candidates as l (l.id)}
      <label class="check"><input type="checkbox" checked={opt.protected.includes(l.id)} onchange={() => toggleProtected(l.id)} /> {l.name}</label>
    {/each}
  </fieldset>
  <div class="grid">
    <label class="check"><input type="checkbox" checked={!!opt.startMm} onchange={(e) => setPoint("startMm", (e.currentTarget as HTMLInputElement).checked)} /> assume start point</label>
    {#if opt.startMm}
      <span class="xy"><input type="number" step="0.1" value={opt.startMm[0]} oninput={(e) => setCoord("startMm", 0, e)} /><input type="number" step="0.1" value={opt.startMm[1]} oninput={(e) => setCoord("startMm", 1, e)} /> mm</span>
    {/if}
    <label class="check"><input type="checkbox" checked={!!opt.endMm} onchange={(e) => setPoint("endMm", (e.currentTarget as HTMLInputElement).checked)} /> assume end point</label>
    {#if opt.endMm}
      <span class="xy"><input type="number" step="0.1" value={opt.endMm[0]} oninput={(e) => setCoord("endMm", 0, e)} /><input type="number" step="0.1" value={opt.endMm[1]} oninput={(e) => setCoord("endMm", 1, e)} /> mm</span>
    {/if}
  </div>
  {#if groups.length}
    <div class="scroll">
      <table>
        <thead>
          <tr><th>Group</th><th>Paths</th><th title="total jump length, before → after">Travel mm</th><th title="longest single jump">Longest</th><th title="jump–jump crossings">Cross</th><th title="protected area under jump footprints">Cover mm²</th><th title="most footprints stacked on one protected spot">Peak</th></tr>
        </thead>
        <tbody>
          {#each groups as g (g.id)}
            <tr class="group-row" data-group={g.id}>
              <td title={g.mode}>{g.name}<span class="mode"> {g.mode === "asGenerated" ? "" : g.mode === "minimumJump" ? "min jump" : "bleed"}</span></td>
              <td class="num">{g.paths}</td>
              <td class="num">{pair(g.before.jumpLengthMm, g.after.jumpLengthMm)}</td>
              <td class="num">{pair(g.before.longestJumpMm, g.after.longestJumpMm)}</td>
              <td class="num">{pair(g.before.crossings, g.after.crossings, (v) => String(v))}</td>
              <td class="num">{g.before.coveredMm2 === undefined ? "–" : `${cov(g.before)} → ${cov(g.after)}`}</td>
              <td class="num">{g.before.peakCoverage === undefined ? "–" : pair(g.before.peakCoverage, g.after.peakCoverage ?? 0, (v) => String(v))}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
  <p class="hint">
    Before → after the plan, assuming straight beam-off jumps between the emitted paths. The laser software may execute in another
    order; coverage is a relative geometric footprint, not a dose. Holes stay before their contour in cutting passes.
  </p>
</section>

<style>
  label {
    display: block;
    font-size: 12px;
    margin: 0.3rem 0;
  }
  select,
  input[type="number"] {
    display: block;
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    font-size: 12px;
  }
  .check,
  .check input {
    display: inline;
    width: auto;
    margin-right: 0.3rem;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0 0.6rem;
    align-items: end;
  }
  .xy {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    font-size: 12px;
  }
  .xy input {
    width: 5rem;
  }
  fieldset {
    border: 1px solid color-mix(in srgb, CanvasText 15%, transparent);
    border-radius: 4px;
    margin: 0.4rem 0;
    padding: 0.2rem 0.5rem 0.4rem;
    font-size: 12px;
  }
  legend {
    font-size: 11px;
    opacity: 0.8;
  }
  .scroll {
    overflow-x: auto;
    margin-top: 0.4rem;
  }
  table {
    border-collapse: collapse;
    font-size: 11px;
  }
  th,
  td {
    text-align: left;
    padding: 2px 4px;
    border-bottom: 1px solid color-mix(in srgb, CanvasText 10%, transparent);
    white-space: nowrap;
  }
  th {
    font-weight: normal;
    opacity: 0.7;
  }
  .num {
    font-family: ui-monospace, monospace;
  }
  .mode {
    opacity: 0.6;
    font-size: 10px;
  }
  .hint {
    font-size: 11px;
    opacity: 0.7;
  }
</style>
