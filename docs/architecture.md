# Architecture

How the pieces fit, and where a given responsibility lives. Requirements are in
[`spec.md`](spec.md); what each file format reader and exporter supports is in
[`formats.md`](formats.md); engineering decisions with their rationale are in
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
(vertices plus the bulge of each segment) that are drawn and discarded. The canvas draws
arcs natively, so curves stay smooth at any zoom. Preview is never the authoritative
geometry.

## Core crate (`crates/lvp-core`)

Pure Rust, host-testable, no browser dependencies. All lengths in millimetres.

| Module | Responsibility |
|---|---|
| `geom`, `units`, `flatten`, `arc` | Points, boxes, units (DXF `$INSUNITS` mapping), bounded-error arc flattening, DXF bulge/angle semantics. `geom::EPS_MM` is the numerical noise floor. |
| `pline` | Line-and-arc polylines (`cavalier_contours::Polyline<f64>`), construction helpers, flattening, `sanitize` (every polyline entering the kernel). |
| `region` | Planar regions (CCW outers, CW holes) and the n-loop boolean engine built on cavalier's pairwise primitives, with a bounding-box grid (`BBoxGrid`) for pair candidates, winding probes and nesting; offsets via `Shape` with round, mitre or bevel joins (`offset_join`, D25); strokes from side offsets. Clusters of near-coincident boundaries that the exact boolean cannot resolve consistently are checked against, or replaced by, i_overlay's polygon boolean on finely flattened loops. See D7, D18, D21, D30, D34. |
| `source` | The format-independent result of reading a file (`SourceDocument`: layers of `Primitive`s in mm, each layer with an optional declared `function`), format detection from content and extension, the `read_any` dispatcher, and the bounded glob used by slot rules. |
| `dxf::read` / `dxf::write` | DXF subset reader (LINE, ARC, CIRCLE, LWPOLYLINE/POLYLINE, ELLIPSE, SPLINE, HATCH, INSERT, SOLID) with diagnostics for everything else; R2000 writer used for fixtures and later export. |
| `ipc` | IPC-2581 reader (our own, D22) on roxmltree: layers and steps, standard and user primitive dictionaries, line descriptors (round ends stay centrelines, square and flat ends become exact polygons), pads with transforms, positive/negative sets composited in source order, the board profile, `layerFunction,side` as the layer function; components and nets are ignored. |
| `gds` | GDSII stream reader (our own, D22): records, structures, BOUNDARY/PATH/BOX, SREF/AREF flattening with reflection, magnification and rotation, database and user units; paths with flush, square or custom ends become exact mitred polygons, round-ended paths stay centrelines with width; layers are `layer/datatype`; TEXT and NODE are counted. |
| `pdf` | Vector PDF reader (D22): lopdf for the object model, our content-stream interpreter for paths, Béziers (circular ones as exact arcs), fill rules, strokes with scaled widths, `q`/`Q`/`cm`, device colours, clipping on fills, form XObjects and optional-content groups as layers (otherwise the paint colour `#rrggbb`); text, images, shadings and patterns are diagnosed. Points are converted at 25.4/72 mm with `UserUnit`. |
| `svg` | SVG reader on usvg (D22): named top-level groups (Inkscape labels) become layers, fills become loops with the SVG fill rule, strokes become centrelines with their scaled width, Béziers that are circular arcs become exact arcs and other curves are flattened to the read tolerance, clip paths cut fills exactly, y is flipped to the model's y-up frame; text, images, masks, filters and foreign objects are diagnosed, never silently dropped. |
| `gerber` | RS-274X / X2 reader: standard and macro apertures (`gerber::macros`: variables, arithmetic, primitives 1/2/4/5/6/7/20/21/22), draws with arcs in both quadrant modes, regions, step-and-repeat, aperture blocks, LM/LR/LS transforms, dark/clear compositing in source order, `TF.FileFunction` as the layer function. One layer per file, named after the file. |
| `model` | Configuration types = the profile schema (serde, camelCase): source bindings, working layers, passes, output groups, export and optimizer settings. |
| `profile` | Versioned JSON parsing with migrations, structural validation, built-in presets. |
| `eval` | Binding resolution (matched / missing / ambiguous) by layer name, file + layer, file-name glob or layer function, dependency order with cycle rejection, source interpretation (closed loops, stroke expansion, hatches), envelope providers (named boundary, bounding box / circle, buffer, convex hull), boolean composition `union(A) op union(B)` (a layer's parts built in parallel and united in one n-ary union), pattern passes, fingerprint cache. Layers evaluate in dependency waves, the layers of a wave, all passes (with their coverage audits) and all output groups concurrently (D35). |
| `patterns` | Passthrough; inward contour fill (`gapfill`: Voronoi medial-axis centrelines for cores narrower than a pitch, missed-area audit); boundary trace (`trace`: inside / on / outside, kerf, rings, holes first); parallel hatch, crosshatch and boundary + hatch (`hatch`: exact line/arc crossings on a global phase lattice, serpentine). |
| `order` | Path-order plans per output group (D24): precedence-aware nearest-entry construction, or-opt / 2-opt improvement with delta evaluation, orientation and seam DP; raster footprint model for the bleed penalty, statistics (jumps, length, crossings, protected coverage, peak stacking) and the heatmap. Move scans, seam costs, crossings and raster stamping use the thread pool without changing any result (D35). |
| `export` | Export paths per output group in planned order; `export::pdf` writes deterministic 1:1 vector PDFs (strokes only), `export::svg` one Inkscape layer per group on the same page, `export::dxf` one layer per group with arcs kept and model coordinates (nearest ACI colour); `export::pdf` writes deterministic 1:1 vector PDFs (strokes only). |
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
- Parallelism never changes a result: geometry, paths, audits and plans are identical with
  parallelism off and on any number of threads (`tests/parallel_determinism.rs`, D35).
- Preview jumps (`Project::preview_jumps`) are hypothetical straight lines between the
  planned path endpoints of one group; they and the footprint heatmap never reach an
  exporter. Plans never alter mark geometry (D24).

## Wasm adapter (`crates/lvp-wasm`)

Holds one `Project` in a thread local. Every export is a synchronous adapter: JSON strings
for structured data, typed arrays for geometry, byte vectors for files, `JsError` for
failures. The rayon pool is started by the host (`initThreadPool`) once per worker. Its
own global allocator (`alloc`, D36) caches small blocks per thread in front of a shared
dlmalloc heap whose lock sleeps; std's wasm allocator spins and serialises every thread.

## Web app (`web/`)

| Path | Responsibility |
|---|---|
| `src/worker/protocol.ts` | Message kinds and result types; `transferables()` lists buffers to move. |
| `src/worker/geometry.worker.ts` | Instantiates the wasm, starts the pool, dispatches requests. |
| `src/worker/client.ts` | Promise façade with request ids; stale responses are dropped; `latestOnly` for previews. Holds a view of the worker's cancellation and progress block (below). |
| `src/core/types.ts` | TypeScript mirror of the serde types. |
| `src/state/project.svelte.ts` | `ProjectStore`: UI state, editable profile, debounced evaluation with at most one run in flight (a newer edit cancels it), cancel, profile persistence, export; undo and redo over snapshots of the profile after each edit (typing on in one field is one step); the structural edits shared by the panels and the graph. |
| `src/state/profileStore.ts` | IndexedDB for named profiles and the last-used profile; no drawing bytes. |
| `src/state/graph.ts` | The model as a graph (imported layers → slots → working layers → passes → output groups) with each node's status, and its layout: a column per kind of node, layers by dependency depth, ordered within columns by dagre. What a connection drawn on it means for the profile (or why it is refused: cycles, duplicates, a second group on a one-group layer) and what deleting a node or edge removes; unmapping uses a copy of the core's slot-rule matching (`eval::resolve_bindings`, `source::glob_match`) that must change with it. Pure functions, tested with vitest. |
| `src/ui/*.svelte` | Panels from the spec: sources, layers, properties, output groups, profiles bar, export, diagnostics, canvas preview; `GraphView` and `GraphNode` draw the graph tab next to the preview with Svelte Flow, loaded on first use. |
| `sw/coi-serviceworker.js` | Vendored coi-serviceworker, extended to precache the bundle for offline use; emitted by the Vite build with the asset manifest. The page half reloads once the registration is active. |
| `e2e/`, `playwright.config.ts` | Playwright suite against the production bundle in the installed Chrome: workflow (import, evaluate, export, slots, two operand groups) and service worker + offline; `static-server.mjs` serves `dist/` without isolation headers. |

## Data flow of an edit

1. A panel mutates `store.profile` through `store.update(...)`; the store marks it dirty
   and debounces.
2. `evaluate()` posts the whole profile snapshot, then an `evaluate` request carrying a
   fresh epoch. The reply carries an `EvaluationSummary`. If another edit arrives while it
   runs, the store cancels that epoch and evaluates the newest profile once the worker has
   stopped; a superseded reply is never shown.
3. The preview re-fetches only the buffers for visible items at the new revision.

### Cancellation and progress

The worker is busy inside a synchronous wasm call while it imports or evaluates, so it
cannot receive a message. Instead `lvp_core::cancel` keeps a four-word block
(`[cancel, running, done, total]`) in the module's shared memory; the worker hands its
address to the client (`control` request), which maps it onto the same
`SharedArrayBuffer`. Each cancellable request (`evaluate`, `addSource`, `replaceSource`)
runs under an epoch; writing that epoch to `cancel` stops it, queued or running. The core
checks between layers, passes and groups, between rings of a fill, inside booleans, in the
path optimizer's sweeps and in the readers' work budget; a stopped run caches nothing
partial and leaves the previous evaluation (and the profile it was made from) in place for
previews and exports. `done` and `total` count thousandths of a step and drive the
progress bar next to the Cancel button.
4. Export asks the worker for PDF bytes; the browser downloads them. Nothing is uploaded.
