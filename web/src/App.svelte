<script lang="ts">
  import { onMount } from "svelte";
  import { ProjectStore } from "./state/project.svelte.js";
  import BindingsPanel from "./ui/BindingsPanel.svelte";
  import DiagnosticsPanel from "./ui/DiagnosticsPanel.svelte";
  import ExportPanel from "./ui/ExportPanel.svelte";
  import GroupsPanel from "./ui/GroupsPanel.svelte";
  import LayersPanel from "./ui/LayersPanel.svelte";
  import OrderPanel from "./ui/OrderPanel.svelte";
  import Preview from "./ui/Preview.svelte";
  import ProfilesBar from "./ui/ProfilesBar.svelte";
  import PropertiesPanel from "./ui/PropertiesPanel.svelte";
  import SourcesPanel from "./ui/SourcesPanel.svelte";

  // Shared-memory wasm cannot start without cross-origin isolation. On static hosting the
  // coi-serviceworker reloads the page once to get there; until then we just wait.
  const isolated = globalThis.crossOriginIsolated === true;
  const store = new ProjectStore();
  /** The main area's tab. The graph mounts on first use and then stays, like the preview. */
  let tab: "preview" | "graph" = $state("preview");
  let graphMounted = $state(false);
  $effect(() => {
    if (tab === "graph") graphMounted = true;
  });
  // Debug and end-to-end test hook: the store is reachable from the console.
  (globalThis as { lvp?: ProjectStore }).lvp = store;

  onMount(() => {
    if (isolated) void store.init();
    return () => store.worker.dispose();
  });
</script>

{#if !isolated}
  <div class="gate">Waiting for cross-origin isolation (the page reloads once on first visit)…</div>
{:else}
  <div class="app">
    <ProfilesBar {store} />
    <aside class="left">
      <SourcesPanel {store} />
      <BindingsPanel {store} />
      <LayersPanel {store} />
    </aside>
    <main>
      <div class="tabs" role="tablist">
        <button role="tab" aria-selected={tab === "preview"} onclick={() => (tab = "preview")}>Preview</button>
        <button role="tab" aria-selected={tab === "graph"} onclick={() => (tab = "graph")}>Graph</button>
      </div>
      <div class="view" class:hidden={tab !== "preview"}><Preview {store} /></div>
      {#if graphMounted}
        <!-- Loaded on first use: the graph library is most of its size. -->
        <div class="view" class:hidden={tab !== "graph"}>
          {#await import("./ui/GraphView.svelte") then { default: GraphView }}<GraphView {store} />{/await}
        </div>
      {/if}
    </main>
    <aside class="right">
      <PropertiesPanel {store} />
      <GroupsPanel {store} />
      <OrderPanel {store} />
      <ExportPanel {store} />
      <DiagnosticsPanel {store} />
    </aside>
  </div>
{/if}

<style>
  .app {
    display: grid;
    grid-template-columns: 300px 1fr 320px;
    grid-template-rows: auto 1fr;
    grid-template-areas:
      "bar bar bar"
      "left main right";
    height: 100%;
  }
  .app > :global(.bar) {
    grid-area: bar;
  }
  .left {
    grid-area: left;
    overflow: auto;
    border-right: 1px solid color-mix(in srgb, CanvasText 15%, transparent);
  }
  .right {
    grid-area: right;
    overflow: auto;
    border-left: 1px solid color-mix(in srgb, CanvasText 15%, transparent);
  }
  main {
    grid-area: main;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .tabs {
    display: flex;
    gap: 2px;
    padding: 2px 4px 0;
    border-bottom: 1px solid color-mix(in srgb, CanvasText 15%, transparent);
  }
  .tabs button {
    border: 1px solid transparent;
    border-bottom: 0;
    background: none;
    color: inherit;
    padding: 3px 10px;
    border-radius: 4px 4px 0 0;
    cursor: pointer;
    opacity: 0.7;
  }
  .tabs button[aria-selected="true"] {
    border-color: color-mix(in srgb, CanvasText 15%, transparent);
    background: Canvas;
    opacity: 1;
  }
  .view {
    flex: 1;
    min-height: 0;
  }
  .view.hidden {
    display: none;
  }
  .gate {
    padding: 2rem;
    opacity: 0.7;
  }
</style>
