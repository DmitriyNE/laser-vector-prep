# Backlog

Grouped by milestone (`docs/spec.md` §13). Check items off in place; move finished
milestones into `log.md`.

## Open questions for the project owner

- none right now

## Staging (done 2026-10-24, see log)

Decided: synthetic generated fixtures (D10), Svelte (D9), shared-memory wasm with rayon
(D8), Apache-2.0 (D11).

## M1 — vertical slice

- [x] Choose geometry kernel (D7): cavalier_contours
- [x] Kernel wrapper over cavalier_contours (n-loop booleans, offsets, strokes)
- [x] DXF reader for the spec subset with diagnostics; fixture-driven tests
- [x] Source → working layer → output group model with stable IDs; profile schema v1,
      migrations scaffold, validation, PCB preset
- [x] Inward contour fill with medial-axis gap fill; linework passthrough
- [x] PDF export: 1:1, strokes only, colour-separated and per-group
- [x] Profiles in IndexedDB, import/export JSON with conflict handling
- [x] Svelte panels: sources, layers, properties, groups, profiles bar, export, diagnostics
- [x] Offline after load via the vendored coi-serviceworker with precache
- [ ] JSON Schema document for the profile format (serde types are the source of truth
      today; generate or hand-write `docs/profile-schema.json`)
- [ ] Contour fill `end_spacing`; configurable join style once the kernel exposes it
- [x] Benchmark the pairwise-boolean kernel on flash-heavy inputs: a bounding-box grid in
      `resolve` was enough (D21), no kernel change needed
- [ ] Preview: `f32` coordinates relative to the document origin lose precision far from
      it; offset by the bounds minimum if large drawings appear
- [x] Worker crash recovery: a wasm trap replaces the worker and re-imports sources
- [x] Playwright e2e on the installed Chrome (D20): import → evaluate → export through the
      real worker protocol, slot mapping, two-group hatch, service worker + offline
- [ ] Release wasm is 1.74 MB after usvg (was 1.05 MB; lopdf will add more); try
      `wasm-opt -Oz`, feature trimming and lazy-loading the readers as separate modules
- [ ] Worker: cooperative cancellation / progress for long computations (today:
      terminate-and-respawn only; with a shared-memory pool, cancellation must also
      stop rayon work, e.g. via an atomic flag checked between chunks)
- [ ] Check Firefox: module workers + SharedArrayBuffer + the coi-serviceworker reload
- [ ] Decide `--max-memory` (1 GiB today) once real fine-pitch workloads exist
- [ ] Cold `just build` takes ~5 min on an M-series Mac: `-Z build-std` rebuilds the wasm
      std and the release profile uses fat LTO. Incremental rebuilds are seconds. Revisit
      (thin LTO, sccache) if it starts to hurt

## M2 — geometry and layers (done 2026-10-24, see log)

- [x] Dependency DAG with cycle rejection and dependency-aware invalidation (landed in M1)
- [x] Booleans: union / intersection / subtraction / XOR; clip = intersection with B (D17)
- [x] Five domain providers for negative regions as envelope layers (D19)
- [x] Boundary trace, parallel hatch, crosshatch, boundary + hatch
- [x] Profile binding diagnostics (matched / missing / ambiguous / extra); migrations
      deferred until a version is deployed (D17)
- [x] Built-in presets: positive LDI, negative background hatch, general marking, boundary
      cutting, each tested on a fixture
- [ ] Hatch link strokes between rows as visible beam-on geometry with a coverage audit
      (spec §6); today rows are independent paths
- [ ] Preset pitches (20 µm fill, 50 µm marking) are starting points, not measured values;
      revisit once the machine has been characterised

## M3 — breadth and optimization (started 2026-10-24; D22, D23)

Order: Gerber → optimizer → SVG → PDF → GDSII → IPC-2581 → SVG/DXF export.

- [x] Shared source document; file-name glob and layer-function match rules; format
      detection by content
- [x] Gerber RS-274X reader: apertures C/R/O/P, macros with variables and expressions,
      D01/D02/D03, G01/G02/G03 with G74/G75, G36/G37 regions, LP polarity compositing in
      source order, SR, AB, LM/LR/LS, X2 attributes (TF.FileFunction → layer function);
      synthetic Gerber fixtures
- [ ] Gerber: draws with rectangular apertures are approximated by round strokes of the
      aperture's extent (diagnosed); exact swept rectangles if a real file needs them
- [ ] Gerber: deprecated image transforms (OF, SF, MI, AS, IR) and IPNEG are diagnosed, not
      applied
- [x] Path optimizer: nearest-endpoint baseline, 2-opt / or-opt moves, open-path reversal,
      seam choice on loops; modes as-generated / minimum jump / bleed-aware with protected
      regions; jump footprint heatmap; inside-before-outside constraint for cutting passes;
      objective before/after in the UI (D24)
- [ ] Optimizer: seam choice is vertex-granular; splitting a line or arc at the closest
      point would be exact too and shorten the entry jump further
- [ ] Optimizer: the improvement phase is single-threaded; the candidate evaluation is
      embarrassingly parallel if large fills make it slow
- [x] SVG reader via usvg (text refused, clip paths applied as intersections, masks/filters
      diagnosed)
- [ ] SVG: clip paths apply to fills only; stroked and unpainted paths inside clipped groups
      are read unclipped with a warning (needs polyline-by-region clipping)
- [ ] SVG: stroke caps and joins are always round in stroke expansion (butt/square/miter
      diagnosed as info)
- [ ] Vector PDF reader: lopdf object model + own content-stream interpreter (paths,
      transforms, colours, page scale); region vs trajectory interpretation per layer
- [ ] GDSII reader: BOUNDARY, PATH (pathtype ends), BOX, SREF/AREF flattening,
      layer/datatype as layer names, UNITS; TEXT/NODE diagnosed
- [ ] IPC-2581 reader: Layer + LayerFeature sets, standard/user primitive dictionaries,
      line descriptors, polarity, Profile; layerFunction as layer function
- [ ] SVG and DXF export of generated paths per output group
- [ ] Gap fill refinements (`end_spacing`, join style) from the M1 list

## M4 — robustness/release

- [ ] Fuzzing of parsers, geometry audits, large-file performance
- [ ] Cross-browser tests, deployment docs
- [ ] Optional: `nix build` package for the static site (D3)
