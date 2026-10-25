<script lang="ts">
  // The model as a graph next to the preview (state/graph.ts): imported layers → slots →
  // working layers → passes → output groups, with each node's live status. Clicking a node
  // selects it for the properties panel; the eyes act on the preview as in the layer list.
  // It is also an editor: drawing a connection maps a layer onto a slot, adds an operand,
  // moves a pass or sets its group; Delete removes what can go. Every edit goes through
  // the store (and its undo history); the graph is redrawn from the profile.
  import { Background, Controls, MiniMap, Panel, SvelteFlow, type Connection, type Edge, type Node } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import { setContext } from "svelte";
  import {
    buildGraph,
    connectionEdit,
    edgeRemoval,
    layout,
    nodeRemoval,
    structureKey,
    type GraphEdge,
    type NodeData,
    type Removal,
  } from "../state/graph.js";
  import type { ProjectStore, Selection } from "../state/project.svelte.js";
  import GraphNode from "./GraphNode.svelte";

  let { store }: { store: ProjectStore } = $props();
  // The app has one store for its whole life; nodes reach it through the context.
  // svelte-ignore state_referenced_locally
  setContext("lvp-store", store);

  // Svelte Flow styles its own node types (input, output, default, group): avoid their names.
  const nodeTypes = { file: GraphNode, slot: GraphNode, layer: GraphNode, pass: GraphNode, outputGroup: GraphNode };

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
      type: n.data.kind === "group" ? "outputGroup" : n.data.kind,
      position: positions.get(n.id) ?? { x: 0, y: 0 },
      width: n.width,
      height: n.height,
      data: n.data as unknown as Record<string, unknown>,
      selected: isSelected(n.data),
      deletable: nodeRemoval(store.profile, n.data) !== null,
    }));
    edges = ge.map((e) => {
      const removable = edgeRemoval(e) !== null;
      return {
        id: e.id,
        source: e.source,
        sourceHandle: e.sourceHandle,
        target: e.target,
        targetHandle: e.targetHandle,
        class: `edge-${e.kind}`,
        data: e as unknown as Record<string, unknown>,
        deletable: removable,
        selectable: removable,
      };
    });
  });

  /** Why the last connection drawn was refused, briefly. */
  let refused: string | null = $state(null);
  let refusedTimer: ReturnType<typeof setTimeout> | undefined;

  const editFor = (c: Connection | Edge) => connectionEdit(store.profile, store.sources, c);

  function connect(c: Connection): false {
    const edit = editFor(c);
    if (typeof edit === "string") {
      refused = edit;
      clearTimeout(refusedTimer);
      refusedTimer = setTimeout(() => (refused = null), 3000);
    } else {
      store.applyEdit(edit);
    }
    // The store's edit redraws the graph; Svelte Flow adds no edge of its own.
    return false;
  }

  async function remove({ nodes: ns, edges: es }: { nodes: Node[]; edges: Edge[] }) {
    const removals: Removal[] = [];
    for (const e of es) {
      const r = edgeRemoval(e.data as unknown as GraphEdge);
      if (r) removals.push(r);
    }
    for (const n of ns) {
      const r = nodeRemoval(store.profile, n.data as unknown as NodeData);
      if (r) removals.push(r);
    }
    // Files are removed by index: highest first, so the others keep theirs.
    removals.sort((a, b) => (a.kind === "file" && b.kind === "file" ? b.index - a.index : 0));
    for (const r of removals) store.applyRemoval(r);
    return false;
  }

  const selectedLayer = $derived(store.selection.kind === "layer" ? store.selection.id : null);
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
    deleteKey={["Backspace", "Delete"]}
    proOptions={{ hideAttribution: true }}
    isValidConnection={(c) => typeof editFor(c) !== "string"}
    onbeforeconnect={connect}
    onbeforedelete={remove}
    onnodeclick={({ node }) => {
      const sel = selectionOf(node.data as unknown as NodeData);
      if (sel) store.selection = sel;
    }}
    onnodedragstop={({ nodes: moved }) => {
      for (const n of moved) positions.set(n.id, n.position);
    }}
  >
    <Panel position="top-left">
      <div class="tools">
        <button onclick={() => store.addBinding(`Slot ${store.profile.sourceBindings.length + 1}`, null)}>+ slot</button>
        <button onclick={() => store.addLayer("region")}>+ layer</button>
        <button onclick={() => store.addLayer("envelope")}>+ envelope</button>
        <button onclick={() => selectedLayer && store.addPass(selectedLayer)} disabled={!selectedLayer} title="add a pass to the selected layer">+ pass</button>
        <button onclick={() => store.addGroup()}>+ group</button>
        <span class="sep"></span>
        <button onclick={() => store.undo()} disabled={!store.canUndo} title="undo (Ctrl/⌘ Z)">Undo</button>
        <button onclick={() => store.redo()} disabled={!store.canRedo} title="redo (Ctrl/⌘ ⇧ Z)">Redo</button>
        {#if refused}<span class="refused" role="status">{refused}</span>{/if}
      </div>
    </Panel>
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
  .graph :global(.svelte-flow__edge.selected path) {
    stroke-width: 3;
  }
  .tools {
    display: flex;
    gap: 4px;
    align-items: center;
    padding: 4px;
    background: color-mix(in srgb, Canvas 92%, transparent);
    border-radius: 4px;
  }
  .sep {
    width: 8px;
  }
  .refused {
    color: #d03030;
    margin-left: 6px;
  }
</style>
