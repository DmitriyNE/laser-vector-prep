<script lang="ts">
  // Preview visibility toggle shared by the sources and layers panels. Click toggles,
  // alt-click shows only this item (and alt-click again on the only visible item restores all).
  let {
    hidden,
    what = "",
    onToggle,
    onSolo,
  }: { hidden: boolean; what?: string; onToggle: () => void; onSolo?: () => void } = $props();

  function click(e: MouseEvent) {
    if (e.altKey && onSolo) onSolo();
    else onToggle();
  }
</script>

<button
  class="eye"
  class:off={hidden}
  aria-pressed={!hidden}
  title={`${hidden ? "show" : "hide"} ${what} in preview${onSolo ? " · alt-click: show only this" : ""}`}
  onclick={click}
>
  <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
    <path d="M1 8s2.5-4.5 7-4.5S15 8 15 8s-2.5 4.5-7 4.5S1 8 1 8z" fill="none" stroke="currentColor" stroke-width="1.3" />
    <circle cx="8" cy="8" r="2.2" fill="currentColor" />
    {#if hidden}<path d="M2.5 13.5l11-11" stroke="currentColor" stroke-width="1.3" />{/if}
  </svg>
</button>

<style>
  .eye {
    background: none;
    border: none;
    padding: 2px;
    color: inherit;
    cursor: pointer;
    display: inline-flex;
    opacity: 0.8;
  }
  .eye.off {
    opacity: 0.35;
  }
</style>
