<script lang="ts">
  import type { Pass, WorkingLayer } from "../core/types.js";
  import { rgbCss, type ProjectStore } from "../state/project.svelte.js";
  import { shortId } from "../util/download.js";

  let { store }: { store: ProjectStore } = $props();

  const summaryOf = (id: string) => store.evaluation?.layers.find((l) => l.id === id);
  const passSummary = (id: string) => store.evaluation?.passes.find((p) => p.id === id);
  const errors = (ds: { severity: string }[] | undefined) => ds?.filter((d) => d.severity === "error").length ?? 0;

  function addLayer() {
    const id = shortId("wl");
    store.update((p) => {
      p.workingLayers.push({
        id,
        name: `Layer ${p.workingLayers.length + 1}`,
        kind: "region",
        inputs: [],
        op: "union",
        interpretation: { loopRule: "mergeThenNest", strokeWidth: "original" },
        passes: [],
        enabled: true,
        referenceOnly: false,
      });
    });
    store.selection = { kind: "layer", id };
  }

  function addPass(layer: WorkingLayer) {
    const id = shortId("pass");
    const group = store.profile.outputGroups[0]?.id ?? "";
    store.update(() => {
      layer.passes.push({
        id,
        name: layer.kind === "region" ? "Contour fill" : "Passthrough",
        pattern: layer.kind === "region" ? { kind: "contourFill", pitchMm: 0.02, firstOffsetMm: null, gapFill: true } : { kind: "passthrough", reverse: false },
        group,
        enabled: true,
      });
    });
    store.selection = { kind: "pass", id };
  }

  function duplicateLayer(i: number) {
    store.update((p) => {
      const src = p.workingLayers[i]!;
      const copy: WorkingLayer = structuredClone($state.snapshot(src));
      copy.id = shortId("wl");
      copy.name = `${src.name} (copy)`;
      copy.passes = copy.passes.map((ps) => ({ ...ps, id: shortId("pass") }));
      p.workingLayers.splice(i + 1, 0, copy);
    });
  }

  function deleteLayer(i: number) {
    store.update((p) => {
      p.workingLayers.splice(i, 1);
    });
    store.selection = { kind: "none" };
  }

  function move(i: number, d: number) {
    store.update((p) => {
      const j = i + d;
      if (j < 0 || j >= p.workingLayers.length) return;
      const [l] = p.workingLayers.splice(i, 1);
      p.workingLayers.splice(j, 0, l!);
    });
  }

  function deletePass(layer: WorkingLayer, pass: Pass) {
    store.update(() => {
      layer.passes.splice(layer.passes.indexOf(pass), 1);
    });
    store.selection = { kind: "none" };
  }

  function patternLabel(p: Pass) {
    return p.pattern.kind === "contourFill" ? `contour ${p.pattern.pitchMm} mm${p.pattern.gapFill ? " +gap" : ""}` : "passthrough";
  }

  function usedBy(id: string) {
    return store.profile.workingLayers.filter((l) => l.inputs.some((o) => "layer" in o && o.layer === id)).map((l) => l.name);
  }
</script>

<section class="panel">
  <h2>Layers <button class="add" onclick={addLayer}>+ layer</button></h2>
  {#if store.profile.workingLayers.length === 0}
    <p class="hint">No working layers. Add one and pick its sources.</p>
  {/if}
  {#each store.profile.workingLayers as layer, i (layer.id)}
    {@const sum = summaryOf(layer.id)}
    {@const errs = errors(sum?.diagnostics) + (store.evaluation?.diagnostics.filter((d) => d.context === layer.id && d.severity === "error").length ?? 0)}
    <div class="layer" class:selected={store.selection.kind === "layer" && store.selection.id === layer.id}>
      <div class="row">
        <input type="checkbox" checked={!store.hiddenLayers.has(layer.id)} title="show region in preview"
          onchange={() => (store.hiddenLayers = store.toggle(store.hiddenLayers, layer.id))} />
        <input type="checkbox" bind:checked={layer.enabled} title="enabled (outputs marks)" onchange={() => store.update(() => {})} />
        <button class="name" onclick={() => (store.selection = { kind: "layer", id: layer.id })}>
          {layer.name}
          <span class="meta">{layer.kind} · {layer.op}{layer.referenceOnly ? " · reference" : ""}</span>
        </button>
        {#if sum}
          <span class="meta">
            {#if layer.kind === "region"}{sum.outers} island{sum.outers === 1 ? "" : "s"}, {sum.holes} hole{sum.holes === 1 ? "" : "s"}{:else}{sum.paths} path{sum.paths === 1 ? "" : "s"}{/if}
          </span>
        {/if}
        {#if errs}<span class="err" title="errors">{errs}!</span>{/if}
        <span class="tools">
          <button onclick={() => move(i, -1)} title="move up">↑</button>
          <button onclick={() => move(i, 1)} title="move down">↓</button>
          <button onclick={() => duplicateLayer(i)} title="duplicate">⧉</button>
          <button onclick={() => deleteLayer(i)} title="delete">×</button>
        </span>
      </div>
      {#if usedBy(layer.id).length}
        <div class="meta uses">used as input by {usedBy(layer.id).join(", ")}</div>
      {/if}
      <ul>
        {#each layer.passes as pass (pass.id)}
          {@const ps = passSummary(pass.id)}
          {@const g = store.group(pass.group)}
          <li class:selected={store.selection.kind === "pass" && store.selection.id === pass.id}>
            <input type="checkbox" checked={!store.hiddenPasses.has(pass.id)} title="show paths in preview"
              onchange={() => (store.hiddenPasses = store.toggle(store.hiddenPasses, pass.id))} />
            <input type="checkbox" bind:checked={pass.enabled} title="enabled" onchange={() => store.update(() => {})} />
            <span class="swatch" style:background={g ? rgbCss(g.color) : "transparent"} title={g?.name ?? "no group"}></span>
            <button class="name" onclick={() => (store.selection = { kind: "pass", id: pass.id })}>
              {pass.name} <span class="meta">{patternLabel(pass)}</span>
            </button>
            {#if ps}<span class="meta">{ps.paths} paths, {ps.lengthMm.toFixed(1)} mm</span>{/if}
            {#if errors(ps?.diagnostics)}<span class="err">!</span>{/if}
            <button onclick={() => deletePass(layer, pass)} title="delete pass">×</button>
          </li>
        {/each}
        <li><button class="add" onclick={() => addPass(layer)}>+ pass</button></li>
      </ul>
    </div>
  {/each}
</section>

<style>
  .layer {
    margin-top: 0.4rem;
    padding: 0.2rem;
    border-radius: 4px;
    font-size: 12px;
  }
  .layer.selected,
  li.selected {
    background: color-mix(in srgb, #1f6feb 15%, transparent);
  }
  .row,
  li {
    display: flex;
    gap: 0.35rem;
    align-items: center;
  }
  .name {
    flex: 1;
    text-align: left;
    background: none;
    border: none;
    padding: 0;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .meta {
    opacity: 0.65;
    font-size: 11px;
  }
  .uses {
    margin-left: 2.6rem;
  }
  .err {
    color: #d33;
    font-weight: bold;
  }
  .tools button,
  li > button {
    background: none;
    border: none;
    cursor: pointer;
    color: inherit;
    padding: 0 2px;
  }
  ul {
    list-style: none;
    margin: 0.2rem 0 0 1.2rem;
    padding: 0;
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    border: 1px solid color-mix(in srgb, CanvasText 30%, transparent);
    display: inline-block;
  }
  .add {
    font-size: 11px;
  }
  .hint {
    font-size: 12px;
    opacity: 0.7;
  }
</style>
