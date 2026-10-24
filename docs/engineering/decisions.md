# Decisions

Short records of choices that are not obvious from the code. Add a new numbered entry
rather than rewriting an old one; mark superseded entries as such.

## D1 — Toolchain from nixpkgs only, no Rust overlay (2026-10-24) — superseded by D8

nixpkgs' `rustc` bundles the `wasm32-unknown-unknown` std, so a single `nixpkgs-unstable`
input was enough for a single-threaded wasm build. Superseded the same day when the owner
asked for shared-memory wasm, which needs `-Z build-std` and therefore nightly (D8).

## D2 — `wasm-bindgen` crate pinned exactly to the nixpkgs CLI (2026-10-24)

`wasm-bindgen-cli` and the `wasm-bindgen` crate must match exactly. We pin
`wasm-bindgen = "=0.2.129"` in `Cargo.toml` and let `just check-wasm-bindgen` fail the
build on drift. `wasm-pack` was rejected because it downloads a CLI binary at build time.

## D3 — Reproducibility via pinned shell + lockfiles; no `nix build` package yet (2026-10-24)

`nix develop -c just build` is the one-command build. A full `nix build` derivation
would add `npmDepsHash`/cargo-vendoring maintenance for little gain at this stage.
Can be added later without changing the layout.

## D4 — Vanilla TypeScript for the staging shell; UI framework undecided (2026-10-24) — superseded by D9

Deferred the framework choice to the owner; answered the same day: Svelte (D9).

## D5 — Cross-origin isolation: headers in dev, coi-serviceworker in production (2026-10-24)

Vite dev/preview servers send COOP/COEP headers directly. The static bundle uses
`coi-serviceworker.js` (npm package, copied to `web/public/` by `just coi`, never
bundled). Isolation is required, not optional: the wasm module uses shared memory (D8),
so the app does not even spawn the geometry worker until `crossOriginIsolated` is true
(the service worker reloads the page once on first visit to get there).

Amended for offline support (M1): only one service worker can control the scope, so the
coi-serviceworker is vendored (`web/sw/coi-serviceworker.js`, MIT) and its worker half
extended with a cache-first precache of the bundle; the Vite build injects the asset list.
No workbox or PWA plugin.

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

## D8 — Date-pinned nightly via rust-overlay; shared-memory wasm with rayon (2026-10-24)

The owner wants shared memory and threads from the start. That needs the wasm32 std
rebuilt with atomics (`-Z build-std=std,panic_abort`), hence nightly. `rust-toolchain.toml`
pins `nightly-2026-10-24` and is read by both rustup and the flake
(`rust-bin.fromRustupToolchainFile`), so there is one source of truth for the toolchain.
The nightly bundles `rust-lld`; no separate `lld`.

Linker flags live in `.cargo/config.toml` under `[target.wasm32-unknown-unknown]` only, so
host builds and tests stay ordinary. Current nightlies no longer add the shared-memory
linker flags when `atomics` is enabled, so we pass them ourselves: `--shared-memory`,
`--import-memory`, `--max-memory=1 GiB`, and `--export=` for `__wasm_init_tls`,
`__tls_size`, `__tls_align`, `__tls_base`, which wasm-bindgen's threading transform
requires. Threads come from `wasm-bindgen-rayon` with its default (bundler) helper:
Vite emits the self-spawning pool worker as its own chunk and resolves the glue through a
`package.json` that `just wasm` writes into the generated directory. The `no-bundler`
feature does not work under Vite: its blob self-fetch re-executes whichever chunk the
helper was inlined into.

The pool is created inside the geometry worker (`initThreadPool(hardwareConcurrency)`),
never on the main thread. Node-side tests load the wasm but cannot start the pool, so
anything that fans out to rayon keeps a sequential path below a size threshold.

## D9 — Svelte 5 for the UI (2026-10-24)

Owner's choice. Runes-mode components, `svelte-check` as the type checker (it covers
`.ts` too, so `tsc` is not run separately). svelte-check 4 requires TypeScript ≤ 6, which
pins `typescript` to `^6` for now. The worker protocol and client stay framework-free.

## D10 — Regression fixtures are synthetic and generated (2026-10-24)

The historical pogo-board files are not used (owner decision). Fixtures are produced by
`crates/lvp-fixtures` from the core's own DXF writer, deterministically, with a
`manifest.json` stating what a correct pipeline must observe (bounds in mm, entity counts,
island/hole counts, unit conversions, block transforms). A test fails when the committed
files differ from the generator's output, so fixtures are never hand-edited. The writer
targets DXF R2000 with handles and a full table skeleton; ezdxf audits the output clean and
agrees with the manifest's counts and bounds.

## D11 — License: Apache-2.0 (2026-10-24)

Owner's choice; `LICENSE` at the root, `license` fields in Cargo and npm manifests.

## D7 — Geometry kernel: cavalier_contours (2026-10-24, owner decision)

