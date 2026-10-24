<script lang="ts">
  import type { ProjectStore } from "../state/project.svelte.js";

  let { store }: { store: ProjectStore } = $props();
  let dragOver = $state(false);

  function onDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    if (e.dataTransfer?.files?.length) void store.addFiles(e.dataTransfer.files);
  }

  function onPick(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    if (input.files?.length) void store.addFiles(input.files);
    input.value = "";
  }

  function onMap(e: Event, file: string, layer: string) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    if (v === "+new") store.newBindingFromLayer(layer);
    else if (v) store.mapToBinding(v, file, layer);
  }

  function size(b: [number, number, number, number] | null) {
    return b ? `${(b[2] - b[0]).toFixed(3)} × ${(b[3] - b[1]).toFixed(3)} mm` : "empty";
  }
</script>

<section class="panel">
  <h2>Sources</h2>
  <div
    class="drop"
    class:over={dragOver}
    role="button"
    tabindex="0"
    ondragover={(e) => {
      e.preventDefault();
      dragOver = true;
    }}
    ondragleave={() => (dragOver = false)}
    ondrop={onDrop}
  >
    Drop DXF files here or
    <label class="pick">choose files<input type="file" accept=".dxf,.DXF" multiple onchange={onPick} /></label>
  </div>
  {#each store.sources as s (s.index)}
    <div class="source">
      <div class="head">
        <strong title={s.name}>{s.name}</strong>
        <span class="units" class:assumed={!s.units} title={s.units ? "declared units" : "no units in file; assumed"}>
          {s.units ?? `${s.assumedUnits}?`}
        </span>
        <span class="size">{size(s.boundsMm)}</span>
        <button class="x" title="remove" onclick={() => store.removeSource(s.index)}>×</button>
      </div>
      <ul>
        {#each s.layers.filter((l) => l.closedPaths + l.openPaths + l.hatches > 0) as l (l.name)}
          {@const key = `${s.index}:${l.name}`}
          {@const b = store.bindingOf(s.name, l.name)}
          <li
            class:selected={store.selection.kind === "source" && store.selection.file === s.index && store.selection.layer === l.name}
          >
            <input
              type="checkbox"
              checked={!store.hiddenSourceLayers.has(key)}
              onchange={() => (store.hiddenSourceLayers = store.toggle(store.hiddenSourceLayers, key))}
              title="show in preview"
            />
            <button class="name" onclick={() => (store.selection = { kind: "source", file: s.index, layer: l.name })}>
              {l.name}
            </button>
            <span class="counts">{l.closedPaths}c {l.openPaths}o{#if l.hatches} {l.hatches}h{/if}</span>
            <select class="map" class:unbound={!b} value={b?.id ?? ""} title="profile slot this layer is mapped to" onchange={(e) => onMap(e, s.name, l.name)}>
              <option value="">{b ? "" : "unmapped"}</option>
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
    gap: 0.5rem;
    align-items: baseline;
  }
  .head strong {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .units {
    font-family: ui-monospace, monospace;
  }
  .units.assumed {
    color: #c80;
  }
  .size {
    opacity: 0.7;
    font-family: ui-monospace, monospace;
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
    text-align: left;
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    color: inherit;
    font: inherit;
  }
  .counts {
    font-family: ui-monospace, monospace;
    opacity: 0.7;
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
