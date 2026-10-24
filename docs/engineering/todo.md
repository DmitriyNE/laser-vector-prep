# Backlog

Grouped by milestone (`docs/spec.md` §13). Check items off in place; move finished
milestones into `log.md`.

## Open questions for the project owner

- [ ] Fixtures: `display-pogo-board.DXF`, legacy PDFs/scripts, sample profile JSON are
      referenced by the spec but not in the repo. Where do they come from?
- [ ] UI framework: Svelte, Preact or stay vanilla? (D4)
- [ ] License for the repository.
- [ ] Do we want wasm threads (shared memory, needs nightly `build-std`) or is
      message-passing between plain workers enough? Affects D1/D5.

## Staging (done 2026-10-24, see log)

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
- [ ] Offline after load: our own caching service worker must *also* inject COOP/COEP,
      because only one service worker can control the scope (coi-serviceworker alone
      does not cache)
- [ ] Worker: cooperative cancellation / progress for long computations (today:
      terminate-and-respawn only)
- [ ] Browser e2e test (Playwright) for the import→export loop; verify
      `crossOriginIsolated === true` on the built bundle

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
