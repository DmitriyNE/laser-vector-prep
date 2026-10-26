<script lang="ts">
  import type { Operand, StrokeWidth, OptimizerMode, JoinStyle, RingOrder } from "../core/types.js";
  import { hexRgb, rgbHex, type ProjectStore } from "../state/project.svelte.js";

  let { store }: { store: ProjectStore } = $props();

  const sel = $derived(store.selection);
  const layer = $derived(sel.kind === "layer" ? store.layer(sel.id) : undefined);
  const passInfo = $derived(sel.kind === "pass" ? store.pass(sel.id) : undefined);
  const group = $derived(sel.kind === "group" ? store.group(sel.id) : undefined);
  const binding = $derived(sel.kind === "binding" ? store.profile.sourceBindings.find((b) => b.id === sel.id) : undefined);
  const sourceSel = $derived(sel.kind === "source" ? sel : undefined);
  const sourceFile = $derived(sourceSel ? store.sources.find((s) => s.index === sourceSel.file) : undefined);
  const sourceLayer = $derived(sourceFile && sourceSel ? sourceFile.layers.find((l) => l.name === sourceSel.layer) : undefined);

  const touch = () => store.update(() => {});
  const CENTRELINE = "centreline distance, independent of the PDF stroke width";

  function hasInput(ops: Operand[], o: Operand) {
    return ops.some((x) => JSON.stringify(x) === JSON.stringify(o));
  }

  function toggleInput(ops: Operand[], o: Operand) {
    store.update(() => {
      const i = ops.findIndex((x) => JSON.stringify(x) === JSON.stringify(o));
      if (i >= 0) ops.splice(i, 1);
      else ops.push(o);
    });
  }

  function strokeMode(sw: StrokeWidth): "none" | "original" | "fixed" {
    return typeof sw === "string" ? sw : "fixed";
  }

  function setStrokeMode(mode: string) {
    if (!layer) return;
    store.update(() => {
      layer.interpretation.strokeWidth = mode === "fixed" ? { fixed: { mm: 0.1 } } : (mode as "none" | "original");
    });
  }

  function setPatternKind(kind: string) {
    if (!passInfo) return;
    store.update(() => {
      switch (kind) {
        case "contourFill":
          passInfo.pass.pattern = { kind: "contourFill", pitchMm: 0.02, firstOffsetMm: null, gapFill: true };
          break;
        case "boundaryTrace":
          passInfo.pass.pattern = { kind: "boundaryTrace", side: "on", offsetMm: 0, kerfMm: 0, rings: 1, ringSpacingMm: 0.05, reverse: false, innerFirst: true };
          break;
        case "centreline":
          passInfo.pass.pattern = { kind: "centreline" };
          break;
        case "hatch":
          passInfo.pass.pattern = { kind: "hatch", pitchMm: 0.02, angleDeg: 0, phaseMm: 0, serpentine: true, insetMm: 0 };
          break;
        case "crossHatch":
          passInfo.pass.pattern = { kind: "crossHatch", pitchMm: 0.02, anglesDeg: [0, 90], phaseMm: 0, serpentine: true, insetMm: 0 };
          break;
        case "boundaryHatch":
          passInfo.pass.pattern = { kind: "boundaryHatch", shells: 1, shellPitchMm: 0.02, insetMm: 0, pitchMm: 0.02, anglesDeg: [0], phaseMm: 0, serpentine: true };
          break;
        default:
          passInfo.pass.pattern = { kind: "passthrough", reverse: false };
      }
    });
  }

  function setKind(kind: string) {
    if (!layer) return;
    store.update(() => {
      layer.kind = kind as "region" | "linework" | "envelope";
      if (kind === "envelope") {
        layer.envelope ??= { provider: "boundingBox", marginMm: 0.5 };
        layer.referenceOnly = true;
        layer.op = "union";
        layer.operands = [];
      } else {
        layer.envelope = null;
      }
    });
  }

  function setProvider(provider: string) {
    if (!layer) return;
    store.update(() => {
      switch (provider) {
        case "namedBoundary":
          layer.envelope = { provider: "namedBoundary" };
          break;
        case "boundingCircle":
          layer.envelope = { provider: "boundingCircle", marginMm: 0.5 };
          break;
        case "buffer":
          layer.envelope = { provider: "buffer", distanceMm: 0.5 };
          break;
        case "convexHull":
          layer.envelope = { provider: "convexHull", distanceMm: 0.5 };
          break;
        default:
          layer.envelope = { provider: "boundingBox", marginMm: 0.5 };
      }
    });
  }

  /** Comma or space separated angle list; invalid entries are dropped. */
  function parseAngles(text: string): number[] {
    const out = text
      .split(/[,\s]+/)
      .map((t) => Number(t))
      .filter((n) => Number.isFinite(n));
    return out.length ? out : [0];
  }

  function addRule() {
    if (!binding) return;
    store.update(() => {
      binding.rules.push({ kind: "layerName", format: "dxf", name: "" });
    });
  }

  function setRuleKind(i: number, kind: string) {
    if (!binding) return;
    store.update(() => {
      const r = binding.rules[i];
      if (!r) return;
      const text = "name" in r ? r.name : r.pattern;
      binding.rules[i] =
        kind === "fileLayer"
          ? { kind: "fileLayer", file: "", name: text }
          : kind === "fileName"
            ? { kind: "fileName", format: "gerber", pattern: text }
            : kind === "function"
              ? { kind: "function", pattern: text }
              : { kind: "layerName", format: "dxf", name: text };
    });
  }

  function removeRule(i: number) {
    if (!binding) return;
    store.update(() => {
      binding.rules.splice(i, 1);
    });
  }

  function deleteBinding() {
    if (binding && !store.removeBinding(binding.id)) alert("This slot is still used as an input by a working layer; remove it there first.");
  }