Candidates were spiked against fixture-shaped inputs: `i_overlay` 9.1 (pure Rust, integer
arithmetic internally, all booleans and fill rules, offsets with bevel/miter/round joins,
stroke expansion, line clipping; contour-fill loop on the 20×20 mm hole fixture at 0.5 mm
pitch in 1.6 ms, union of 5000 random rectangles in 90 ms), `cavalier_contours` 0.9
(line+arc polylines with arcs preserved through offsets, pairwise closed-polyline
union/intersection/difference, shape offsets with islands, segment intersection
primitives), and Clipper2 via C++ (rejected: needs a C++ wasm toolchain incompatible with
our `build-std` flow; the Rust `clipper2` crate wraps the C++ via `cc`).

The owner chose `cavalier_contours` for arc fidelity: contour rings around round pads stay
true arcs instead of flattened chords. Consequences we accept and own:

- Multi-shape booleans (union of many primitives, XOR, clipping) are built from pairwise
  `boolean()` calls driven by a spatial index; XOR = (A∪B) − (A∩B).
- Hatch is our scanline intersection against line and arc segments (exact endpoints on
  arcs), not a kernel clip.
- Stroke expansion of linework is a union of per-segment capsules (arcs are native).
- Gap fill uses a Voronoi medial axis (`boostvoronoi`) of segment-approximated slivers.
- Each segment's |bulge| must be ≤ 1 (half circle); importers split larger arcs.

`i_overlay` stays the documented fallback if pairwise booleans turn out too slow on
flash-heavy inputs; the spike numbers above are the yardstick.

## D12 — The Rust core owns the project model (2026-10-24)

Parsed sources, the working-layer DAG with its evaluation cache, pattern passes, the
route planner, exporters, and the versioned profile schema with migrations all live in
`lvp-core`. The worker holds one `Project`; TypeScript keeps UI state plus a JSON mirror
of the configuration, sends whole-config updates, and receives diagnostics and preview
buffers tagged with a revision. The spec's acceptance tests become cargo tests.

## D13 — Fill strategies are our own code on top of kernel primitives (2026-10-24)

No library implements the spec's fill semantics (pitch, first offset, phase in document
mm, edge coverage, hole handling). The kernel supplies offsets, booleans and intersection
primitives; `lvp-core::patterns` implements inward contour fill, gap fill, passthrough,
boundary trace, parallel/cross hatch and boundary+hatch, each proptested in mm.

## D14 — DXF parsing: own parser (2026-10-24)

Owner initially chose the `dxf` crate (0.6.2). Investigation: its entity set is generated
from a spec in which HATCH is commented out, so hatches cannot be represented; it also
pulls `image`, `chrono`, `encoding_rs` and `uuid` (needs the `js` feature) into the wasm.
Re-asked with that fact; owner chose our own parser for the spec's subset, with explicit
diagnostics for everything outside it and proptest round-trips against `dxf::write`.

## D15 — Kernel inputs are sanitised at the boundary (2026-10-24)

`cavalier_contours` asserts (debug) and misbehaves (release) on zero-length arc segments.
Every polyline entering a boolean, offset or stroke passes `pline::sanitize` (coincident
vertices within `REPEAT_EPS = 1e-6 mm` merged, closing duplicate removed), and arc
capsules whose inner offset arc would collapse fall back to flattened line capsules.
Found by the export pipeline test on the board fixture.

## D16 — Gap fill: Voronoi input must be provably valid (2026-10-24)

`boostvoronoi` requires segments that meet only at endpoints and panics (debug) or
returns garbage (release) otherwise; on wasm a panic aborts the worker. Sliver boundaries
are therefore snapped to the finest i32 grid (never a coarse one, which folded thin
slivers onto themselves) and checked with exact integer orientation tests before the
diagram is built; invalid slivers are skipped and reported (`pattern.gap_fill_skipped`).
Curved Voronoi edges are chord-approximated, which is exact enough for sub-pitch slivers.

## D17 — Layer operations take two operand groups; schema v2 (2026-10-24)

Owner feedback: subtraction as "first checked input minus the rest" depended on click
order. Every binary operation is now `union(A) op union(B)` with `inputs` (A) and
`operands` (B) as two independent checkbox groups; union simply merges both. This is the
first real schema migration (v1 → v2): the first v1 input stays in A, the rest move to B.
Clipping (M2) will be intersection with B. Saved v1 profiles are migrated on load by the
worker, and the UI reads the migrated form back rather than trusting stored JSON.

## D18 — Strokes are built from side offsets, not capsule unions (2026-10-24)

The first `stroke` unioned one capsule per segment. Adjacent capsules share identical cap
circles and touch neighbouring arcs tangentially, and a DXF bulge rounded to six decimals
turned exact tangencies into near-tangent intersections that the n-loop boolean could not
classify consistently; the hole of a stroked outline vanished silently. Strokes now come
from cavalier offsets: a closed path is `offset(+r) − offset(−r)` of its filled loop, an
open path is its two side offsets joined by semicircular caps; capsules remain only as a
fallback when an offset fragments. The stitcher also asserts in debug builds when a chain
does not close, so an inconsistent boolean fails a test instead of returning a partial
result. Contour fill's uncovered cores are likewise derived by offsetting the region past
the last ring rather than by stroking the rings.
