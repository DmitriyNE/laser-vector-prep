<script lang="ts">
  import type { ProjectStore } from "../state/project.svelte.js";
  import Eye from "./Eye.svelte";

  let { store }: { store: ProjectStore } = $props();
  let dragOver = $state(false);

  function onDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    if (e.dataTransfer?.files?.length) void store.addFiles(e.dataTransfer.files);
  }

  function onPick(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    input.value = "";
    if (files.length) void store.addFiles(files);
  }

  function onMap(e: Event, file: string, layer: string) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    if (v === "+new") store.newBindingFromLayer(file, layer);
    else if (v) store.mapToBinding(v, file, layer);
  }

  const formatLabel = (f: string) =>
    ({ dxf: "DXF", gerber: "Gerber", svg: "SVG", pdf: "PDF", gds: "GDSII", ipc2581: "IPC-2581" })[f] ?? f;

  function size(b: [number, number, number, number] | null) {
    return b ? `${(b[2] - b[0]).toFixed(3)} × ${(b[3] - b[1]).toFixed(3)} mm` : "empty";
  }
</script>

<section class="panel">
  <h2>Sources</h2>
  <!-- Full size until something is imported, then a single line. -->
  <div
    class="drop"
    class:compact={store.sources.length > 0}
    class:over={dragOver}
    title="DXF, Gerber, SVG, PDF, GDSII or IPC-2581"
    role="button"
    tabindex="0"
    ondragover={(e) => {
      e.preventDefault();
      dragOver = true;
    }}
    ondragleave={() => (dragOver = false)}
    ondrop={onDrop}
  >
    {#if store.sources.length === 0}Drop DXF, Gerber, SVG, PDF, GDSII or IPC-2581 files here or{/if}
    <label class="pick"
      >{store.sources.length ? "+ files" : "choose files"}<input type="file" accept={store.acceptedExtensions.map((e) => `.${e}`).join(",")} multiple onchange={onPick} /></label
    >{#if store.sources.length}<span class="or">or drop here</span>{/if}
  </div>
  {#each store.sources as s (s.index)}
    <div class="source">
      <div class="head">
        <Eye hidden={store.sourceFileHidden(s.index)} what="all layers of this file" onToggle={() => store.toggleSourceFile(s.index)} onSolo={() => store.solo({ kind: "file", key: String(s.index) })} />
        <div class="file">
          <strong title={s.name}>{s.name}</strong>
          <span class="meta">
            <span class="format">{formatLabel(s.format)}</span> ·
            <span class="units" class:assumed={!s.units} title={s.units ? "declared units" : "no units in file; assumed"}>
              {s.units ?? `${s.assumedUnits}?`}
            </span>
            · {size(s.boundsMm)}
          </span>
        </div>
        <button class="x" title="remove" onclick={() => store.removeSource(s.index)}>×</button>
      </div>
      <ul>
        {#each s.layers.filter((l) => l.closedPaths + l.openPaths + l.hatches > 0) as l (l.name)}
          {@const key = `${s.index}:${l.name}`}
          {@const slots = store.bindingsOf(s.name, l.name)}
          <li
            class:selected={store.selection.kind === "source" && store.selection.file === s.index && store.selection.layer === l.name}
          >
            <Eye hidden={store.hiddenSourceLayers.has(key)} what="layer" onToggle={() => store.toggleSourceLayer(key)} onSolo={() => store.solo({ kind: "source", key })} />
            <button
              class="name"
              title={`${l.name}${l.function ? ` · ${l.function}` : ""}\n${l.closedPaths} closed, ${l.openPaths} open paths${l.hatches ? `, ${l.hatches} hatches` : ""}`}
              onclick={() => (store.selection = { kind: "source", file: s.index, layer: l.name })}
            >
              {l.name}{#if l.function}<span class="fn"> {l.function}</span>{/if}
            </button>
            <button class="only" onclick={() => store.solo({ kind: "source", key })} title="show only this in preview">only</button>
            <select
              class="map"
              class:unbound={!slots.length}
              value={slots.length > 1 ? "+several" : (slots[0]?.id ?? "")}
              title="profile slot this layer is mapped to (a slot takes one layer; mapping another moves it)"
              onchange={(e) => onMap(e, s.name, l.name)}
            >
              <option value="">{slots.length ? "" : "unmapped"}</option>
              {#if slots.length > 1}<option value="+several" disabled>→ {slots.map((x) => x.name).join(", ")}</option>{/if}
              {#each store.profile.sourceBindings as pb (pb.id)}<option value={pb.id}>→ {pb.name}</option>{/each}
              <option value="+new">+ new slot "{l.name}"</option>
            </select>
          </li>
        {/each}
      </ul>
      {#if s.diagnostics.length}
        <div class="diag">{s.diagnostics.length} note(s), see diagnostics</div>
      {/if}
    </div>
  {/each}
</section>

<style>
  .drop {
    border: 1px dashed color-mix(in srgb, CanvasText 30%, transparent);
    padding: 0.6rem;
    text-align: center;
    border-radius: 4px;
    font-size: 12px;
  }
  .drop.compact {
    padding: 0.25rem;
    text-align: left;
    opacity: 0.85;
  }
  .drop.over {
    border-color: #1f6feb;
    background: color-mix(in srgb, #1f6feb 10%, transparent);
  }
  .pick {
    text-decoration: underline;
    cursor: pointer;
  }
  .pick input {
    display: none;
  }
  .source {
    margin-top: 0.5rem;
    font-size: 12px;
  }
  .head {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .file {
    flex: 1;
    min-width: 0;
  }
  .file strong,
  .file .meta {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1.35;
  }
  .file .meta {
    opacity: 0.7;
    font-size: 11px;
    font-family: ui-monospace, monospace;
  }
  .units.assumed {
    color: #c80;
  }
  .format {
    font-family: system-ui, sans-serif;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: 10px;
  }
  .fn {
    margin-left: 0.3em;
    opacity: 0.6;
    font-size: 11px;
  }
  .or {
    margin-left: 0.3em;
  }
  ul {
    list-style: none;
    margin: 0.25rem 0 0;
    padding: 0;
  }
  li {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    padding: 1px 2px;
  }
  li.selected {
    background: color-mix(in srgb, #1f6feb 15%, transparent);
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: left;
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    color: inherit;
    font: inherit;
  }
  .only {
    background: none;
    border: none;
    padding: 0 2px;
    color: inherit;
    cursor: pointer;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    opacity: 0.7;
    display: none;
  }
  li:hover .only {
    display: inline;
  }
  .map {
    font: inherit;
    font-size: 11px;
    max-width: 9rem;
    color: #2a8;
  }
  .map.unbound {
    color: #c80;
  }
  .x {
    background: none;
    border: none;
    cursor: pointer;
    color: inherit;
  }
  .diag {
    opacity: 0.7;
  }
</style>
