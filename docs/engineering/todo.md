# Backlog

Grouped by milestone (`docs/spec.md` §13). Check items off in place; move finished
milestones into `log.md`.

## Open questions for the project owner

- none right now

## Staging (done 2026-10-24, see log)

Decided: synthetic generated fixtures (D10), Svelte (D9), shared-memory wasm with rayon
(D8), Apache-2.0 (D11).

## M1 — vertical slice

- [ ] Choose geometry kernel (D7) with a spike on the fixture
- [ ] Canonical geometry model: polylines/arcs/regions with provenance, mm units
- [ ] DXF reader: units, layers, LINE/ARC/CIRCLE/LWPOLYLINE(bulge, width)/POLYLINE,
      ELLIPSE/SPLINE flattening, HATCH, INSERT; detect REGION/ACIS
- [ ] Source → working layer → output group model with stable IDs
- [ ] Inward contour fill (pitch, first offset, end spacing, gap-fill)
- [ ] Linework passthrough
- [ ] PDF export: 1:1, mm→pt, nonzero RGB strokes, path-stream order, per-group option
- [ ] Profile v1 schema (JSON Schema), save/load in IndexedDB, import/export JSON
- [ ] Layer panel + properties panel (after framework decision)
- [ ] DXF reader; its tests consume `fixtures/manifest.json` expectations
- [ ] Replace the `App.svelte` smoke test (arc + parallel bbox) with the real panels
- [ ] Offline after load: our own caching service worker must *also* inject COOP/COEP,
      because only one service worker can control the scope (coi-serviceworker alone
      does not cache)
- [ ] Worker: cooperative cancellation / progress for long computations (today:
      terminate-and-respawn only; with a shared-memory pool, cancellation must also
      stop rayon work, e.g. via an atomic flag checked between chunks)
- [ ] Browser e2e test (Playwright) for the import→export loop, thread pool start and
      `crossOriginIsolated === true`; today this is a manual DevTools-protocol check
- [ ] Check Firefox: module workers + SharedArrayBuffer + the coi-serviceworker reload
- [ ] Decide `--max-memory` (1 GiB today) once real fine-pitch workloads exist

## M2 — geometry and layers

- [ ] Dependency DAG with cycle rejection and dependency-aware invalidation
- [ ] Booleans: union / intersection / subtraction / XOR / clip
- [ ] Five domain providers for negative regions
- [ ] Parallel hatch, crosshatch
- [ ] Profile binding diagnostics (matched/missing/ambiguous/extra), migrations

## M3 — breadth and optimization

- [ ] Gerber RS-274X, SVG, vector PDF subset readers
- [ ] Boundary+hatch, path ordering modes, bleed-aware optimizer + heatmap
- [ ] SVG/DXF export

## M4 — robustness/release

- [ ] Fuzzing of parsers, geometry audits, large-file performance
- [ ] Cross-browser tests, deployment docs
- [ ] Optional: `nix build` package for the static site (D3)
