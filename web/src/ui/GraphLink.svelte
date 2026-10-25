<script lang="ts">
  // An edge of the model graph: a curve, with a × in its middle when it can be removed (an
  // input of a layer, or a mapping made by a rule that names the layer).
  import { BaseEdge, EdgeLabel, getBezierPath, type EdgeProps } from "@xyflow/svelte";
  import { getContext } from "svelte";

  let { id, sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, data, style, interactionWidth }: EdgeProps = $props();
  const remove = getContext<(id: string) => void>("lvp-remove-edge");
  const curve = $derived(getBezierPath({ sourceX, sourceY, sourcePosition, targetX, targetY, targetPosition }));
  const removable = $derived((data as { removable?: string } | undefined)?.removable);
</script>

<BaseEdge {id} path={curve[0]} {style} {interactionWidth} />
{#if removable}
  <EdgeLabel x={curve[1]} y={curve[2]} transparent>
    <button class="x nodrag nopan" title={removable} aria-label={removable} data-edge={id} onclick={() => remove(id)}>×</button>
  </EdgeLabel>
{/if}

<style>
  .x {
    width: 16px;
    height: 16px;
    padding: 0;
    border: 1px solid color-mix(in srgb, CanvasText 35%, transparent);
    border-radius: 50%;
    background: Canvas;
    color: CanvasText;
    font-size: 12px;
    line-height: 13px;
    cursor: pointer;
    opacity: 0.7;
  }
  .x:hover {
    opacity: 1;
    border-color: #d03030;
    color: #d03030;
  }
</style>
