<script lang="ts">
  import { ruleText } from "../state/labels.js";
  import type { ProjectStore } from "../state/project.svelte.js";

  let { store }: { store: ProjectStore } = $props();

  const status = (id: string) => store.bindings.bindings.find((b) => b.id === id);
  const imported = $derived(
    store.sources.flatMap((s) =>
      s.layers.filter((l) => l.closedPaths + l.openPaths + l.hatches > 0).map((l) => ({ file: s.name, layer: l.name, key: `${s.name}\u0000${l.name}` })),
    ),
  );

  function onMap(e: Event, bindingId: string) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    if (!v) return;
    const [file, layer] = v.split("\u0000");
    if (file && layer) store.mapToBinding(bindingId, file, layer);
  }

</script>

<section class="panel">
  <h2 title="the source layers this profile needs: imported layers are mapped onto these slots by each slot's rules, or explicitly here; working layers read from the slots">Profile sources <button class="add" onclick={() => (store.selection = { kind: "binding", id: store.addBinding("New source", null) })}>+ slot</button></h2>
  {#if store.profile.sourceBindings.length === 0}
    <p class="hint">No slots yet. Import a file and use "+ new slot" on one of its layers.</p>
  {/if}
  {#each store.profile.sourceBindings as b (b.id)}
    {@const st = status(b.id)}
    {@const users = store.bindingUsers(b.id)}
    <div class="slot" class:selected={store.selection.kind === "binding" && store.selection.id === b.id}>
      <div class="row">
        <button
          class="name"
          title={`rules: ${b.rules.map(ruleText).join("; ") || "none"}\n${users.length ? `used by ${users.join(", ")}` : "not used by any working layer"}`}
          onclick={() => (store.selection = { kind: "binding", id: b.id })}
        >
          {b.name}{#if b.required}<span class="req" title="required: output is blocked while missing">*</span>{/if}
        </button>
        <span class="status {st?.status ?? ''}">{st?.status ?? "…"}</span>
        <select class="map" value={st?.status === "matched" ? `${st.file}\u0000${st.layer}` : ""} onchange={(e) => onMap(e, b.id)} title="map an imported layer onto this slot">
          <option value="">{st?.status === "matched" ? "" : "— map imported layer —"}</option>
          {#each imported as il (il.key)}<option value={il.key}>{il.file} : {il.layer}</option>{/each}
        </select>
      </div>
      {#if st?.status === "ambiguous"}<div class="meta err">{st.candidates.length} layers match: pick one</div>{/if}
    </div>
  {/each}
  {#if store.bindings.unboundLayers.length}
    <div class="extra">
      <span class="meta">imported, not mapped:</span>
      {#each store.bindings.unboundLayers as [file, layer] (file + layer)}
        <span class="chip">{file} : {layer} <button title="new slot matching this layer" onclick={() => store.newBindingFromLayer(file, layer)}>+ slot</button></span>
      {/each}
    </div>
  {/if}
</section>

<style>
  .slot {
    padding: 0.2rem;
    border-radius: 4px;
    font-size: 12px;
  }
  .slot.selected {
    background: color-mix(in srgb, #1f6feb 15%, transparent);
  }
  .row {
    display: flex;
    gap: 0.4rem;
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
    font-weight: 600;
  }
  .req {
    color: #c80;
    margin-left: 2px;
  }
  .status {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .status.matched {
    color: #2a8;
  }
  .status.missing,
  .status.ambiguous {
    color: #d33;
  }
  .map {
    font: inherit;
    font-size: 11px;
    max-width: 11rem;
  }
  .meta {
    opacity: 0.65;
    font-size: 11px;
    margin-left: 0.1rem;
  }
  .err {
    color: #d33;
    opacity: 1;
  }
  .extra {
    margin-top: 0.4rem;
    font-size: 11px;
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    align-items: center;
  }
  .chip {
    border: 1px solid color-mix(in srgb, CanvasText 20%, transparent);
    border-radius: 3px;
    padding: 1px 4px;
  }
  .chip button,
  .add {
    font-size: 11px;
  }
  .hint {
    font-size: 11px;
    opacity: 0.7;
    margin: 0.2rem 0 0.4rem;
  }
</style>
