# Architecture

How the pieces fit, and where a given responsibility lives. Requirements are in
[`spec.md`](spec.md); engineering decisions with their rationale are in
[`engineering/decisions.md`](engineering/decisions.md).

## Shape of the system

```
 browser main thread (Svelte)                 geometry worker (wasm, shared memory)
 ┌──────────────────────────────┐   messages   ┌──────────────────────────────────┐
 │ ProjectStore                 │ ───────────▶ │ lvp-wasm: thin adapters          │
 │  · editable Profile (JSON)   │  profile,    │   └─ lvp-core::project::Project  │
 │  · sources & evaluation      │  files,      │        ├─ Sources (DxfDocument)   │
 │    summaries (JSON)          │  requests    │        ├─ Profile                 │
 │  · selection, visibility     │ ◀─────────── │        ├─ Evaluator (DAG + cache) │
 │ Panels, canvas preview       │  summaries,  │        └─ last Evaluation         │
 │ IndexedDB: profiles only     │  previews,   │   rayon pool: N web workers       │
 └──────────────────────────────┘  PDF bytes   └──────────────────────────────────┘
```

The worker owns all geometry. The UI never sees a polyline as data: it receives
summaries (counts, areas, bounds, diagnostics) and flat `Float32Array` preview buffers
that are drawn and discarded. Preview is never the authoritative geometry.

## Core crate (`crates/lvp-core`)

Pure Rust, host-testable, no browser dependencies. All lengths in millimetres.

| Module | Responsibility |
|---|---|
| `geom`, `units`, `flatten`, `arc` | Points, boxes, units (DXF `$INSUNITS` mapping), bounded-error arc flattening, DXF bulge/angle semantics. `geom::EPS_MM` is the numerical noise floor. |
| `pline` | Line-and-arc polylines (`cavalier_contours::Polyline<f64>`), construction helpers, flattening, `sanitize` (every polyline entering the kernel). |
| `region` | Planar regions (CCW outers, CW holes) and the n-loop boolean engine built on cavalier's pairwise primitives, with a bounding-box grid (`BBoxGrid`) for pair candidates, winding probes and nesting; offsets via `Shape`; strokes from side offsets. See D7, D18, D21. |
| `source` | The format-independent result of reading a file (`SourceDocument`: layers of `Primitive`s in mm, each layer with an optional declared `function`), format detection from content and extension, the `read_any` dispatcher, and the bounded glob used by slot rules. |
| `dxf::read` / `dxf::write` | DXF subset reader (LINE, ARC, CIRCLE, LWPOLYLINE/POLYLINE, ELLIPSE, SPLINE, HATCH, INSERT, SOLID) with diagnostics for everything else; R2000 writer used for fixtures and later export. |
| `gerber` | RS-274X / X2 reader: standard and macro apertures (`gerber::macros`: variables, arithmetic, primitives 1/2/4/5/6/7/20/21/22), draws with arcs in both quadrant modes, regions, step-and-repeat, aperture blocks, LM/LR/LS transforms, dark/clear compositing in source order, `TF.FileFunction` as the layer function. One layer per file, named after the file. |
| `model` | Configuration types = the profile schema (serde, camelCase): source bindings, working layers, passes, output groups, export and optimizer settings. |
| `profile` | Versioned JSON parsing with migrations, structural validation, built-in presets. |
| `eval` | Binding resolution (matched / missing / ambiguous) by layer name, file + layer, file-name glob or layer function, dependency order with cycle rejection, source interpretation (closed loops, stroke expansion, hatches), envelope providers (named boundary, bounding box / circle, buffer, convex hull), boolean composition `union(A) op union(B)`, pattern passes, fingerprint cache. |
| `patterns` | Passthrough; inward contour fill (`gapfill`: Voronoi medial-axis centrelines for cores narrower than a pitch, missed-area audit); boundary trace (`trace`: inside / on / outside, kerf, rings, holes first); parallel hatch, crosshatch and boundary + hatch (`hatch`: exact line/arc crossings on a global phase lattice, serpentine). |
| `export` | Flattened export paths per output group; `export::pdf` writes deterministic 1:1 vector PDFs (strokes only). |
| `project` | The worker-side facade: sources, profile, evaluation, JSON summaries, preview buffers, jump previews, export. |
| `diag` | `Diagnostic { severity, code, message, context }` used everywhere; codes are stable dotted identifiers. |

Invariants worth knowing:

- Open linework never becomes a region implicitly; a region layer with widthless open
  paths reports `eval.open_linework_in_region` and stays empty.
- Layer list order never changes semantics; evaluation follows the dependency graph and
  rejects cycles with `eval.cycle`.
- A layer combines two operand groups, each a union: `union(inputs) op union(operands)`.
  Neither list order nor click order matters.
- A missing required binding blocks dependent output (`eval.binding_missing`); nothing is
  substituted silently.
- Domains are explicit envelope layers (D19). A subtraction whose B reaches outside A
  reports `eval.subject_outside_domain`; an envelope without reference geometry is
  `eval.envelope_unbounded`. No bounding box is ever assumed.
- Preview jumps (`Project::preview_jumps`) are hypothetical and never reach an exporter.

## Wasm adapter (`crates/lvp-wasm`)

Holds one `Project` in a thread local. Every export is a synchronous adapter: JSON strings
for structured data, typed arrays for geometry, byte vectors for files, `JsError` for
failures. The rayon pool is started by the host (`initThreadPool`) once per worker.

## Web app (`web/`)

| Path | Responsibility |
|---|---|
| `src/worker/protocol.ts` | Message kinds and result types; `transferables()` lists buffers to move. |
| `src/worker/geometry.worker.ts` | Instantiates the wasm, starts the pool, dispatches requests. |
| `src/worker/client.ts` | Promise façade with request ids; stale responses are dropped; `latestOnly` for previews. |
| `src/core/types.ts` | TypeScript mirror of the serde types. |
| `src/state/project.svelte.ts` | `ProjectStore`: UI state, editable profile, debounced evaluation that only accepts the newest result, profile persistence, export. |
| `src/state/profileStore.ts` | IndexedDB for named profiles and the last-used profile; no drawing bytes. |
| `src/ui/*.svelte` | Panels from the spec: sources, layers, properties, output groups, profiles bar, export, diagnostics, canvas preview. |
| `sw/coi-serviceworker.js` | Vendored coi-serviceworker, extended to precache the bundle for offline use; emitted by the Vite build with the asset manifest. The page half reloads once the registration is active. |
| `e2e/`, `playwright.config.ts` | Playwright suite against the production bundle in the installed Chrome: workflow (import, evaluate, export, slots, two operand groups) and service worker + offline; `static-server.mjs` serves `dist/` without isolation headers. |

## Data flow of an edit

1. A panel mutates `store.profile` through `store.update(...)`; the store marks it dirty
   and debounces.
2. `evaluate()` posts the whole profile snapshot, then an `evaluate` request. The reply
   carries an `EvaluationSummary`; a reply for a superseded request is ignored.
3. The preview re-fetches only the buffers for visible items at the new revision.
4. Export asks the worker for PDF bytes; the browser downloads them. Nothing is uploaded.
