<script lang="ts">
  // Inside the graph's Svelte Flow: hands out its fit-view function, which only a component
  // within the flow can reach.
  import { useSvelteFlow } from "@xyflow/svelte";

  let { fit = $bindable() }: { fit?: () => void } = $props();
  const flow = useSvelteFlow();
  fit = () => void flow.fitView({ maxZoom: 1, padding: 0.05 });
</script>
