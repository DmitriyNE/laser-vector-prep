<script lang="ts">
  import { onMount } from "svelte";
  import { ProjectStore } from "./state/project.svelte.js";
  import BindingsPanel from "./ui/BindingsPanel.svelte";
  import DiagnosticsPanel from "./ui/DiagnosticsPanel.svelte";
  import ExportPanel from "./ui/ExportPanel.svelte";
  import GroupsPanel from "./ui/GroupsPanel.svelte";
  import LayersPanel from "./ui/LayersPanel.svelte";
  import Preview from "./ui/Preview.svelte";
  import ProfilesBar from "./ui/ProfilesBar.svelte";
  import PropertiesPanel from "./ui/PropertiesPanel.svelte";
  import SourcesPanel from "./ui/SourcesPanel.svelte";

  // Shared-memory wasm cannot start without cross-origin isolation. On static hosting the
  // coi-serviceworker reloads the page once to get there; until then we just wait.
  const isolated = globalThis.crossOriginIsolated === true;
  const store = new ProjectStore();
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
      <Preview {store} />
    </main>
    <aside class="right">
      <PropertiesPanel {store} />
      <GroupsPanel {store} />
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
  }
  .gate {
    padding: 2rem;
    opacity: 0.7;
  }
</style>
