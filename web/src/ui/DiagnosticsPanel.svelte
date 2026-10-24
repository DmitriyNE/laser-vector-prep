<script lang="ts">
  import type { ProjectStore } from "../state/project.svelte.js";

  let { store }: { store: ProjectStore } = $props();
  const diags = $derived(store.allDiagnostics);
  const count = (s: string) => diags.filter((d) => d.severity === s).length;
</script>

<section class="panel">
  <h2>Diagnostics <span class="meta">{count("error")} errors · {count("warning")} warnings · {count("info")} notes</span></h2>
  {#if diags.length === 0}
    <p class="hint">Nothing to report.</p>
  {/if}
  <ul>
    {#each diags as d, i (i)}
      <li class={d.severity}>
        <span class="sev">{d.severity}</span>
        <span class="msg">{d.message}</span>
        {#if d.context}<span class="ctx">{d.context}</span>{/if}
        <span class="code">{d.code}</span>
      </li>
    {/each}
  </ul>
</section>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 11px;
  }
  li {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0 0.4rem;
    padding: 2px 0;
    border-bottom: 1px solid color-mix(in srgb, CanvasText 10%, transparent);
  }
  .sev {
    font-weight: 600;
    text-transform: uppercase;
    font-size: 10px;
  }
  .error .sev {
    color: #d33;
  }
  .warning .sev {
    color: #c80;
  }
  .info .sev {
    opacity: 0.6;
  }
  .ctx,
  .code {
    grid-column: 2;
    opacity: 0.6;
    font-family: ui-monospace, monospace;
  }
  .meta,
  .hint {
    font-weight: normal;
    opacity: 0.7;
    font-size: 11px;
  }
</style>
