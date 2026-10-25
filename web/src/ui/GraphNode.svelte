<script lang="ts">
  // One node of the model graph (see state/graph.ts): a file with a handle per imported
  // layer, a slot, a working layer with its operand groups A and B, a pass, or an output group.
  import { Handle, Position } from "@xyflow/svelte";
  import { getContext } from "svelte";
  import { rgbCss, type ProjectStore } from "../state/project.svelte.js";
  import { HEADER_HEIGHT, ROW_HEIGHT, rowHandle, type NodeData } from "../state/graph.js";
  import Eye from "./Eye.svelte";

  let { data }: { data: NodeData } = $props();
  const store = getContext<ProjectStore>("lvp-store");

  const rowTop = (i: number) => `top: ${HEADER_HEIGHT + ROW_HEIGHT * (i + 0.5)}px`;
</script>

{#if data.kind === "file"}
  <div class="node file">
    <div class="head" title={data.name}><span class="tag">{data.format}</span> {data.name}</div>
    {#each data.rows as row, i (row.name)}
      <button
        class="row {row.state}"
        title={row.state === "bound" ? `matched to a slot` : row.state === "ambiguous" ? "one of several layers a slot matches" : "not used by any slot"}
        onclick={() => (store.selection = { kind: "source", file: data.index, layer: row.name })}>{row.name}</button>
      <Handle type="source" position={Position.Right} id={rowHandle(row.name)} style={rowTop(i)} />
    {/each}
  </div>
{:else if data.kind === "slot"}
  <div class="node slot {data.state}">
    <Handle type="target" position={Position.Left} id="in" />
    <div class="head" title={data.name}><span class="tag">slot</span> {data.name}{data.required ? " *" : ""}</div>
    <div class="sub" title={data.detail}>{data.state} · {data.detail}</div>
    <Handle type="source" position={Position.Right} id="out" />
  </div>
{:else if data.kind === "layer"}
  <div class="node layer {data.status}" class:off={!data.enabled}>
    <Handle type="target" position={Position.Left} id="a" style={data.twoGroups ? "top: 30%" : ""} />
    {#if data.twoGroups}
      <span class="port a">A</span>
      <Handle type="target" position={Position.Left} id="b" style="top: 74%" />
      <span class="port b">B</span>
    {/if}
    <div class="head" title={data.name}>
      <span class="tag">{data.geometry}</span>
      <span class="name">{data.name}</span>
      <span class="nodrag"><Eye hidden={store.hiddenLayers.has(data.id)} what="layer and its passes" onToggle={() => store.toggleLayer(data.id)} onSolo={() => store.solo({ kind: "layer", key: data.id })} /></span>
    </div>
    <div class="sub">{data.op}{data.referenceOnly ? " · reference" : ""}{data.enabled ? "" : " · disabled"}</div>
    <div class="sub">{data.stats}</div>
    <Handle type="source" position={Position.Right} id="out" />
  </div>
{:else if data.kind === "pass"}
  <div class="node pass {data.status}" class:off={!data.enabled}>
    <Handle type="target" position={Position.Left} id="in" />
    <div class="head" title={data.name}>
      <span class="tag">pass</span>
      <span class="name">{data.name}</span>
      <span class="nodrag"><Eye hidden={store.hiddenPasses.has(data.id)} what="paths" onToggle={() => store.togglePass(data.id)} onSolo={() => store.solo({ kind: "pass", key: data.id })} /></span>
    </div>
    <div class="sub">{data.pattern} · {data.stats}</div>
    <Handle type="source" position={Position.Right} id="out" />
  </div>
{:else}
  <div class="node group">
    <Handle type="target" position={Position.Left} id="in" />
    <div class="head" title={data.name}><span class="swatch" style="background: {rgbCss(data.color)}"></span> {data.name}</div>
    <div class="sub">{data.stats}</div>
  </div>
{/if}

<style>
  .node {
    box-sizing: border-box;
    height: 100%;
    padding: 4px 8px;
    border: 1px solid color-mix(in srgb, CanvasText 30%, transparent);
    border-left-width: 4px;
    border-radius: 4px;
    background: Canvas;
    color: CanvasText;
    font-size: 12px;
    overflow: hidden;
  }
  :global(.svelte-flow__node.selected) .node {
    outline: 2px solid Highlight;
  }
  .ok,
  .matched {
    border-left-color: #2a9d4b;
  }
  .warning,
  .ambiguous {
    border-left-color: #d08a00;
  }
  .error,
  .missing {
    border-left-color: #d03030;
  }
  .idle {
    border-left-color: color-mix(in srgb, CanvasText 25%, transparent);
  }
  .off {
    opacity: 0.55;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 4px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sub {
    opacity: 0.75;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tag {
    font-size: 10px;
    font-weight: 400;
    text-transform: uppercase;
    opacity: 0.6;
  }
  .file .head {
    height: 22px;
  }
  .row {
    display: block;
    width: 100%;
    height: 22px;
    text-align: left;
    border: 0;
    background: none;
    color: inherit;
    padding: 0 4px;
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row.unbound {
    opacity: 0.55;
  }
  .row.ambiguous {
    color: #d08a00;
  }
  .port {
    position: absolute;
    left: 6px;
    font-size: 10px;
    opacity: 0.6;
  }
  .port.a {
    top: calc(30% - 6px);
  }
  .port.b {
    top: calc(74% - 6px);
  }
  .layer .head,
  .layer .sub {
    padding-left: 10px;
  }
  .swatch {
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 2px;
  }
</style>
