<script lang="ts">
  import { rgbCss, type ProjectStore } from "../state/project.svelte.js";
  import { shortId } from "../util/download.js";

  let { store }: { store: ProjectStore } = $props();

  function add() {
    const id = shortId("grp");
    store.update((p) => {
      p.outputGroups.push({ id, name: `Group ${p.outputGroups.length + 1}`, color: [0, 160, 0], strokeWidthMm: null, export: true, visible: true });
    });
    store.selection = { kind: "group", id };
  }

  function remove(i: number) {
    store.update((p) => {
      p.outputGroups.splice(i, 1);
    });
    store.selection = { kind: "none" };
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
  <h2>Output groups <button class="add" onclick={add}>+ group</button></h2>
  <ul>
    {#each store.profile.outputGroups as g, i (g.id)}
      <li class:selected={store.selection.kind === "group" && store.selection.id === g.id}>
        <span class="swatch" style:background={rgbCss(g.color)}></span>
        <button class="name" onclick={() => (store.selection = { kind: "group", id: g.id })}>{g.name}</button>
        <span class="meta">rgb({g.color.join(",")}) · {g.strokeWidthMm ?? store.profile.export.strokeWidthMm} mm · {usage(g.id)} pass(es){g.export ? "" : " · not exported"}</span>
        {#if collisions.has(g.id)}<span class="err" title="another exporting group has this exact colour">colour clash</span>{/if}
        <button onclick={() => remove(i)} title="delete">×</button>
      </li>
    {/each}
  </ul>
  <p class="hint">Colours let the laser software separate processes. They carry no laser settings.</p>
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
  .hint {
    font-size: 11px;
    opacity: 0.7;
  }
</style>
