<script lang="ts">
  // The model as a graph next to the preview (state/graph.ts): imported layers → slots →
  // working layers → passes → output groups, with each node's live status. Clicking a node
  // selects it for the properties panel; the eyes act on the preview as in the layer list.
  // It is also an editor: dragging from a node's right handle to another's left handle maps
  // a layer onto a slot, adds an operand, moves a pass or sets its group; the × on an edge
  // and Delete remove what can go, and a click on any other edge says how to change it.
  // Every edit goes through the store (and its undo history); the graph is redrawn from the
  // profile.
  import { Background, Controls, MiniMap, SvelteFlow, type Connection, type Edge, type Node } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import { setContext, untrack } from "svelte";
  import {
    buildGraph,
    connectionEdit,
    edgeHint,
    edgeRemoval,
    layout,
    nodeRemoval,
    structureKey,
    type GraphEdge,
    type NodeData,
    type Removal,
  } from "../state/graph.js";
  import type { ProjectStore, Selection } from "../state/project.svelte.js";
  import { ruleText } from "../state/labels.js";
  import GraphFit from "./GraphFit.svelte";
  import GraphLink from "./GraphLink.svelte";
  import GraphNode from "./GraphNode.svelte";

  let { store }: { store: ProjectStore } = $props();
  // The app has one store for its whole life; nodes reach it through the context.
  // svelte-ignore state_referenced_locally
  setContext("lvp-store", store);

  // Svelte Flow styles its own node types (input, output, default, group): avoid their names.
  const nodeTypes = { file: GraphNode, slot: GraphNode, layer: GraphNode, pass: GraphNode, outputGroup: GraphNode };
  const edgeTypes = { link: GraphLink };

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

  // Redraw from the model. A redraw (after every edit and evaluation) keeps what is selected
  // on the canvas; the store's selection only applies when it changes (below).
  $effect(() => {
    const { nodes: gn, edges: ge } = model;
    const key = structureKey(gn, ge);
    // Lay out again when nodes or edges come or go; status updates keep dragged positions.
    if (key !== laidOut) {
      positions = layout(gn, ge);
      laidOut = key;
    }
    const [wasNode, wasEdge] = untrack(() => [new Map(nodes.map((n) => [n.id, !!n.selected])), new Map(edges.map((e) => [e.id, !!e.selected]))]);
    nodes = gn.map((n) => ({
      id: n.id,
      type: n.data.kind === "group" ? "outputGroup" : n.data.kind,
      position: positions.get(n.id) ?? { x: 0, y: 0 },
      width: n.width,
      height: n.height,
      data: n.data as unknown as Record<string, unknown>,
      selected: wasNode.get(n.id) ?? untrack(() => isSelected(n.data)),
      deletable: nodeRemoval(store.profile, n.data) !== null,
    }));
    edges = ge.map((e) => {
      const removal = edgeRemoval(e, store.profile, store.sources);
      return {
        id: e.id,
        type: "link",
        source: e.source,
        sourceHandle: e.sourceHandle,
        target: e.target,
        targetHandle: e.targetHandle,
        class: `edge-${e.kind}`,
        data: { edge: e, removable: removal && (removal.kind === "unmap" ? "unmap: remove the slot's rules that match this layer" : "remove this input") },
        deletable: removal !== null,
        selectable: removal !== null,
        selected: removal !== null && (wasEdge.get(e.id) ?? false),
      };
    });
  });

  // Something selected elsewhere (the layer list, a panel) is what is selected here.
  $effect(() => {
    void store.selection;
    untrack(() => {
      nodes = nodes.map((n) => ({ ...n, selected: isSelected(n.data as unknown as NodeData) }));
      edges = edges.map((e) => (e.selected ? { ...e, selected: false } : e));
    });
  });

  const graphEdge = (e: Edge) => (e.data as { edge: GraphEdge }).edge;

  /** A short message in the toolbar: why a connection was refused, how to change an edge. */
  let message: { text: string; error: boolean } | null = $state(null);
  let messageTimer: ReturnType<typeof setTimeout> | undefined;
  function say(text: string, error: boolean) {
    message = { text, error };
    clearTimeout(messageTimer);
    messageTimer = setTimeout(() => (message = null), 5000);
  }

  // The × on an edge.
  setContext("lvp-remove-edge", (id: string) => {
    const e = model.edges.find((x) => x.id === id);
    const r = e && edgeRemoval(e, store.profile, store.sources);
    if (!r) return;
    if (r.kind === "unmap") {
      const slot = store.profile.sourceBindings.find((b) => b.id === r.slot);
      const removed = r.rules.map((i) => ruleText(slot!.rules[i]!)).join("; ");
      say(`Removed from slot ${slot?.name}: ${removed} (undo restores them).`, false);
    }
    store.applyRemoval(r);
  });

  const editFor = (c: Connection | Edge) => connectionEdit(store.profile, store.sources, c);

  function connect(c: Connection): false {
    const edit = editFor(c);
    if (typeof edit === "string") say(edit, true);
    else store.applyEdit(edit);
    // The store's edit redraws the graph; Svelte Flow adds no edge of its own.
    return false;
  }

  async function remove({ nodes: ns, edges: es }: { nodes: Node[]; edges: Edge[] }) {
    const removals: Removal[] = [];
    for (const e of es) {
      const r = edgeRemoval(graphEdge(e), store.profile, store.sources);
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

  // Double-click on the background fits the graph, as on the preview.
  let fit: (() => void) | undefined = $state();
  function dblclick(e: MouseEvent) {
    const t = e.target as Element;
    if (!t.closest(".svelte-flow__node, .svelte-flow__edge, .svelte-flow__edge-label, .svelte-flow__panel, .overlay-bar")) fit?.();
  }
</script>

<div class="graph" role="presentation" ondblclick={dblclick}>
  <SvelteFlow
    bind:nodes
    bind:edges
    {nodeTypes}
    {edgeTypes}
    connectionRadius={36}
    fitView
    fitViewOptions={{ maxZoom: 1, padding: 0.05 }}
    zoomOnDoubleClick={false}
    colorMode="system"
    minZoom={0.1}
    deleteKey={["Backspace", "Delete"]}
    proOptions={{ hideAttribution: true }}
    isValidConnection={(c) => typeof editFor(c) !== "string"}
    onbeforeconnect={connect}
    onbeforedelete={remove}
    onedgeclick={({ edge }) => {
      const e = graphEdge(edge);
      if (!edgeRemoval(e, store.profile, store.sources)) say(edgeHint(e), false);
    }}
    onnodeclick={({ node }) => {
      const sel = selectionOf(node.data as unknown as NodeData);
      if (sel) store.selection = sel;
    }}
    onnodedragstop={({ nodes: moved }) => {
      for (const n of moved) positions.set(n.id, n.position);
    }}
  >
    <GraphFit bind:fit />
    <div class="tools overlay-bar nopan nodrag nowheel">
        <button onclick={() => store.addBinding(`Slot ${store.profile.sourceBindings.length + 1}`, null)}>+ slot</button>
        <button onclick={() => store.addLayer("region")}>+ layer</button>
        <button onclick={() => store.addLayer("envelope")}>+ envelope</button>
        <button onclick={() => selectedLayer && store.addPass(selectedLayer)} disabled={!selectedLayer} title="add a pass to the selected layer">+ pass</button>
        <button onclick={() => store.addGroup()}>+ group</button>
        <span class="help" title="Drag from a ● on a node's right to a ● on another's left to connect; × on a connection removes it; Delete removes what is selected; double-click fits the view.">?</span>
        {#if message}<span class="message" class:error={message.error} role="status">{message.text}</span>{/if}
    </div>
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
  /* The same edge as the preview's minimap. */
  .graph :global(.svelte-flow__minimap) {
    box-shadow: 0 0 2px 1px rgba(0, 0, 0, 0.08);
  }
  .graph :global(.svelte-flow__edge.selected path) {
    stroke-width: 3;
  }
  /* Handles big enough to find and grab, with a ring while hovered or connecting. */
  .graph :global(.svelte-flow__handle) {
    width: 11px;
    height: 11px;
    border: 2px solid Canvas;
    background: color-mix(in srgb, CanvasText 55%, transparent);
  }
  .graph :global(.svelte-flow__handle:hover),
  .graph :global(.svelte-flow__handle.connectingfrom),
  .graph :global(.svelte-flow__handle.connectingto.valid) {
    background: Highlight;
    box-shadow: 0 0 0 4px color-mix(in srgb, Highlight 35%, transparent);
  }
  .tools {
    gap: 4px;
  }
  .tools button {
    flex: none;
    white-space: nowrap;
  }
  .help {
    display: inline-flex;
    justify-content: center;
    align-items: center;
    width: 16px;
    height: 16px;
    border: 1px solid color-mix(in srgb, CanvasText 35%, transparent);
    border-radius: 50%;
    font-size: 11px;
    opacity: 0.7;
    cursor: help;
  }
  .message {
    margin-left: 6px;
    max-width: 46ch;
    font-size: 12px;
    opacity: 0.75;
  }
  .message.error {
    color: #d03030;
    opacity: 1;
  }
</style>
