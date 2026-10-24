# Decisions

Short records of choices that are not obvious from the code. Add a new numbered entry
rather than rewriting an old one; mark superseded entries as such.

## D1 — Toolchain from nixpkgs only, no Rust overlay (2026-10-24)

nixpkgs' `rustc` bundles the `wasm32-unknown-unknown` std, so `rustc`, `cargo`,
`clippy`, `rustfmt`, `rust-analyzer`, `wasm-bindgen-cli`, `binaryen`, `nodejs_24` and
`just` all come from a single `nixpkgs-unstable` input. One flake input, one lockfile.
nixpkgs' rustc does not ship `rust-lld`, so `lld` is in the shell for the wasm32 link.
Revisit only if we need a nightly feature (e.g. wasm threads via `-Z build-std`).

## D2 — `wasm-bindgen` crate pinned exactly to the nixpkgs CLI (2026-10-24)

`wasm-bindgen-cli` and the `wasm-bindgen` crate must match exactly. We pin
`wasm-bindgen = "=0.2.129"` in `Cargo.toml` and let `just check-wasm-bindgen` fail the
build on drift. `wasm-pack` was rejected because it downloads a CLI binary at build time.

## D3 — Reproducibility via pinned shell + lockfiles; no `nix build` package yet (2026-10-24)

`nix develop -c just build` is the one-command build. A full `nix build` derivation
would add `npmDepsHash`/cargo-vendoring maintenance for little gain at this stage.
Can be added later without changing the layout.

## D4 — Vanilla TypeScript for the staging shell; UI framework undecided (2026-10-24)

The staging slice (status header + canvas smoke test) does not need a framework. The
choice between Svelte, Preact and vanilla is deferred to the user before the layer /
properties panels are built (M1). Nothing in `web/src/worker` depends on this choice.

## D5 — Cross-origin isolation: headers in dev, coi-serviceworker in production (2026-10-24)

Vite dev/preview servers send COOP/COEP headers directly. The static bundle uses
`coi-serviceworker.js` (npm package, copied to `web/public/` by `just coi`, never
bundled). Plain Web Workers do not need isolation; it is set up now so that
`SharedArrayBuffer` / shared wasm memory is available when the geometry pipeline
needs it.

## D6 — Numerical policy: explicit tolerances, 1 pm noise floor (2026-10-24)

Arcs are flattened to polylines with vertices exactly on the curve, chordal deviation
(sagitta) ≤ caller tolerance, and no segment spanning more than a quarter turn. The
segment count is the minimum satisfying both. Endpoints are exact (interpolated on the
angle, not accumulated). Tolerance is always an explicit parameter in mm; there is no
global default in the core.

`geom::EPS_MM = 1e-9` mm is the floating-point noise floor: differences below it are
snapped, not diagnosed (found by proptest: shrinking a degenerate box by exactly its
expansion margin inverts it by one ulp). Anything above it is a real geometric event
and is reported to the caller. The integer scaling for the boolean kernel (D7) must be
chosen so its resolution is well above this floor.

## D7 — Geometry kernel: to be chosen in M1 (open)

Spec prefers Clipper2 WASM. With a Rust core the candidates are: `i_overlay` (pure Rust
booleans, integer-robust, includes offsetting), `cavalier_contours` (polyline offsetting
with true arcs), or Clipper2 C++ via `cxx` (painful on wasm32-unknown-unknown). Decide
with a small benchmark/robustness spike on the pogo-board fixture before M1 contour fill.
