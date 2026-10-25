<script lang="ts">
  // Progress and Cancel for a running import or evaluation. Appears after a short delay so
  // quick updates do not flicker; reads progress from the worker's shared control block.
  import type { ProjectStore } from "../state/project.svelte.js";

  let { store }: { store: ProjectStore } = $props();

  const DELAY_MS = 300;
  let visible = $state(false);
  let fraction: number | null = $state(null);

  $effect(() => {
    if (!(store.evaluating || store.importing)) {
      visible = false;
      fraction = null;
      return;
    }
    const since = performance.now();
    let frame = 0;
    const tick = () => {
      visible = performance.now() - since >= DELAY_MS;
      const p = store.progress();
      fraction = p && p.running && p.total > 0 ? Math.min(p.done / p.total, 1) : null;
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  });
</script>

{#if visible}
  <span class="busy" role="status">
    {store.importing ? "importing" : "evaluating"}
    <progress max="1" value={fraction ?? undefined}></progress>
    <button onclick={() => store.cancel()} title="stop it; the previous result stays">Cancel</button>
  </span>
{/if}

<style>
  .busy {
    display: inline-flex;
    gap: 0.4rem;
    align-items: center;
  }
  progress {
    width: 6rem;
    height: 0.6rem;
  }
</style>
