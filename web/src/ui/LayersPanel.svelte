<script lang="ts">
  import type { Pass, WorkingLayer } from "../core/types.js";
  import { rgbCss, type ProjectStore } from "../state/project.svelte.js";
  import { shortId } from "../util/download.js";
  import Eye from "./Eye.svelte";

  let { store }: { store: ProjectStore } = $props();

  const summaryOf = (id: string) => store.evaluation?.layers.find((l) => l.id === id);
  const passSummary = (id: string) => store.evaluation?.passes.find((p) => p.id === id);
  const errors = (ds: { severity: string }[] | undefined) => ds?.filter((d) => d.severity === "error").length ?? 0;
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

  function addLayer(kind: "region" | "envelope" = "region") {
    const id = shortId("wl");
    store.update((p) => {
      p.workingLayers.push({
        id,
        name: kind === "envelope" ? `Envelope ${p.workingLayers.length + 1}` : `Layer ${p.workingLayers.length + 1}`,
        kind,
        inputs: [],
        operands: [],
        op: "union",
        envelope: kind === "envelope" ? { provider: "boundingBox", marginMm: 0.5 } : null,
        interpretation: { loopRule: "mergeThenNest", strokeWidth: "original" },
        passes: [],
        enabled: true,
        // Envelopes are domains for other layers; they never mark unless asked explicitly.
        referenceOnly: kind === "envelope",
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
    const pt = p.pattern;
    switch (pt.kind) {
      case "contourFill":
        return `contour ${pt.pitchMm} mm${pt.gapFill ? " +gap" : ""}`;
      case "boundaryTrace":
        return `trace ${pt.side}${pt.offsetMm ? ` ${pt.offsetMm} mm` : ""}${pt.kerfMm ? ` kerf ${pt.kerfMm}` : ""}${pt.rings > 1 ? ` ×${pt.rings}` : ""}`;
      case "hatch":
        return `hatch ${pt.pitchMm} mm @ ${pt.angleDeg}°`;
      case "crossHatch":
        return `crosshatch ${pt.pitchMm} mm @ ${pt.anglesDeg.join("/")}°`;
      case "boundaryHatch":
        return `${pt.shells} shell${pt.shells === 1 ? "" : "s"} + hatch ${pt.pitchMm} mm`;
      default:
        return "passthrough";
    }
  }

  function envelopeLabel(l: WorkingLayer) {
    const e = l.envelope;
    if (!e) return "envelope";
    switch (e.provider) {
      case "namedBoundary":
        return "named boundary";
      case "boundingBox":
        return `box +${e.marginMm} mm`;
      case "boundingCircle":
        return `circle +${e.marginMm} mm`;
      case "buffer":
        return `buffer ${e.distanceMm} mm`;
      case "convexHull":
        return `hull +${e.distanceMm} mm`;
    }
  }

  function usedBy(id: string) {
    return store.profile.workingLayers
      .filter((l) => [...l.inputs, ...l.operands].some((o) => "layer" in o && o.layer === id))
      .map((l) => l.name);
  }

  function opLabel(l: WorkingLayer) {
    if (l.kind === "envelope") return envelopeLabel(l);
    return l.op === "union" ? "union" : l.op === "intersection" ? "A ∩ B" : l.op === "subtraction" ? "A − B" : "A xor B";
  }

  /** Second line of a layer row: kind, operation, flags and who reads it. */
  function layerDetail(l: WorkingLayer) {
    const parts = [l.kind, opLabel(l)];
    if (l.referenceOnly) parts.push("reference");
    if (!l.enabled) parts.push("disabled");
    const users = usedBy(l.id);
    if (users.length) parts.push(`used by ${users.join(", ")}`);
    return parts.join(" · ");
  }

  function layerStats(l: WorkingLayer) {
    const sum = summaryOf(l.id);
    if (!sum) return "";
    return l.kind === "linework" ? plural(sum.paths, "path") : `${plural(sum.outers, "island")}, ${plural(sum.holes, "hole")}`;
  }

  function layerErrors(l: WorkingLayer) {
    return errors(summaryOf(l.id)?.diagnostics) + (store.evaluation?.diagnostics.filter((d) => d.context === l.id && d.severity === "error").length ?? 0);
  }
</script>

<section class="panel">
  <h2>
    Layers <button class="add" onclick={() => addLayer("region")}>+ layer</button>
    <button class="add" onclick={() => addLayer("envelope")} title="a bounded domain around a reference set, for negative regions">+ envelope</button>
    {#if store.hiddenCount}<button class="add quiet" onclick={() => store.showAll()} title="show everything in the preview again">show all</button>{/if}
  </h2>
  {#if store.profile.workingLayers.length === 0}
    <p class="hint">No working layers. Add one and pick its sources.</p>
  {/if}
  {#each store.profile.workingLayers as layer, i (layer.id)}
    {@const errs = layerErrors(layer)}
    {@const hidden = store.hiddenLayers.has(layer.id)}
    <div class="layer" class:selected={store.selection.kind === "layer" && store.selection.id === layer.id} class:disabled={!layer.enabled}>
      <div class="row">
        <Eye {hidden} what="layer and its passes" onToggle={() => store.toggleLayer(layer.id)} onSolo={() => store.solo({ kind: "layer", key: layer.id })} />
        <button class="name" title={layer.name} onclick={() => (store.selection = { kind: "layer", id: layer.id })}>
          <span class="title">{layer.name}</span>
          <span class="sub" title={layerDetail(layer)}>{layerDetail(layer)}</span>
        </button>
        <span class="stats">
          {#if errs}<span class="err" title="errors">{errs}!</span>{/if}
          {layerStats(layer)}
        </span>
        <span class="tools">
          <button class="only" onclick={() => store.solo({ kind: "layer", key: layer.id })} title="show only this in preview">only</button>
          <button onclick={() => move(i, -1)} title="move up">↑</button>
          <button onclick={() => move(i, 1)} title="move down">↓</button>
          <button onclick={() => duplicateLayer(i)} title="duplicate">⧉</button>
          <button onclick={() => deleteLayer(i)} title="delete layer">×</button>
        </span>
      </div>
      <ul>
        {#each layer.passes as pass (pass.id)}
          {@const ps = passSummary(pass.id)}
          {@const g = store.group(pass.group)}
          {@const passHidden = store.hiddenPasses.has(pass.id)}
          <li class="row pass" class:selected={store.selection.kind === "pass" && store.selection.id === pass.id} class:disabled={!pass.enabled}>
            <Eye hidden={passHidden} what="paths" onToggle={() => store.togglePass(pass.id)} onSolo={() => store.solo({ kind: "pass", key: pass.id })} />
            <button class="name" title={pass.name} onclick={() => (store.selection = { kind: "pass", id: pass.id })}>
              <span class="title"><span class="swatch" style:background={g ? rgbCss(g.color) : "transparent"} title={g?.name ?? "no group"}></span>{pass.name}</span>
              <span class="sub">{patternLabel(pass)}{pass.enabled ? "" : " · disabled"}</span>
            </button>
            <span class="stats two">
              {#if errors(ps?.diagnostics)}<span class="err" title="errors">!</span>{/if}
              {#if ps}<span>{plural(ps.paths, "path")}</span><span>{ps.lengthMm.toFixed(ps.lengthMm >= 100 ? 0 : 1)} mm</span>{/if}
            </span>
            <span class="tools">
              <button class="only" onclick={() => store.solo({ kind: "pass", key: pass.id })} title="show only this in preview">only</button>
              <button onclick={() => deletePass(layer, pass)} title="delete pass">×</button>
            </span>
          </li>
        {/each}
        <li class="addrow"><button class="add" onclick={() => addPass(layer)}>+ pass</button></li>
      </ul>
    </div>
  {/each}
</section>

<style>
  .layer {
    margin-top: 0.3rem;
    padding: 0.15rem 0.2rem;
    border-radius: 4px;
    font-size: 12px;
  }
  .layer.selected > .row,
  li.selected {
    background: color-mix(in srgb, #1f6feb 15%, transparent);
    border-radius: 3px;
  }
  /* eye | name (two lines) | stats over tools */
  .row {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    grid-template-rows: auto auto;
    column-gap: 0.35rem;
    align-items: start;
    padding: 0.1rem 0.15rem;
  }
  .row > :global(.eye) {
    grid-row: 1 / 3;
    align-self: center;
  }
  .name {
    grid-column: 2;
    grid-row: 1 / 3;
    min-width: 0;
    display: block;
    text-align: left;
    background: none;
    border: none;
    padding: 0;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .title,
  .sub {
    display: block;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    line-height: 1.35;
  }
  .sub {
    opacity: 0.6;
    font-size: 11px;
  }
  .stats {
    grid-column: 3;
    grid-row: 1;
    white-space: nowrap;
    opacity: 0.75;
    font-size: 11px;
    line-height: calc(12px * 1.35);
    text-align: right;
  }
  /* Pass rows: paths over length, delete tool in its own column. */
  .pass {
    grid-template-columns: auto minmax(0, 1fr) auto auto;
  }
  .stats.two {
    grid-row: 1 / 3;
  }
  .stats.two > span {
    display: block;
    line-height: calc(11px * 1.35 + 0.5px);
  }
  .pass > .tools {
    grid-column: 4;
    grid-row: 1 / 3;
    align-self: center;
  }
  .tools {
    grid-column: 3;
    grid-row: 2;
    justify-self: end;
    white-space: nowrap;
    visibility: hidden;
    line-height: 1;
  }
  .row:hover > .tools,
  .layer.selected > .row > .tools,
  li.selected > .tools {
    visibility: visible;
  }
  .tools button {
    background: none;
    border: none;
    cursor: pointer;
    color: inherit;
    padding: 0 3px;
    font-size: 12px;
    opacity: 0.7;
  }
  .tools button:hover {
    opacity: 1;
  }
  .tools .only {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .quiet {
    opacity: 0.6;
  }
  .disabled > .row > .name,
  li.disabled > .name,
  .disabled > .row > .stats,
  li.disabled > .stats {
    opacity: 0.45;
  }
  .err {
    color: #d33;
    font-weight: bold;
    margin-right: 0.3rem;
  }
  ul {
    list-style: none;
    margin: 0 0 0 1.1rem;
    padding: 0;
  }
  .addrow {
    padding: 0 0 0 1.5rem;
  }
  .addrow .add {
    background: none;
    border: none;
    padding: 0 2px;
    color: inherit;
    opacity: 0.55;
    cursor: pointer;
  }
  .addrow .add:hover {
    opacity: 1;
  }
  .swatch {
    width: 9px;
    height: 9px;
    border-radius: 2px;
    border: 1px solid color-mix(in srgb, CanvasText 30%, transparent);
    display: inline-block;
    vertical-align: baseline;
    margin-right: 0.3rem;
  }
  .add {
    font-size: 11px;
  }
  .hint {
    font-size: 12px;
    opacity: 0.7;
  }
</style>
