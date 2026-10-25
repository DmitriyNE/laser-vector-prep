# Backlog

Grouped by milestone (`docs/spec.md` §13). Check items off in place; move finished
milestones into `log.md`.

## Open questions for the project owner

- none right now

## Staging (done 2026-10-24, see log)

Decided: synthetic generated fixtures (D10), Svelte (D9), shared-memory wasm with rayon
(D8), Apache-2.0 (D11).

## M1 — vertical slice

Open M1 items on the profile schema, preview precision, cancellation and Firefox moved to M4.

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
- [x] Contour fill end spacing and ring simplification; join styles for traces and buffers
      (D25)
- [x] Benchmark the pairwise-boolean kernel on flash-heavy inputs: a bounding-box grid in
      `resolve` was enough (D21), no kernel change needed
- [x] Worker crash recovery: a wasm trap replaces the worker and re-imports sources
- [x] Playwright e2e on the installed Chrome (D20): import → evaluate → export through the
      real worker protocol, slot mapping, two-group hatch, service worker + offline
- [x] Release wasm is 2.49 MB after usvg and lopdf (was 1.05 MB): left as is (D29)
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

## M3 — breadth and optimization (done 2026-10-25; D22–D25; open items are follow-ups)

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
- [x] Vector PDF reader: lopdf object model + own content-stream interpreter (paths,
      transforms, colours, page scale); region vs trajectory interpretation per layer
- [ ] PDF: clipping applies to fills only (strokes diagnosed); dash patterns and non-round
      caps are read solid/round; encrypted files are refused
- [x] GDSII reader: BOUNDARY, PATH (pathtype ends), BOX, SREF/AREF flattening,
      layer/datatype as layer names, UNITS; TEXT/NODE diagnosed
- [ ] GDSII: a layer map (number → name) could name layers like the EDA tool does; today
      they are `layer/datatype`
- [ ] GDSII: angles with absolute-magnification/angle STRANS bits are treated as relative
- [x] IPC-2581 reader: Layer + LayerFeature sets, standard/user primitive dictionaries,
      line descriptors, polarity, Profile; layerFunction as layer function
- [ ] IPC-2581: Butterfly, Moire and Marking primitives are skipped with a warning; drill
      holes are read as circles on their layer
- [x] SVG and DXF export of generated paths per output group
- [ ] DXF export: true colour (group code 420) alongside the nearest ACI once the writer
      supports it; SVG export could keep arcs as `A` commands instead of flattening
- [x] Gap fill refinements: end spacing, ring simplification, join styles (D25)
- [x] User documentation of every format's supported subset (`docs/formats.md`)

## M4 — robustness/release (D29)

- [x] cargo-fuzz targets (six readers + end to end), crasher regressions in `just test`,
      `just fuzz`
- [x] Nightly fuzz workflow in CI
- [x] Gerber panic on non-UTF-8 bytes; compositing hang on extreme coordinates (bounded
      grid, out-of-range primitives dropped at import)
- [x] Triage the first two 10-minute runs of every target: boolean robustness (D30,
      DEF-5/6/10/11), import work budget and PDF form cycles (D31, DEF-9)
- [x] Expansion budget across readers (D31)
- [ ] Booleans: near-coincident edges and arcs whose vertices do not line up can still
      leave a boundary chain open; reported as a warning (`Region::incomplete`), not fixed
- [x] Large inputs: contour fill now fills islands independently and in parallel
      (`par::map`); a 4×8 panel of the board evaluates in 0.5 s natively (was 1.1 s), an
      8×8 panel (22 k paths) in 0.8 s natively and 2.6 s in Chrome
- [ ] Still sequential on large inputs: layer compositing, the order statistics' coverage
      raster, and bleed-aware improvement (about 2 s of the 4.6 s for the 8×8 panel)
- [x] Cooperative cancellation and progress through a control block in shared wasm
      memory; a new edit cancels the running evaluation; Cancel button and progress bar
      (spec §12 #19)
- [x] Firefox: Playwright Firefox from nixpkgs, e2e suite on Chrome and Firefox locally and
      in CI. The "extra reload" was Firefox refusing the test port (4190 is a Fetch "bad
      port"); the slow start does not reproduce locally (0.8 s through the service worker).
      Firefox evaluates the board about 2× slower than Chrome (wasm speed)
- [ ] Re-check the live site's first visit in Firefox after deploying (14.6 s at kickoff)
- [x] Coverage audit (spec §10): spill, missed area, short segments against a per-group
      effective marking width; schema v2 with the first migration (D32)
- [x] JSON Schema of the profile format generated from the serde types (schemars), with a
      staleness test and validation of the presets (`docs/profile-schema.json`)
- [ ] Preview precision far from the origin
- [ ] `just package` builds a versioned tarball of the site; `nix build` package
- [ ] Later: publish GitHub Releases (deferred by the owner)
- [ ] Deployment documentation (headers or service worker, sub-paths, offline)
- [x] CI on GitHub Actions and deployment to GitHub Pages (D27)
- [x] Bundle size: decided to leave it (D29)
- [ ] Cache the Nix store in CI if fetching the dev shell becomes the slow step
