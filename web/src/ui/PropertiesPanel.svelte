<script lang="ts">
  import type { MatchRule, Operand, StrokeWidth } from "../core/types.js";
  import { hexRgb, rgbHex, type ProjectStore } from "../state/project.svelte.js";

  let { store }: { store: ProjectStore } = $props();

  const sel = $derived(store.selection);
  const layer = $derived(sel.kind === "layer" ? store.layer(sel.id) : undefined);
  const passInfo = $derived(sel.kind === "pass" ? store.pass(sel.id) : undefined);
  const group = $derived(sel.kind === "group" ? store.group(sel.id) : undefined);
  const sourceSel = $derived(sel.kind === "source" ? sel : undefined);
  const sourceFile = $derived(sourceSel ? store.sources.find((s) => s.index === sourceSel.file) : undefined);
  const sourceLayer = $derived(sourceFile && sourceSel ? sourceFile.layers.find((l) => l.name === sourceSel.layer) : undefined);

  const touch = () => store.update(() => {});

  function hasInput(ops: Operand[], o: Operand) {
    return ops.some((x) => JSON.stringify(x) === JSON.stringify(o));
  }

  function toggleInput(ops: Operand[], o: Operand) {
    store.update(() => {
      const i = ops.findIndex((x) => JSON.stringify(x) === JSON.stringify(o));
      if (i >= 0) ops.splice(i, 1);
      else ops.push(o);
    });
  }

  function strokeMode(sw: StrokeWidth): "none" | "original" | "fixed" {
    return typeof sw === "string" ? sw : "fixed";
  }

  function setStrokeMode(mode: string) {
    if (!layer) return;
    store.update(() => {
      layer.interpretation.strokeWidth = mode === "fixed" ? { fixed: { mm: 0.1 } } : (mode as "none" | "original");
    });
  }

  function setPatternKind(kind: string) {
    if (!passInfo) return;
    store.update(() => {
      passInfo.pass.pattern =
        kind === "contourFill" ? { kind: "contourFill", pitchMm: 0.02, firstOffsetMm: null, gapFill: true } : { kind: "passthrough", reverse: false };
    });
  }

  /** Explicit mapping of the selected source layer onto a binding (front of its rules). */
  function bindTo(bindingId: string) {
    if (!sourceFile || !sourceLayer) return;
    store.update((p) => {
      const b = p.sourceBindings.find((x) => x.id === bindingId);
      if (!b) return;
      const rule: MatchRule = { kind: "fileLayer", file: sourceFile.name, name: sourceLayer.name };
      b.rules = [rule, ...b.rules.filter((r) => JSON.stringify(r) !== JSON.stringify(rule))];
    });
  }

  function addBinding() {
    if (!sourceFile || !sourceLayer) return;
    store.update((p) => {
      p.sourceBindings.push({
        id: `src-${crypto.randomUUID().slice(0, 8)}`,
        name: sourceLayer.name,
        required: false,
        rules: [{ kind: "layerName", format: "dxf", name: sourceLayer.name }],
      });
    });
  }
</script>

