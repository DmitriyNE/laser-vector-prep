<script lang="ts">
  import type { GroupSummary, OptimizerMode, OrderStats } from "../core/types.js";
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
  const MODES: Record<OptimizerMode, string> = { asGenerated: "as generated", minimumJump: "minimum jump", bleedAware: "bleed-aware" };

  /** A group's other statistics, before → after, for its row's tooltip. */
  function details(g: GroupSummary): string {
    const lines = [
      `${MODES[g.mode]}`,
      `longest jump ${pair(g.before.longestJumpMm, g.after.longestJumpMm)} mm`,
      `crossings ${pair(g.before.crossings, g.after.crossings, String)}`,
    ];
    if (g.before.coveredMm2 !== undefined) lines.push(`protected area under footprints ${cov(g.before)} → ${cov(g.after)} mm²`);
    if (g.before.peakCoverage !== undefined) lines.push(`most footprints on one spot ${pair(g.before.peakCoverage, g.after.peakCoverage ?? 0, String)}`);
    return lines.join("\n");
  }
</script>

<section class="panel order">
  <h2
    title="Before → after the plan, assuming straight beam-off jumps between the emitted paths. The laser software may execute in another order; coverage is a relative geometric footprint, not a dose. Holes stay before their contour in cutting passes."
  >
    Path order
  </h2>
  <label>Mode
    <select value={opt.mode} onchange={setMode}>
      <option value="asGenerated">as generated</option>
      <option value="minimumJump">minimum jump length</option>
      <option value="bleedAware">bleed-aware (protected regions)</option>
    </select>
  </label>
  <fieldset>
    <legend title="keep-clear areas: bleed-aware planning keeps jump footprints off them">Protected regions</legend>
    {#if candidates.length === 0}<p class="hint">No region or envelope layers to protect.</p>{/if}
    {#each candidates as l (l.id)}
      <label class="check"><input type="checkbox" checked={opt.protected.includes(l.id)} onchange={() => toggleProtected(l.id)} /> {l.name}</label>
    {/each}
  </fieldset>
  {#if groups.length}
    <table>
      <thead>
        <tr><th>Group</th><th class="num">Paths</th><th class="num" title="total jump length, before → after">Travel mm</th></tr>
      </thead>
      <tbody>
        {#each groups as g (g.id)}
          <tr class="group-row" class:inspected={store.showOrder && store.orderGroup === g.id} data-group={g.id} title={details(g)}>
            <td><button class="inspect" title="step through this group's order in the preview" onclick={() => store.inspectOrder(g.id)}>{g.name}</button></td>
            <td class="num">{g.paths}</td>
            <td class="num">{pair(g.before.jumpLengthMm, g.after.jumpLengthMm)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
  <details>
    <summary>Advanced</summary>
    {#if opt.mode === "bleedAware" || opt.protected.length}
      <div class="grid">
        <label title="assumed width of a jump's influence: the geometric surrogate behind the bleed penalty and the heatmap">Footprint (mm) <input type="number" step="0.01" min="0.001" bind:value={opt.footprintMm} oninput={touch} /></label>
        <label title="weight of the bleed penalty against jump length">Bleed weight <input type="number" step="0.1" min="0" bind:value={opt.bleedWeight} oninput={touch} /></label>
        <label title="raster cell of the footprint model; coarsened automatically for very large groups">Raster cell (mm) <input type="number" step="0.01" min="0.005" bind:value={opt.cellMm} oninput={touch} /></label>
        <label title="upper bound on local-improvement sweeps">Max sweeps <input type="number" step="1" min="0" bind:value={opt.maxSweeps} oninput={touch} /></label>
      </div>
    {/if}
    <div class="grid">
      <label class="check" title="where the beam is before the first path, if you know it (never invented)"><input type="checkbox" checked={!!opt.startMm} onchange={(e) => setPoint("startMm", (e.currentTarget as HTMLInputElement).checked)} /> start point</label>
      {#if opt.startMm}
        <span class="xy"><input type="number" step="0.1" value={opt.startMm[0]} oninput={(e) => setCoord("startMm", 0, e)} /><input type="number" step="0.1" value={opt.startMm[1]} oninput={(e) => setCoord("startMm", 1, e)} /> mm</span>
      {/if}
      <label class="check" title="where the beam goes after the last path, if you know it"><input type="checkbox" checked={!!opt.endMm} onchange={(e) => setPoint("endMm", (e.currentTarget as HTMLInputElement).checked)} /> end point</label>
      {#if opt.endMm}
        <span class="xy"><input type="number" step="0.1" value={opt.endMm[0]} oninput={(e) => setCoord("endMm", 0, e)} /><input type="number" step="0.1" value={opt.endMm[1]} oninput={(e) => setCoord("endMm", 1, e)} /> mm</span>
      {/if}
    </div>
  </details>
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
  table {
    border-collapse: collapse;
    font-size: 11px;
    width: 100%;
    margin-top: 0.3rem;
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
  .inspect {
    background: none;
    border: none;
    padding: 0;
    color: inherit;
    font: inherit;
    cursor: pointer;
    text-decoration: underline dotted;
  }
  .group-row.inspected {
    background: color-mix(in srgb, #1f6feb 12%, transparent);
  }
  .hint {
    font-size: 11px;
    opacity: 0.7;
  }
</style>