</script>

{#snippet joinEditor(get: () => JoinStyle | undefined, set: (j: JoinStyle) => void)}
  {@const j = get() ?? { kind: "round" }}
  <label>Corner joins
    <select value={j.kind} onchange={(e) => { const k = (e.currentTarget as HTMLSelectElement).value; store.update(() => set(k === "miter" ? { kind: "miter", limit: 4 } : k === "bevel" ? { kind: "bevel" } : { kind: "round" })); }}>
      <option value="round">round (exact arcs)</option>
      <option value="miter">miter (sharp corners)</option>
      <option value="bevel">bevel (cut corners)</option>
    </select>
  </label>
  {#if j.kind === "miter"}
    <label>Miter limit (× distance; sharper corners are bevelled)
      <input type="number" step="0.5" min="1" value={j.limit} oninput={(e) => { const v = Number((e.currentTarget as HTMLInputElement).value); if (v >= 1) store.update(() => set({ kind: "miter", limit: v })); }} />
    </label>
  {/if}
{/snippet}

{#snippet operandBoxes(ops: Operand[])}
  {#each store.profile.sourceBindings as b (b.id)}
    <label class="check">
      <input type="checkbox" checked={hasInput(ops, { source: b.id })} onchange={() => toggleInput(ops, { source: b.id })} />
      source: {b.name}
    </label>
  {/each}
  {#each store.profile.workingLayers.filter((l) => !layer || l.id !== layer.id) as l (l.id)}
    <label class="check">
      <input type="checkbox" checked={hasInput(ops, { layer: l.id })} onchange={() => toggleInput(ops, { layer: l.id })} />
      layer: {l.name}
    </label>
  {/each}
{/snippet}

<section class="panel">
  <h2>Properties</h2>
  {#if layer}
    <label>Name <input bind:value={layer.name} oninput={touch} /></label>
    <label>Geometry
      <select value={layer.kind} onchange={(e) => setKind((e.currentTarget as HTMLSelectElement).value)}>
        <option value="region">region (filled area)</option>
        <option value="linework">linework (centrelines)</option>
        <option value="envelope">envelope (bounded domain around a reference set)</option>
      </select>
    </label>
    {#if layer.kind === "envelope"}
      {@const env = layer.envelope ?? { provider: "boundingBox", marginMm: 0.5 }}
      <label>Domain provider
        <select value={env.provider} onchange={(e) => setProvider((e.currentTarget as HTMLSelectElement).value)}>
          <option value="namedBoundary">named boundary (filled interior of the reference)</option>
          <option value="boundingBox">bounding box + margin</option>
          <option value="boundingCircle">bounding circle + margin</option>
          <option value="buffer">buffer (dilate the reference)</option>
          <option value="convexHull">buffered convex hull</option>
        </select>
      </label>
      {#if env.provider === "boundingBox" || env.provider === "boundingCircle"}
        <label>Margin (mm) <input type="number" step="0.1" bind:value={env.marginMm} oninput={touch} /></label>
      {:else if env.provider === "buffer" || env.provider === "convexHull"}
        <label>Distance (mm) <input type="number" step="0.1" bind:value={env.distanceMm} oninput={touch} /></label>
        {@render joinEditor(
          () => (env.provider === "buffer" || env.provider === "convexHull" ? env.join : undefined),
          (jn) => { if (env.provider === "buffer" || env.provider === "convexHull") env.join = jn; },
        )}
      {/if}
      <fieldset>
        <legend title="the envelope is a region built around everything checked here; use it as A in a subtraction layer (A − B) to get a negative of B; an empty reference set is an error, never a page-sized plane">Reference set</legend>
        {@render operandBoxes(layer.inputs)}
      </fieldset>
    {:else}
    <label>Combine
      <select bind:value={layer.op} onchange={touch} disabled={layer.kind === "linework"}>
        <option value="union">A ∪ B (union of everything checked)</option>
        <option value="intersection">A ∩ B</option>
        <option value="subtraction">A − B</option>
        <option value="xor">A xor B</option>
      </select>
    </label>
    <fieldset>
      <legend>{layer.op === "union" ? "Inputs" : "A (combined by union)"}</legend>
      {@render operandBoxes(layer.inputs)}
    </fieldset>
    {#if layer.op !== "union"}
      <fieldset>
        <legend>B (combined by union){layer.op === "subtraction" ? ", subtracted from A" : ""}</legend>
        {@render operandBoxes(layer.operands)}
        {#if layer.operands.length === 0}<p class="hint">Nothing in B yet: the result is {layer.op === "intersection" ? "empty" : "A unchanged"}.</p>{/if}
      </fieldset>
    {/if}
    {/if}
    {#if layer.kind === "region"}
      <label title="applies to every source input of this layer (layer inputs are regions already)">Closed loops
        <select bind:value={layer.interpretation.loopRule} onchange={touch}>
          <option value="mergeThenNest">merge overlaps, nested = holes</option>
          <option value="evenOdd">even-odd</option>
          <option value="nonZero">non-zero (no holes)</option>
        </select>
      </label>
      <label title="applies to every source input of this layer; to combine a filled layer with a stroked one, give the stroked geometry its own layer and use it as an input here">Open paths
        <select value={strokeMode(layer.interpretation.strokeWidth)} onchange={(e) => setStrokeMode((e.currentTarget as HTMLSelectElement).value)}>
          <option value="original">expand by source width</option>
          <option value="fixed">expand by fixed width</option>
          <option value="none">not allowed (error)</option>
        </select>
      </label>
      {#if typeof layer.interpretation.strokeWidth === "object"}
        <label>Width (mm) <input type="number" step="0.01" min="0.001" bind:value={layer.interpretation.strokeWidth.fixed.mm} oninput={touch} /></label>
      {/if}
    {/if}
    <label class="check" title="runs its passes; a disabled layer still serves as an input"><input type="checkbox" bind:checked={layer.enabled} onchange={touch} /> enabled</label>
    <label class="check" title="never marks: only an input of other layers"><input type="checkbox" bind:checked={layer.referenceOnly} onchange={touch} /> reference only</label>
  {:else if passInfo}
    {@const pass = passInfo.pass}
    <label>Name <input bind:value={pass.name} oninput={touch} /></label>
    <label>Pattern
      <select value={pass.pattern.kind} onchange={(e) => setPatternKind((e.currentTarget as HTMLSelectElement).value)}>
        <option value="contourFill">contour fill</option>
        <option value="boundaryTrace">boundary trace / contour only</option>
        <option value="hatch">parallel hatch</option>
        <option value="crossHatch">crosshatch (several angles)</option>
        <option value="boundaryHatch">boundary shells + hatch</option>
        <option value="centreline">centreline (thin features)</option>
        <option value="passthrough">linework passthrough</option>
      </select>
    </label>
    {#if pass.pattern.kind === "contourFill"}
      <label title={CENTRELINE}>Pitch (mm) <input type="number" step="0.001" min="0.0005" bind:value={pass.pattern.pitchMm} oninput={touch} /></label>
      <label title="depth of the first ring; empty = half the pitch">First offset (mm)
        <input type="number" step="0.001" min="0" placeholder="pitch/2" value={pass.pattern.firstOffsetMm ?? ""}
          oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { if (pass.pattern.kind === "contourFill") pass.pattern.firstOffsetMm = v === "" ? null : Number(v); }); }} />
      </label>
      <label title="inward: each ring after the one outside it; outward: from the core out, each ring before the one outside it; either way the path order keeps it">Ring order
        <select value={pass.pattern.ringOrder ?? "any"} onchange={(e) => { const v = (e.currentTarget as HTMLSelectElement).value as RingOrder; store.update(() => { if (pass.pattern.kind === "contourFill") pass.pattern.ringOrder = v === "any" ? undefined : v; }); }}>
          <option value="any">any (optimizer)</option>
          <option value="inward">inward</option>
          <option value="outward">outward</option>
        </select>
      </label>
      <label class="check" title="join nested rings into one path: a straight connector a pitch long leads into each next ring, marked like the rings (shown and audited)"><input type="checkbox" checked={pass.pattern.link ?? false} onchange={(e) => { const v = (e.currentTarget as HTMLInputElement).checked; store.update(() => { if (pass.pattern.kind === "contourFill") pass.pattern.link = v || undefined; }); }} /> link rings</label>
      <label class="check" title="centrelines in cores too narrow for another ring"><input type="checkbox" bind:checked={pass.pattern.gapFill} onchange={touch} /> gap fill</label>
      {#if pass.pattern.gapFill}
        <label title="where gap fill starts beyond the last ring; empty = half the pitch">End spacing (mm)
          <input type="number" step="0.001" min="0" placeholder="pitch/2" value={pass.pattern.endSpacingMm ?? ""}
            oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { if (pass.pattern.kind === "contourFill") pass.pattern.endSpacingMm = v === "" ? null : Number(v); }); }} />
        </label>
      {/if}
      <label title="tolerance for dropping ring vertices, at most a quarter of the pitch; empty = off">Simplify rings (mm)
        <input type="number" step="0.0005" min="0" placeholder="off" value={pass.pattern.simplifyMm ?? ""}
          oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { if (pass.pattern.kind === "contourFill") pass.pattern.simplifyMm = v === "" ? null : Number(v); }); }} />
      </label>
    {:else if pass.pattern.kind === "boundaryTrace"}
      <label>Side
        <select bind:value={pass.pattern.side} onchange={touch}>
          <option value="on">on the boundary</option>
          <option value="inside">inside</option>
          <option value="outside">outside</option>
        </select>
      </label>
      <label>Offset from boundary (mm) <input type="number" step="0.01" min="0" bind:value={pass.pattern.offsetMm} oninput={touch} /></label>
      <label title="kerf to compensate; half of it is applied outward">Kerf (mm) <input type="number" step="0.001" min="0" bind:value={pass.pattern.kerfMm} oninput={touch} /></label>
      <label>Rings <input type="number" step="1" min="1" bind:value={pass.pattern.rings} oninput={touch} /></label>
      {#if pass.pattern.rings > 1}
        <label>Ring spacing (mm) <input type="number" step="0.01" min="0.001" bind:value={pass.pattern.ringSpacingMm} oninput={touch} /></label>
      {/if}
      {@render joinEditor(
        () => (pass.pattern.kind === "boundaryTrace" ? pass.pattern.join : undefined),
        (jn) => { if (pass.pattern.kind === "boundaryTrace") pass.pattern.join = jn; },
      )}
      <label class="check" title="cutting order: inner contours first, so parts do not drop before their holes are cut"><input type="checkbox" bind:checked={pass.pattern.innerFirst} onchange={touch} /> holes first</label>
      <label class="check"><input type="checkbox" bind:checked={pass.pattern.reverse} onchange={touch} /> reverse direction</label>
    {:else if pass.pattern.kind === "centreline"}
      <label title="single-line engraving: the middle of every stroke, half a stroke width short of its ends; lines shorter than this are left out (empty keeps all)">Minimum length (mm)
        <input type="number" step="0.01" min="0" placeholder="keep all" value={pass.pattern.minLengthMm ?? ""}
          oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { if (pass.pattern.kind === "centreline") pass.pattern.minLengthMm = v === "" ? null : Number(v); }); }} />
      </label>
      <label title="tolerance for dropping vertices; empty = off">Simplify (mm)
        <input type="number" step="0.001" min="0" placeholder="off" value={pass.pattern.simplifyMm ?? ""}
          oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { if (pass.pattern.kind === "centreline") pass.pattern.simplifyMm = v === "" ? null : Number(v); }); }} />
      </label>
    {:else if pass.pattern.kind === "hatch"}
      <label title={CENTRELINE}>Pitch (mm) <input type="number" step="0.001" min="0.0005" bind:value={pass.pattern.pitchMm} oninput={touch} /></label>
      <label title="in document coordinates">Angle (°) <input type="number" step="1" bind:value={pass.pattern.angleDeg} oninput={touch} /></label>
      <label title="offset of the global line lattice, so neighbouring regions line up">Phase (mm) <input type="number" step="0.001" bind:value={pass.pattern.phaseMm} oninput={touch} /></label>
      <label>Inset from boundary (mm) <input type="number" step="0.001" min="0" bind:value={pass.pattern.insetMm} oninput={touch} /></label>
      <label class="check" title="alternate the direction of every other row"><input type="checkbox" bind:checked={pass.pattern.serpentine} onchange={touch} /> serpentine</label>
    {:else if pass.pattern.kind === "crossHatch"}
      <label title={CENTRELINE}>Pitch (mm) <input type="number" step="0.001" min="0.0005" bind:value={pass.pattern.pitchMm} oninput={touch} /></label>
      <label title="comma separated">Angles (°)
        <input placeholder="0, 90" value={pass.pattern.anglesDeg.join(", ")} onchange={(e) => { const v = parseAngles((e.currentTarget as HTMLInputElement).value); store.update(() => { if (pass.pattern.kind === "crossHatch") pass.pattern.anglesDeg = v; }); }} />
      </label>
      <label title="offset of the global line lattice, so neighbouring regions line up">Phase (mm) <input type="number" step="0.001" bind:value={pass.pattern.phaseMm} oninput={touch} /></label>
      <label>Inset from boundary (mm) <input type="number" step="0.001" min="0" bind:value={pass.pattern.insetMm} oninput={touch} /></label>
      <label class="check" title="alternate the direction of every other row"><input type="checkbox" bind:checked={pass.pattern.serpentine} onchange={touch} /> serpentine</label>
    {:else if pass.pattern.kind === "boundaryHatch"}
      <label>Shells <input type="number" step="1" min="0" bind:value={pass.pattern.shells} oninput={touch} /></label>
      <label>Shell pitch (mm) <input type="number" step="0.001" min="0.0005" bind:value={pass.pattern.shellPitchMm} oninput={touch} /></label>
      <label>Gap between shells and hatch (mm) <input type="number" step="0.001" min="0" bind:value={pass.pattern.insetMm} oninput={touch} /></label>
      <label>Hatch pitch (mm) <input type="number" step="0.001" min="0.0005" bind:value={pass.pattern.pitchMm} oninput={touch} /></label>
      <label title="comma separated">Hatch angles (°)
        <input placeholder="0, 90" value={pass.pattern.anglesDeg.join(", ")} onchange={(e) => { const v = parseAngles((e.currentTarget as HTMLInputElement).value); store.update(() => { if (pass.pattern.kind === "boundaryHatch") pass.pattern.anglesDeg = v; }); }} />
      </label>
      <label title="offset of the global line lattice, so neighbouring regions line up">Phase (mm) <input type="number" step="0.001" bind:value={pass.pattern.phaseMm} oninput={touch} /></label>
      <label class="check" title="alternate the direction of every other row"><input type="checkbox" bind:checked={pass.pattern.serpentine} onchange={touch} /> serpentine</label>
    {:else}
      <label class="check"><input type="checkbox" bind:checked={pass.pattern.reverse} onchange={touch} /> reverse direction</label>
      <label title="tolerance for dropping vertices; empty = off">Simplify (mm)
        <input type="number" step="0.001" min="0" placeholder="off" value={pass.pattern.simplifyMm ?? ""}
          oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { if (pass.pattern.kind === "passthrough") pass.pattern.simplifyMm = v === "" ? null : Number(v); }); }} />
      </label>
    {/if}
    <label>Output group
      <select bind:value={pass.group} onchange={touch}>
        {#each store.profile.outputGroups as g (g.id)}<option value={g.id}>{g.name}</option>{/each}
      </select>
    </label>
    <label class="check"><input type="checkbox" bind:checked={pass.enabled} onchange={touch} /> enabled</label>
    {@const audit = store.evaluation?.passes.find((p) => p.id === pass.id)?.audit}
    {#if audit}
      {@const missed = (100 * audit.missedMm2) / Math.max(audit.regionMm2, 1e-12)}
      <p
        class="audit"
        title={`Paths swept at an assumed ${audit.widthMm} mm marking width against the region they fill (a geometric surrogate, not a dose).\nMissed ${audit.missedMm2.toFixed(4)} mm² of ${audit.regionMm2.toFixed(3)} mm²; spill ${audit.spillMm2.toFixed(4)} mm² outside the region; ${audit.shortSegments} path(s) shorter than the width.`}
      >
        Audit at {audit.widthMm} mm: missed {missed.toFixed(2)} %, spill {audit.spillMm2.toFixed(3)} mm²{audit.shortSegments ? `, ${audit.shortSegments} short` : ""}
      </p>
    {:else}
      <p class="hint" title="the group's audit width; fills of region layers only">No coverage audit: set an audit width on group “{store.group(pass.group)?.name ?? pass.group}”.</p>
    {/if}
  {:else if group}
    <label>Name <input bind:value={group.name} oninput={touch} /></label>
    <label>Colour
      <input type="color" value={rgbHex(group.color)} oninput={(e) => store.update(() => { group.color = hexRgb((e.currentTarget as HTMLInputElement).value); })} />
      <code>rgb({group.color.join(", ")})</code>
    </label>
    <label title="stroke width of this group's paths in the PDF, for the importer (not a kerf, pitch or exposure width); empty = the export default">PDF stroke width (mm)
      <input type="number" step="0.0001" min="0.0001" placeholder="default" value={group.strokeWidthMm ?? ""}
        oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { group.strokeWidthMm = v === "" ? null : Number(v); }); }} />
    </label>
    <label title="the effective width each path is assumed to sweep when auditing fills of this group (missed area, spill, short segments); a geometric surrogate, not a dose; empty = no audit">Audit width (mm)
      <input type="number" step="0.001" min="0.001" placeholder="off" value={group.auditWidthMm ?? ""}
        oninput={(e) => { const v = (e.currentTarget as HTMLInputElement).value; store.update(() => { group.auditWidthMm = v === "" ? null : Number(v); }); }} />
    </label>
    <label class="check"><input type="checkbox" bind:checked={group.export} onchange={touch} /> export</label>
    <label class="check"><input type="checkbox" bind:checked={group.visible} onchange={touch} /> visible in preview</label>
    <label>Path order
      <select value={group.orderMode ?? ""} onchange={(e) => { const v = (e.currentTarget as HTMLSelectElement).value; store.update(() => { group.orderMode = v === "" ? null : (v as OptimizerMode); }); }}>
        <option value="">follow profile ({store.profile.optimizer.mode})</option>
        <option value="asGenerated">as generated</option>
        <option value="minimumJump">minimum jump</option>
        <option value="bleedAware">bleed-aware</option>
      </select>
    </label>
  {:else if binding}
    <label>Slot name <input bind:value={binding.name} oninput={touch} /></label>
    <label class="check" title="output is blocked while no imported layer is mapped onto it"><input type="checkbox" bind:checked={binding.required} onchange={touch} /> required</label>
    <fieldset>
      <legend title="the first rule that matches decides; layer-name, function and file-pattern rules travel with the profile to new designs, file + layer rules pin one imported file; patterns are case-insensitive globs (* and ?)">Matching rules</legend>
      {#each binding.rules as rule, i (i)}
        <div class="rule">
          <select value={rule.kind} onchange={(e) => setRuleKind(i, (e.currentTarget as HTMLSelectElement).value)}>
            <option value="layerName">layer name</option>
            <option value="function">layer function</option>
            <option value="fileName">file name pattern</option>
            <option value="fileLayer">file + layer</option>
          </select>
          {#if rule.kind === "fileLayer"}
            <input placeholder="file name" bind:value={rule.file} oninput={touch} />
            <input placeholder="layer name" bind:value={rule.name} oninput={touch} />
          {:else if rule.kind === "layerName"}
            <input placeholder="layer name" bind:value={rule.name} oninput={touch} />
          {:else if rule.kind === "fileName"}
            <input placeholder="*.gtl" bind:value={rule.pattern} oninput={touch} title="case-insensitive; * and ? wildcards" />
          {:else}
            <input placeholder="Copper,L1,Top" bind:value={rule.pattern} oninput={touch} title="Gerber X2 file function or IPC-2581 layer function; * and ? wildcards" />
          {/if}
          <button title="remove rule" onclick={() => removeRule(i)}>×</button>
        </div>
      {/each}
      <button onclick={addRule}>+ rule</button>
    </fieldset>
    <p class="hint">Used by: {store.bindingUsers(binding.id).join(", ") || "no working layer"}</p>
    <button onclick={deleteBinding}>delete slot</button>
  {:else if sourceLayer && sourceFile}
    {@const mapped = store.bindingOf(sourceFile.name, sourceLayer.name)}
    {@const b = sourceLayer.boundsMm}
    <p title="imported layers are read-only; map one onto a profile slot in the Sources list and working layers read from the slot">
      <strong>{sourceLayer.name}</strong> in {sourceFile.name}
    </p>
    <dl class="facts">
      {#if sourceLayer.function}<dt>Function</dt><dd>{sourceLayer.function}</dd>{/if}
      <dt>Paths</dt><dd>{sourceLayer.closedPaths} closed, {sourceLayer.openPaths} open{sourceLayer.hatches ? `, ${sourceLayer.hatches} hatches` : ""}</dd>
      {#if b}<dt>Size</dt><dd title={`from (${b[0].toFixed(3)}, ${b[1].toFixed(3)}) to (${b[2].toFixed(3)}, ${b[3].toFixed(3)}) mm`}>{(b[2] - b[0]).toFixed(3)} × {(b[3] - b[1]).toFixed(3)} mm</dd>{/if}
      <dt>Slot</dt><dd>{mapped ? mapped.name : "not mapped"}</dd>
    </dl>
  {:else}
    <p class="hint">Select something to edit it.</p>
  {/if}
</section>

<style>
  label {
    display: block;
    font-size: 12px;
    margin: 0.35rem 0;
  }
  label.check {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  input:not([type="checkbox"]):not([type="color"]),
  select {
    display: block;
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    font-size: 12px;
    margin-top: 2px;
  }
  fieldset {
    border: 1px solid color-mix(in srgb, CanvasText 20%, transparent);
    margin: 0.4rem 0;
    padding: 0.3rem 0.5rem;
    font-size: 12px;
  }
  .hint {
    font-size: 11px;
    opacity: 0.7;
  }
  .audit {
    margin: 0.4rem 0;
    font-size: 12px;
  }
  .facts {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.1rem 0.6rem;
    margin: 0.4rem 0;
    font-size: 12px;
  }
  .facts dt {
    opacity: 0.7;
  }
  .facts dd {
    margin: 0;
  }
  .rule {
    display: flex;
    gap: 0.3rem;
    align-items: center;
    margin: 0.2rem 0;
  }
  .rule input,
  .rule select {
    width: auto;
    flex: 1;
    display: inline-block;
    margin: 0;
  }
  .rule button {
    background: none;
    border: none;
    cursor: pointer;
    color: inherit;
  }
  code {
    font-size: 11px;
    margin-left: 0.5rem;
  }
</style>
