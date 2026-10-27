<script lang="ts">
  import type { ProjectStore } from "../state/project.svelte.js";
  import { RAMP_CSS, type Sequence } from "../state/sequence.js";

  let { store, seq, group }: { store: ProjectStore; seq: Sequence | null; group: string | null } = $props();

  const count = $derived(seq?.count ?? 0);
  const index = $derived(Math.min(store.orderIndex, Math.max(count - 1, 0)));
  const groups = $derived((store.evaluation?.groups ?? []).filter((g) => g.paths > 0));
  const passNames = $derived(new Map((store.evaluation?.passes ?? []).map((p) => [p.id, p.name])));
  const SPEEDS = [10, 50, 250, 1000, 5000];
  let speed = $state(250);
  let playing = $state(false);

  const fmt = (n: number) => n.toLocaleString("en-US");
  const mm = (v: number) => (v < 10 ? v.toFixed(3) : v < 1000 ? v.toFixed(1) : v.toFixed(0));

  function seek(i: number) {
    if (count) store.orderIndex = Math.min(Math.max(0, i), count - 1);
  }

  function togglePlay() {
    if (!playing && index >= count - 1) seek(0);
    playing = !playing;
  }

  // Playback: `speed` paths a second, stopping at the end (or when the sequence goes away).
  $effect(() => {
    if (!playing) return;
    let last = performance.now();
    let due = 0;
    let frame = requestAnimationFrame(function tick(now) {
      due += ((now - last) / 1000) * speed;
      last = now;
      const step = Math.floor(due);
      if (step > 0) {
        due -= step;
        seek(index + step);
        if (index >= count - 1) {
          playing = false;
          return;
        }
      }
      frame = requestAnimationFrame(tick);
    });
    return () => cancelAnimationFrame(frame);
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === " ") {
      e.preventDefault();
      togglePlay();
    }
  }

  const readout = $derived.by(() => {
    if (!seq || !count) return null;
    const i = index;
    return {
      text: `${passNames.get(seq.pass(i)) ?? seq.pass(i)} · ${seq.closed(i) ? "loop" : "open"} ${mm(seq.lengths[i]!)} mm · jump ${mm(seq.jumps[i]!)} mm`,
      title: `marked up to here: ${mm(seq.markedTo[i]!)} mm in ${fmt(i + 1)} paths, ${mm(seq.jumpedTo[i]!)} mm of jumps\nwhole group: ${mm(seq.markedTo[count - 1]!)} mm marked, ${mm(seq.jumpedTo[count - 1]!)} mm of jumps`,
    };
  });
</script>

<div class="timeline" role="group" aria-label="marking order">
  <div class="row">
    <button class="play" title={playing ? "pause (space)" : "play the order (space)"} aria-label={playing ? "pause" : "play"} onclick={togglePlay} disabled={!count}>
      {playing ? "❚❚" : "▶"}
    </button>
    <select
      class="group"
      value={group}
      title="output group to step through: each group is ordered on its own, and the laser software may run groups in any order"
      onchange={(e) => store.inspectOrder((e.currentTarget as HTMLSelectElement).value)}
    >
      {#each groups as g (g.id)}<option value={g.id}>{g.name}</option>{/each}
    </select>
    <button title="previous path" aria-label="previous path" onclick={() => seek(index - 1)} disabled={index <= 0}>‹</button>
    <div class="track">
      <input
        type="range"
        min="0"
        max={Math.max(count - 1, 0)}
        value={index}
        aria-label="current path"
        title="current path: the ones before it are marked (arrow keys step, space plays)"
        oninput={(e) => seek(Number((e.currentTarget as HTMLInputElement).value))}
        onkeydown={onKey}
        disabled={!count}
      />
      {#if store.orderGradient}<div class="ramp" style:background={RAMP_CSS}></div>{/if}
    </div>
    <button title="next path" aria-label="next path" onclick={() => seek(index + 1)} disabled={index >= count - 1}>›</button>
    <span class="count" title="current path / paths in the group">{count ? fmt(index + 1) : 0} / {fmt(count)}</span>
  </div>
  <div class="row meta">
    <span class="readout" title={readout?.title}>{readout?.text ?? "no paths"}</span>
    <label title="colour marked paths by their place in the order, early (blue) to late (red); otherwise in the group's colour"
      ><input type="checkbox" bind:checked={store.orderGradient} /> gradient</label
    >
    <select class="speed" bind:value={speed} title="playback speed">
      {#each SPEEDS as s (s)}<option value={s}>{fmt(s)} paths/s</option>{/each}
    </select>
  </div>
</div>

<style>
  .timeline {
    position: absolute;
    left: 56px;
    right: 230px;
    bottom: 15px;
    z-index: 5;
    padding: 4px 8px;
    font-size: 12px;
    background: color-mix(in srgb, Canvas 88%, transparent);
    border-radius: 4px;
    box-shadow: 0 0 2px 1px rgba(0, 0, 0, 0.08);
  }
  .row {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .meta {
    margin-top: 2px;
    font-size: 11px;
  }
  .track {
    flex: 1;
    min-width: 4rem;
    display: flex;
    flex-direction: column;
  }
  .track input {
    width: 100%;
    margin: 0;
  }
  .ramp {
    height: 3px;
    margin: 0 7px;
    border-radius: 2px;
  }
  .count {
    font-family: ui-monospace, monospace;
    white-space: nowrap;
    min-width: 7.5em;
    text-align: right;
  }
  .readout {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    opacity: 0.85;
  }
  .play {
    width: 2.2em;
  }
  .group {
    max-width: 9rem;
  }
  select,
  button {
    font: inherit;
    font-size: 11px;
  }
  label {
    white-space: nowrap;
  }
</style>
