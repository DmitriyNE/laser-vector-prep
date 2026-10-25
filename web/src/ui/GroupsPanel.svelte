<script lang="ts">
  import { rgbCss, type ProjectStore } from "../state/project.svelte.js";

  let { store }: { store: ProjectStore } = $props();

  function remove(id: string) {
    const users = store.groupUsers(id);
    if (!store.removeGroup(id)) store.notice = `Group is used by ${users.join(", ")}; move those passes to another group first.`;
  }

  const collisions = $derived(new Set(
    store.profile.outputGroups
      .filter((g) => g.export)
      .filter((g, _, all) => all.some((o) => o !== g && o.export && o.color.join() === g.color.join()))
      .map((g) => g.id),
  ));
  const usage = (id: string) => store.profile.workingLayers.flatMap((l) => l.passes).filter((p) => p.group === id).length;
</script>

<section class="panel">
  <h2 title="colours let the laser software separate processes; they carry no laser settings">Output groups <button class="add" onclick={() => store.addGroup()}>+ group</button></h2>
  <ul>
    {#each store.profile.outputGroups as g (g.id)}
      <li class:selected={store.selection.kind === "group" && store.selection.id === g.id}>
        <span class="swatch" style:background={rgbCss(g.color)}></span>
        <button
          class="name"
          title={`rgb(${g.color.join(", ")}) · PDF stroke ${g.strokeWidthMm ?? store.profile.export.strokeWidthMm} mm`}
          onclick={() => (store.selection = { kind: "group", id: g.id })}>{g.name}</button
        >
        <span class="meta">{usage(g.id)} pass{usage(g.id) === 1 ? "" : "es"}{g.export ? "" : " · not exported"}</span>
        {#if collisions.has(g.id)}<span class="err" title="another exporting group has this exact colour">colour clash</span>{/if}
        <button onclick={() => remove(g.id)} title="delete">×</button>
      </li>
    {/each}
  </ul>
</section>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 12px;
  }
  li {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    padding: 2px;
    border-radius: 4px;
  }
  li.selected {
    background: color-mix(in srgb, #1f6feb 15%, transparent);
  }
  .swatch {
    width: 12px;
    height: 12px;
    border-radius: 2px;
    border: 1px solid color-mix(in srgb, CanvasText 30%, transparent);
  }
  .name {
    background: none;
    border: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
    padding: 0;
  }
  .meta {
    flex: 1;
    opacity: 0.65;
    font-size: 11px;
  }
  .err {
    color: #d33;
  }
  li > button:last-child {
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
  }
  .add {
    font-size: 11px;
  }
</style>
