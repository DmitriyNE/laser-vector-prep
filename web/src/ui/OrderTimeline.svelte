<script lang="ts">
  import type { ProjectStore } from "../state/project.svelte.js";
  import { RAMP_CSS, Timing, type Sequence } from "../state/sequence.js";

  let { store, seq, timing, group }: { store: ProjectStore; seq: Sequence | null; timing: Timing | null; group: string | null } = $props();

  const count = $derived(seq?.count ?? 0);
  /** Trajectory mode: the moment shown and where the beam is then. */
  const time = $derived(timing ? Math.min(store.orderTime, timing.total) : 0);
  const beam = $derived(timing?.at(time) ?? null);
  const index = $derived(beam ? beam.index : Math.min(store.orderIndex, Math.max(count - 1, 0)));
  const groups = $derived((store.evaluation?.groups ?? []).filter((g) => g.paths > 0));
  const passNames = $derived(new Map((store.evaluation?.passes ?? []).map((p) => [p.id, p.name])));
  const SPEEDS = [10, 50, 250, 1000, 5000];
  let speed = $state(250);
  /** Trajectory playback: time at the chosen speeds times this. */
  const RATES = [0.01, 0.1, 1, 10, 100];
  let rate = $state(1);
  let playing = $state(false);

  const fmt = (n: number) => n.toLocaleString("en-US");
  const mm = (v: number) => (v < 10 ? v.toFixed(3) : v < 1000 ? v.toFixed(1) : v.toFixed(0));
  const secs = (t: number) => (t < 60 ? `${t.toFixed(t < 10 ? 3 : 2)} s` : `${Math.floor(t / 60)}:${(t % 60).toFixed(1).padStart(4, "0")}`);

  /** Make path `i` the current one; following the beam, from where it starts marking it. */
  function seek(i: number) {
    if (!count) return;
    const k = Math.min(Math.max(0, i), count - 1);
    store.orderIndex = k;
    if (timing) store.orderTime = timing.markStart(k);
  }

  function seekTime(t: number) {
    if (timing) store.orderTime = Math.min(Math.max(0, t), timing.total);
  }

  const atEnd = () => (timing ? time >= timing.total : index >= count - 1);

  function togglePlay() {
    if (!playing && atEnd()) {
      if (timing) seekTime(0);
      else seek(0);
    }
    playing = !playing;
  }

  /** Switch between stepping paths and following the beam, keeping the place. */
  function setTrajectory(on: boolean) {
    playing = false;
    if (on && seq) {
      const t = new Timing(seq, store.markSpeed, store.jumpSpeed);
      store.orderTime = store.orderIndex >= count ? Number.MAX_VALUE : t.markStart(index);
    } else store.orderIndex = time >= (timing?.total ?? 0) ? Number.MAX_SAFE_INTEGER : index;
    store.orderMode = on ? "trajectory" : "paths";
  }

  /** A new speed keeps the beam on its path, from where it starts marking it. */
  function setSpeed(which: "markSpeed" | "jumpSpeed", value: number) {
    const keep = timing && time < timing.total ? index : null;
    store.setSpeed(which, value);
    if (keep !== null && seq) store.orderTime = new Timing(seq, store.markSpeed, store.jumpSpeed).markStart(keep);
  }

  // Playback: `speed` paths a second, or following the beam `rate` times the speeds' time;
  // stopping at the end (or when the sequence goes away).
  $effect(() => {
    if (!playing) return;
    let last = performance.now();
    let due = 0;
    let frame = requestAnimationFrame(function tick(now) {
      const dt = (now - last) / 1000;
      last = now;
      if (timing) {
        seekTime(time + dt * rate);
        if (atEnd()) {
          playing = false;
          return;
        }
      } else {
        due += dt * speed;
        const step = Math.floor(due);
        if (step > 0) {
          due -= step;
          seek(index + step);
          if (index >= count - 1) {
            playing = false;
            return;
          }
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
    } else if (timing) {
      // The time slider steps path by path, as the path slider does.
      const to = { ArrowLeft: () => seek(index - 1), ArrowDown: () => seek(index - 1), ArrowRight: () => seek(index + 1), ArrowUp: () => seek(index + 1), Home: () => seekTime(0), End: () => seekTime(timing.total) }[e.key];
      if (to) {
        e.preventDefault();
        to();
      }
    }
  }

  const readout = $derived.by(() => {
    if (!seq || !count) return null;
    const i = index;
    if (timing && beam) {
      const doing = beam.phase === "jump" ? `jumping ${mm(beam.s)} of ${mm(seq.jumps[i]!)} mm` : beam.phase === "end" ? `jumping to the end point, ${mm(beam.s)} mm` : `marking ${mm(beam.s)} of ${mm(seq.lengths[i]!)} mm`;
      return {
        text: `${passNames.get(seq.pass(i)) ?? seq.pass(i)} · ${doing} · ${secs(time)} / ${secs(timing.total)}`,
        title: `at ${fmt(store.markSpeed)} mm/s marking and ${fmt(store.jumpSpeed)} mm/s jumping: ${secs(timing.marking)} marking, ${secs(timing.total - timing.marking)} jumping\nacceleration, galvo settling and laser delays are not modelled`,
      };
    }
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
      {#if timing}
        <input
          type="range"
          min="0"
          max={timing.total}
          step="any"
          value={time}
          aria-label="time"
          title="time: the beam marks the paths and jumps between them at the speeds set below (arrow keys step path by path, space plays)"
          oninput={(e) => seekTime(Number((e.currentTarget as HTMLInputElement).value))}
          onkeydown={onKey}
          disabled={!count}
        />
      {:else}
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
      {/if}
      {#if store.orderGradient}<div class="ramp" style:background={RAMP_CSS}></div>{/if}
    </div>
    <button title="next path" aria-label="next path" onclick={() => seek(index + 1)} disabled={index >= count - 1}>›</button>
    <span class="count" title="current path / paths in the group">{count ? fmt(index + 1) : 0} / {fmt(count)}</span>
  </div>
  <div class="row meta">
    <span class="readout" title={readout?.title}>{readout?.text ?? "no paths"}</span>
    <label title="follow the beam along the paths and the jumps between them in time, rather than stepping whole paths"
      ><input type="checkbox" checked={store.orderMode === "trajectory"} onchange={(e) => setTrajectory((e.currentTarget as HTMLInputElement).checked)} /> trajectory</label
    >
    <label title="colour marked paths by their place in the order, early (blue) to late (red); otherwise in the group's colour"
      ><input type="checkbox" bind:checked={store.orderGradient} /> gradient</label
    >
    {#if timing}
      <label class="speed-field" title="mark speed for the preview only: kept in this browser, not in the profile">mark
        <input type="number" min="1" step="100" value={store.markSpeed} onchange={(e) => setSpeed("markSpeed", Number((e.currentTarget as HTMLInputElement).value))} /></label
      >
      <label class="speed-field" title="jump speed for the preview only: kept in this browser, not in the profile">jump
        <input type="number" min="1" step="100" value={store.jumpSpeed} onchange={(e) => setSpeed("jumpSpeed", Number((e.currentTarget as HTMLInputElement).value))} /> mm/s</label
      >
      <select class="speed" bind:value={rate} title="playback rate: 1× runs in the time the speeds give">
        {#each RATES as r (r)}<option value={r}>{r}×</option>{/each}
      </select>
    {:else}
      <select class="speed" bind:value={speed} title="playback speed">
        {#each SPEEDS as s (s)}<option value={s}>{fmt(s)} paths/s</option>{/each}
      </select>
    {/if}
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
  .speed-field input {
    width: 4.5em;
    font: inherit;
    font-size: 11px;
  }
</style>
