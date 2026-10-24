<script lang="ts">
  import type { ProjectStore } from "../state/project.svelte.js";

  let { store }: { store: ProjectStore } = $props();
  const touch = () => store.update(() => {});
  const ev = $derived(store.evaluation);
  const exportable = $derived(
    (ev?.passes ?? []).filter((p) => store.group(p.group)?.export && store.layer(p.layer)?.enabled && !store.layer(p.layer)?.referenceOnly),
  );
  const totalPaths = $derived(exportable.reduce((n, p) => n + p.paths, 0));
  const totalLength = $derived(exportable.reduce((n, p) => n + p.lengthMm, 0));
  const blocked = $derived(!ev || ev.hasErrors || totalPaths === 0);
  const size = $derived(ev?.boundsMm ? `${(ev.boundsMm[2] - ev.boundsMm[0] + 2 * store.profile.export.marginMm).toFixed(2)} × ${(ev.boundsMm[3] - ev.boundsMm[1] + 2 * store.profile.export.marginMm).toFixed(2)} mm page` : "");
</script>

<section class="panel">
  <h2>Export</h2>
  <label>PDF stroke width (mm) <input type="number" step="0.0001" min="0.0001" bind:value={store.profile.export.strokeWidthMm} oninput={touch} /></label>
  <label>Page margin (mm) <input type="number" step="0.1" min="0" bind:value={store.profile.export.marginMm} oninput={touch} /></label>
  <label>Arc flattening tolerance (mm) <input type="number" step="0.0001" min="0.00001" bind:value={store.profile.export.flattenToleranceMm} oninput={touch} /></label>
  <p class="hint">
    {totalPaths} trajectories, {totalLength.toFixed(1)} mm total, {size}. 1:1 scale, no fitting.
    Stroke width is for the importer; it is not a kerf, pitch or exposure width.
  </p>
  {#if ev?.hasErrors}<p class="err">Export is blocked until the errors in diagnostics are fixed.</p>{/if}
  <div class="buttons">
    <button disabled={blocked} onclick={() => store.exportPdf()}>Export PDF</button>
    <button disabled={blocked} onclick={() => store.exportPdfPerGroup()}>Export one PDF per group</button>
  </div>
  <p class="hint">Path order in the PDF follows the planned order (see Path order); the laser software may still reorder it.</p>
</section>

<style>
  label {
    display: block;
    font-size: 12px;
    margin: 0.35rem 0;
  }
  input {
    display: block;
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    font-size: 12px;
  }
  .hint {
    font-size: 11px;
    opacity: 0.7;
  }
  .err {
    color: #d33;
    font-size: 12px;
  }
  .buttons {
    display: flex;
    gap: 0.4rem;
  }
</style>
