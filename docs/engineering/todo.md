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
- [ ] Release wasm is ~0.9 MB (0.36 MB gzip); try `wasm-opt -Oz` and feature trimming
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

## M3 — breadth and optimization

- [ ] Gerber RS-274X, SVG, vector PDF subset readers
- [ ] Path ordering modes, bleed-aware optimizer + heatmap
- [ ] SVG/DXF export

## M4 — robustness/release

- [ ] Fuzzing of parsers, geometry audits, large-file performance
- [ ] Cross-browser tests, deployment docs
- [ ] Optional: `nix build` package for the static site (D3)
