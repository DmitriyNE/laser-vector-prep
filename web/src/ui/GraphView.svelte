<script lang="ts">
  // The model as a graph next to the preview (state/graph.ts): imported layers → slots →
  // working layers → passes → output groups, with each node's live status. Clicking a node
  // selects it for the properties panel; the eyes act on the preview as in the layer list.
  import { Background, Controls, MiniMap, SvelteFlow, type Edge, type Node } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import { setContext } from "svelte";
  import { buildGraph, layout, structureKey, type NodeData } from "../state/graph.js";
  import type { ProjectStore, Selection } from "../state/project.svelte.js";
  import GraphNode from "./GraphNode.svelte";

  let { store }: { store: ProjectStore } = $props();
  // The app has one store for its whole life; nodes reach it through the context.
  // svelte-ignore state_referenced_locally
  setContext("lvp-store", store);

  // "group" is Svelte Flow's own node type (with its own styling): output groups are "output".
  const nodeTypes = { file: GraphNode, slot: GraphNode, layer: GraphNode, pass: GraphNode, output: GraphNode };

  const model = $derived(buildGraph(store.profile, store.sources, store.evaluation));

  /** Node positions: from the layout, or where the user dragged them. */
  let positions = new Map<string, { x: number; y: number }>();
  let laidOut = "";
  let nodes: Node[] = $state.raw([]);
  let edges: Edge[] = $state.raw([]);

  function selectionOf(d: NodeData): Selection | null {
    switch (d.kind) {
      case "slot":
        return { kind: "binding", id: d.id };
      case "layer":
      case "pass":
      case "group":
        return { kind: d.kind, id: d.id };
      default:
        return null;
    }
  }

  function isSelected(d: NodeData): boolean {
    const s = store.selection as { kind: string; id?: string };
    const mine = selectionOf(d) as { kind: string; id?: string } | null;
    return mine !== null && s.kind === mine.kind && s.id === mine.id;
  }

  $effect(() => {
    const { nodes: gn, edges: ge } = model;
    const key = structureKey(gn, ge);
    // Lay out again when nodes or edges come or go; status updates keep dragged positions.
    if (key !== laidOut) {
      positions = layout(gn, ge);
      laidOut = key;
    }
    nodes = gn.map((n) => ({
      id: n.id,
      type: n.data.kind === "group" ? "output" : n.data.kind,
      position: positions.get(n.id) ?? { x: 0, y: 0 },
      width: n.width,
      height: n.height,
      data: n.data as unknown as Record<string, unknown>,
      selected: isSelected(n.data),
      connectable: false,
      deletable: false,
    }));
    edges = ge.map((e) => ({
      id: e.id,
      source: e.source,
      sourceHandle: e.sourceHandle,
      target: e.target,
      targetHandle: e.targetHandle,
      class: `edge-${e.kind}`,
      deletable: false,
      selectable: false,
    }));
  });
</script>

<div class="graph">
  <SvelteFlow
    bind:nodes
    bind:edges
    {nodeTypes}
    fitView
    fitViewOptions={{ maxZoom: 1, padding: 0.05 }}
    colorMode="system"
    minZoom={0.1}
    deleteKey={null}
    nodesConnectable={false}
    proOptions={{ hideAttribution: true }}
    onnodeclick={({ node }) => {
      const sel = selectionOf(node.data as unknown as NodeData);
      if (sel) store.selection = sel;
    }}
    onnodedragstop={({ nodes: moved }) => {
      for (const n of moved) positions.set(n.id, n.position);
    }}
  >
    <Background />
    <Controls showLock={false} />
    <MiniMap pannable zoomable />
  </SvelteFlow>
</div>

<style>
  .graph {
    width: 100%;
    height: 100%;
  }
  .graph :global(.edge-candidate path) {
    stroke-dasharray: 4 3;
    stroke: #d08a00;
  }
  .graph :global(.edge-map path) {
    stroke: #2a9d4b;
  }
  .graph :global(.edge-b path) {
    stroke: #8a5cd0;
  }
</style>