<section class="panel">
  <h2>Properties</h2>
  {#if layer}
    <label>Name <input bind:value={layer.name} oninput={touch} /></label>
    <label>Geometry
      <select bind:value={layer.kind} onchange={touch}>
        <option value="region">region (filled area)</option>
        <option value="linework">linework (centrelines)</option>
      </select>
    </label>
    <label>Combine
      <select bind:value={layer.op} onchange={touch} disabled={layer.kind === "linework"}>
        <option value="union">union</option>
        <option value="intersection">intersection</option>
        <option value="subtraction">subtraction (first − rest)</option>
        <option value="xor">xor</option>
      </select>
    </label>
    <fieldset>
      <legend>Inputs</legend>
      {#each store.profile.sourceBindings as b (b.id)}
        <label class="check">
          <input type="checkbox" checked={hasInput(layer.inputs, { source: b.id })} onchange={() => toggleInput(layer.inputs, { source: b.id })} />
          source: {b.name}
        </label>
      {/each}
      {#each store.profile.workingLayers.filter((l) => l.id !== layer.id) as l (l.id)}
        <label class="check">
          <input type="checkbox" checked={hasInput(layer.inputs, { layer: l.id })} onchange={() => toggleInput(layer.inputs, { layer: l.id })} />
          layer: {l.name}
        </label>
      {/each}
      {#if layer.op === "subtraction"}<p class="hint">Subtraction order follows the input order above: the first checked input is the subject.</p>{/if}
    </fieldset>
    {#if layer.kind === "region"}
      <label>Closed loops
        <select bind:value={layer.interpretation.loopRule} onchange={touch}>
          <option value="mergeThenNest">merge overlaps, nested = holes</option>
          <option value="evenOdd">even-odd</option>
          <option value="nonZero">non-zero (no holes)</option>
        </select>
      </label>
      <label>Open paths
        <select value={strokeMode(layer.interpretation.strokeWidth)} onchange={(e) => setStrokeMode((e.currentTarget as HTMLSelectElement).value)}>
          <option value="original">expand by source width</option>
          <option value="fixed">expand by fixed width</option>
          <option value="none">not allowed (error)</option>
        </select>
      </label>
      {#if typeof layer.interpretation.strokeWidth === "object"}
        <label>Width (mm) <input type="number" step="0.01" min="0.001" bind:value={layer.interpretation.strokeWidth.fixed.mm} oninput={touch} /></label>
      {/if}
    {/if}
    <label class="check"><input type="checkbox" bind:checked={layer.referenceOnly} onchange={touch} /> reference only (never marks)</label>
  {:else if passInfo}
    {@const pass = passInfo.pass}
    <label>Name <input bind:value={pass.name} oninput={touch} /></label>
    <label>Pattern
      <select value={pass.pattern.kind} onchange={(e) => setPatternKind((e.currentTarget as HTMLSelectElement).value)}>
        <option value="contourFill">inward contour fill</option>
        <option value="passthrough">linework passthrough</option>
      </select>
    </label>
    {#if pass.pattern.kind === "contourFill"}
      <label>Pitch (mm) <input type="number" step="0.001" min="0.0005" bind:value={pass.pattern.pitchMm} oninput={touch} /></label>
      <label>First offset (mm, empty = pitch/2)
        <input type="number" step="0.001" min="0" value={pass.pattern.firstOffsetMm ?? ""}
          oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { if (pass.pattern.kind === "contourFill") pass.pattern.firstOffsetMm = v === "" ? null : Number(v); }); }} />
      </label>
      <label class="check"><input type="checkbox" bind:checked={pass.pattern.gapFill} onchange={touch} /> centreline gap fill for narrow cores</label>
    {:else}
      <label class="check"><input type="checkbox" bind:checked={pass.pattern.reverse} onchange={touch} /> reverse direction</label>
      <label>Simplify (mm, empty = off)
        <input type="number" step="0.001" min="0" value={pass.pattern.simplifyMm ?? ""}
          oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { if (pass.pattern.kind === "passthrough") pass.pattern.simplifyMm = v === "" ? null : Number(v); }); }} />
      </label>
    {/if}
    <label>Output group
      <select bind:value={pass.group} onchange={touch}>
        {#each store.profile.outputGroups as g (g.id)}<option value={g.id}>{g.name}</option>{/each}
      </select>
    </label>
    <label class="check"><input type="checkbox" bind:checked={pass.enabled} onchange={touch} /> enabled</label>
    <p class="hint">Layer: {passInfo.layer.name}. Pitch and offsets are centreline distances in mm, independent of the PDF stroke width.</p>
  {:else if group}
    <label>Name <input bind:value={group.name} oninput={touch} /></label>
    <label>Colour
      <input type="color" value={rgbHex(group.color)} oninput={(e) => store.update(() => { group.color = hexRgb((e.currentTarget as HTMLInputElement).value); })} />
      <code>rgb({group.color.join(", ")})</code>
    </label>
    <label>PDF stroke width (mm, empty = export default)
      <input type="number" step="0.0001" min="0.0001" value={group.strokeWidthMm ?? ""}
        oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { group.strokeWidthMm = v === "" ? null : Number(v); }); }} />
    </label>
    <label class="check"><input type="checkbox" bind:checked={group.export} onchange={touch} /> export</label>
    <label class="check"><input type="checkbox" bind:checked={group.visible} onchange={touch} /> visible in preview</label>
  {:else if sourceLayer && sourceFile}
    <p><strong>{sourceLayer.name}</strong> in {sourceFile.name}</p>
    <p class="hint">
      {sourceLayer.closedPaths} closed, {sourceLayer.openPaths} open path(s), {sourceLayer.hatches} hatch(es).
      {#if sourceLayer.boundsMm}Bounds {sourceLayer.boundsMm.map((v) => v.toFixed(3)).join(", ")} mm.{/if}
      Source layers are immutable; working layers reference them through bindings.
    </p>
    <label>Bind to
      <select onchange={(e) => { const v = (e.currentTarget as HTMLSelectElement).value; if (v) bindTo(v); (e.currentTarget as HTMLSelectElement).value = ""; }}>
        <option value="">choose a binding…</option>
        {#each store.profile.sourceBindings as b (b.id)}<option value={b.id}>{b.name}</option>{/each}
      </select>
    </label>
    <button onclick={addBinding}>new binding for this layer name</button>
  {:else}
    <p class="hint">Select a source layer, working layer, pass or output group.</p>
  {/if}
</section>

<style>
  label {
    display: block;
    font-size: 12px;
    margin: 0.35rem 0;
  }
  label.check {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  input:not([type="checkbox"]):not([type="color"]),
  select {
    display: block;
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    font-size: 12px;
    margin-top: 2px;
  }
  fieldset {
    border: 1px solid color-mix(in srgb, CanvasText 20%, transparent);
    margin: 0.4rem 0;
    padding: 0.3rem 0.5rem;
    font-size: 12px;
  }
  .hint {
    font-size: 11px;
    opacity: 0.7;
  }
  code {
    font-size: 11px;
    margin-left: 0.5rem;
  }
</style>
