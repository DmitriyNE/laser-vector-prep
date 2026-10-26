<script lang="ts">
  import type { ProjectStore } from "../state/project.svelte.js";

  let { store }: { store: ProjectStore } = $props();
  let importInput: HTMLInputElement;
  let message: string | null = $state(null);

  const current = $derived(store.activeSavedId ? `saved:${store.activeSavedId}` : store.basePresetId ? `preset:${store.basePresetId}` : "");
  /** The selected entry names the profile being edited (renamed, or with unsaved edits). */
  const label = (key: string, name: string) => (key === current ? `${store.profile.name}${store.dirty ? " ● unsaved" : ""}` : name);
  const bindingCounts = $derived(() => {
    const b = store.bindings.bindings;
    return {
      matched: b.filter((x) => x.status === "matched").length,
      missing: b.filter((x) => x.status === "missing").length,
      ambiguous: b.filter((x) => x.status === "ambiguous").length,
    };
  });

  function choose(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    if (store.dirty && !confirm("Discard unsaved profile edits?")) {
      (e.currentTarget as HTMLSelectElement).value = current;
      return;
    }
    if (v.startsWith("preset:")) store.applyPreset(v.slice(7));
    else if (v.startsWith("saved:")) store.applySaved(v.slice(6));
  }

  async function saveAs() {
    const name = prompt("Profile name", store.profile.name);
    if (name) await store.saveAs(name);
  }

  async function rename() {
    const name = prompt("New name", store.profile.name);
    if (name) await store.rename(name);
  }

  async function del() {
    if (store.activeSavedId && confirm(`Delete saved profile "${store.profile.name}"?`)) await store.deleteSaved();
  }

  async function onImport(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    const text = await file.text();
    try {
      const conflict = store.savedConflict(text);
      if (conflict) {
        const overwrite = confirm(`A saved profile "${conflict.name}" with this id or name exists.\nOK = overwrite it after import, Cancel = import as a copy.`);
        const diags = await store.importJson(text);
        if (diags.some((d) => d.severity === "error")) {
          message = diags.filter((d) => d.severity === "error").map((d) => d.message).join("; ");
          return;
        }
        if (overwrite) {
          store.activeSavedId = conflict.id;
          await store.save();
        } else {
          store.profile.profileId = crypto.randomUUID();
          store.profile.name = `${store.profile.name} (imported)`;
        }
      } else {
        const diags = await store.importJson(text);
        if (diags.some((d) => d.severity === "error")) {
          message = diags.filter((d) => d.severity === "error").map((d) => d.message).join("; ");
          return;
        }
      }
      message = "Profile imported (unsaved until you save it).";
    } catch (err) {
      message = err instanceof Error ? err.message : String(err);
    }
  }
</script>

<div class="bar">
  <strong title={`core ${store.coreVersion} · ${store.threads ?? "…"} threads`}>Laser Vector Prep</strong>
  <!-- An imported profile not saved yet is in neither list: the first entry names it. -->
  <select value={current} onchange={choose} title="profile" class:dirty={store.dirty}>
    <option value="" disabled>{current === "" ? label("", "") : "— choose profile —"}</option>
    <optgroup label="Built-in (read-only)">
      {#each store.presets as p (p.profileId)}<option value={"preset:" + p.profileId}>{label("preset:" + p.profileId, p.name)}</option>{/each}
    </optgroup>
    <optgroup label="Saved">
      {#each store.saved as s (s.id)}<option value={"saved:" + s.id}>{label("saved:" + s.id, s.name)}</option>{/each}
    </optgroup>
  </select>
  <button onclick={() => store.save()} title="save (built-ins are saved as a new profile)">Save</button>
  <button onclick={saveAs}>Save as…</button>
  <button onclick={rename} disabled={!store.activeSavedId && !store.dirty}>Rename</button>
  <button onclick={() => store.duplicate()}>Duplicate</button>
  <button onclick={del} disabled={!store.activeSavedId}>Delete</button>
  <button onclick={() => importInput.click()}>Import JSON</button>
  <input bind:this={importInput} type="file" accept=".json,application/json" hidden onchange={onImport} />
  <button onclick={() => store.exportJson()}>Export JSON</button>
  <button onclick={() => store.reset()} disabled={!store.dirty}>Reset</button>
  <button onclick={() => store.undo()} disabled={!store.canUndo} title="undo the last profile edit (Ctrl/⌘ Z)">Undo</button>
  <button onclick={() => store.redo()} disabled={!store.canRedo} title="redo (Ctrl/⌘ ⇧ Z)">Redo</button>
  <span class="status">
    {#if store.bindings.bindings.length || store.sources.length}
      {@const c = bindingCounts()}
      <span class="ok">{c.matched} matched</span>
      {#if c.missing}<span class="err">{c.missing} missing</span>{/if}
      {#if c.ambiguous}<span class="err">{c.ambiguous} ambiguous</span>{/if}
      {#if store.bindings.unboundLayers.length}<span class="meta">{store.bindings.unboundLayers.length} unbound</span>{/if}
    {/if}
  </span>
  {#if message}<span class="msg">{message} <button onclick={() => (message = null)}>×</button></span>{/if}
  {#if store.error}<span class="err">{store.error}</span>{/if}
  {#if store.notice}<span class="msg">{store.notice}</span>{/if}
</div>

<style>
  .bar {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    padding: 0.4rem 0.75rem;
    border-bottom: 1px solid color-mix(in srgb, CanvasText 20%, transparent);
    font-size: 12px;
    flex-wrap: wrap;
  }
  select {
    font-weight: 600;
  }
  .status {
    margin-left: auto;
    display: flex;
    gap: 0.6rem;
  }
  .ok {
    color: #2a8;
  }
  .err {
    color: #d33;
  }
  .meta {
    opacity: 0.65;
    font-family: ui-monospace, monospace;
  }
  .msg {
    background: color-mix(in srgb, #1f6feb 15%, transparent);
    padding: 1px 6px;
    border-radius: 3px;
  }
  button {
    font-size: 12px;
  }
</style>
