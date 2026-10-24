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

## D17 — Layer operations take two operand groups (2026-10-24)

Owner feedback: subtraction as "first checked input minus the rest" depended on click
order. Every binary operation is now `union(A) op union(B)` with `inputs` (A) and
`operands` (B) as two independent checkbox groups; union simply merges both. Clipping (M2)
will be intersection with B.

Schema policy, also from the owner: no migrations before anything is deployed. The schema
stays at version 1 and is redefined in place; the migration table exists but is empty, and
a profile claiming an older version fails with a precise "no migration path" error. The
first real migration is written when a deployed version exists to migrate from.

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

## D19 — Envelopes are a layer kind (2026-10-24)

The spec's five domain providers for inverted regions could have been a property of every
region layer (an implicit domain applied before subtraction) or a layer of their own. Owner
chose a separate `envelope` layer kind: it has a reference set (its inputs) and a provider
(`namedBoundary`, `boundingBox`, `boundingCircle`, `buffer`, `convexHull`), produces a
region, and is used as an ordinary operand, so an inversion stays the plain `A − B` the rest
of the model already has. Consequences: the domain is visible and previewable in the layer
list; one envelope can serve several layers (the negative preset subtracts a buffered copper
from a named board interior); reference-only envelopes are never exported. The point-based
providers (box, circle, hull) work from the reference's points, so open widthless linework
is a valid reference for them; the region-based ones (named boundary, buffer) go through the
normal region interpretation and report open linework the way a region layer does. No
reference geometry at all is `eval.envelope_unbounded`; a margin or distance that collapses
the shape is `eval.envelope_collapsed`. A subtraction whose B reaches outside A reports
`eval.subject_outside_domain` instead of clipping silently.

## D20 — End-to-end tests: Playwright on the installed Chrome (2026-10-24)

Choices: keep the DevTools-protocol scripts as a manual check; Playwright with its own
downloaded browsers; Playwright driving the Chrome already on the machine
(`channel: "chrome"`). Owner chose the last. Rationale: no browser download outside Nix, and
the thing under test is the real production bundle (`web/dist`) with shared memory, the
rayon pool, the service worker and offline mode, which only a real browser exercises.
`just e2e` is separate from `just test` because it needs `just build` and a local Chrome.
The suite runs two servers: Vite preview with COOP/COEP headers, and a header-less static
server for the service-worker path.

Found while writing the suite: the page half of coi-serviceworker reloaded on `updatefound`,
before the (precaching) worker had activated, so a fresh browser profile ended on an
uncontrolled, un-isolated page. It now reloads after `navigator.serviceWorker.ready`, at
most once per session, which also makes the first visit deterministic for users.

## D21 — Boolean engine indexes loop bounding boxes (2026-10-24)

`resolve` compared every loop pair and probed winding against every loop, quadratic in the
number of loops; a dense 5000-pad board took 3 s per boolean. A uniform grid over loop
bounding boxes (`region::BBoxGrid`) now serves pair candidates, winding probes, nesting and
island grouping. Chosen over an R-tree crate because loops on PCB artwork are similar in
size, the grid is deterministic and adds no dependency. Measured on synthetic boards: dense
5000 pads 3.0 s → 121 ms, sparse 240 → 21 ms, one large blob 530 → 92 ms. This closes the
M1 question whether `i_overlay` would be needed for performance.

## D22 — M3 readers: usvg and lopdf as libraries, Gerber (and later GDSII, IPC-2581) our own (2026-10-24)

Spike numbers on top of the 1.05 MB wasm: usvg 0.49 without text +0.52 MB / 22 crates,
lopdf 0.46 +0.75 MB / 61 crates, gerber_parser 0.5 +1.75 MB / 49 crates (regex, chrono,
strum). Owner chose libraries where they buy fidelity we would otherwise get wrong (SVG CSS,
`use`, symbols, viewBox; PDF xref/object streams and filters) and our own parsers where the
grammar is small: RS-274X including aperture macros, step-and-repeat, polarity and X2
attributes. PDF content-stream geometry (path operators, transforms, colours) is our code on
top of lopdf's object model. Owner added GDSII (binary records; own parser) and IPC-2581
(XML via roxmltree; own interpretation of the feature dictionaries) to M3 after the
spec's three formats. Order chosen: Gerber → path optimizer → SVG → PDF → GDSII →
IPC-2581 → SVG/DXF export, each committed and usable on its own.

## D23 — Source layers carry a function; slots match by file-name glob and function (2026-10-24)

One-layer-per-file formats (Gerber) and formats that declare what a layer is (Gerber X2
`TF.FileFunction`, IPC-2581 `layerFunction`) need rules beyond the exact layer name. Owner
chose: a case-insensitive glob on the file name (`*` and `?` only, no regex) and a glob on
a per-layer `function` string that each reader fills in when the format declares it
(`Copper,L1,Top`, `Profile,NP`, …). Both are ordinary `MatchRule` variants, so the
first-match-wins slot semantics and the explicit-mapping UI are unchanged. The source
document type is shared by all readers (`source::SourceDocument`); a file is detected by
extension and content, never by trusting the extension alone.

## D24 — Path order planning: per-group plans, raster footprint, squared coverage (2026-10-24)

The spec's optimizer (§7) is implemented as a plan per output group that reorders paths,
reverses open paths and rotates the seam of closed loops, never touching mark geometry.
Construction is a deterministic nearest-entry walk over a grid of entry points (both ends
of open paths, every vertex of a loop) that only offers paths whose precedence
predecessors are done; cutting passes emit "hole before its outer" constraints. Improvement
is or-opt and 2-opt with spatial neighbour candidates and delta evaluation of the affected
jumps only; a prefix reversal is allowed when no start point is assumed so the first path is
not pinned. Orientation and seam are then settled by a small dynamic programme.

The bleed penalty is geometric: each jump's footprint is a capsule of the configured width,
accumulated on a raster whose cells are flagged protected by a scanline fill of the
protected layers' regions. The objective is jump length plus `bleed_weight × Σ coverage²`
over protected cells, scaled to millimetres. Squaring is what turns "avoid hotspots" into a
number: stacking two footprints on one spot costs four, spreading them costs two. The same
raster is the preview heatmap. Nothing here is a dose and the labels say so.

## D25 — Corner joins, end spacing and ring simplification (2026-10-25)

The spec asks for a join style on boundary traces and buffers and lists "end spacing" and a
simplifying tolerance for contour fill without defining them; cavalier_contours only offsets
with round joins. Choices made without a separate question (owner may redefine):

- **Joins** (`round`, `miter { limit }`, `bevel`) on boundary traces and on the buffer and
  buffered-hull envelopes. Built from the exact round offset: each join arc (radius equal
  to the offset distance, centred on a corner of the original boundary) is replaced by the
  extended edges or by its chord. Arcs of the original geometry are never touched. A mitre
  whose tip exceeds `limit × distance` or would cross other geometry becomes a bevel; a
  bevel's chord lies within the offset distance of the corner and cannot cross anything.
  Editing the outline avoids booleans on coincident arcs, which the n-loop engine handles
  poorly.
- **End spacing** is the counterpart of the first offset: the distance beyond the last ring
  from which the centreline gap fill must still cover the core. Default `pitch / 2`, the
  previous behaviour. Larger values keep gap-fill centrelines away from the last ring.
- **Simplify** (contour fill rings): arcs whose sagitta is within half the tolerance become
  chords, then Douglas-Peucker runs over straight runs with the other half; larger arcs are
  kept exactly. Clamped to a quarter of the pitch so neighbouring rings cannot touch.

## D26 — Arcs stay arcs end to end (2026-10-25)

Owner question: "Why is everything tessellated into line segments?" The geometry model was
already lines plus exact arcs, but the preview flattened at a fixed 5 µm (visible facets
past about 200 px/mm, exactly the zoom needed to inspect a 20 µm fill), PDF and SVG
exports flattened at 1 µm, and SVG/PDF circles arrived as Béziers and were flattened at
import. Owner approved removing all four:

- **Preview** buffers carry vertices and bulges; the canvas draws arcs with `arc()` and
  falls back to a line only when the sagitta is under a quarter pixel.
- **SVG export** writes arc commands (exact).
- **PDF export** writes cubic Béziers, split so each piece is within the curve tolerance
  (default 1 µm; the former "arc flattening tolerance" setting) of the arc. PDF has no arc
  operator; this is the closest exact-enough form and far smaller than polylines.
- **SVG/PDF import** recognises a cubic as a circular arc when its end tangents are
  symmetric about the chord and it stays within `max(1 µm, 0.03 % of the radius)` of the
  circle. The relative allowance is the error the standard Bézier approximation of an arc
  itself makes, so a circle drawn as four Béziers comes back as the exact circle it stands
  for rather than as a polygon within 1 µm of the Bézier. Ellipses and general Béziers
  are still flattened.

## D27 — CI on GitHub Actions with the Nix dev shell; deploy to GitHub Pages (2026-10-25)

Owner asked for CI with GitHub Actions and publishing to GitHub Pages. CI runs the same
`nix develop -c just …` commands as a developer (lint, test, build, browser tests), so the
pinned toolchain is the one CI uses and there is no second set of setup steps to drift.
Browser tests use the runner's preinstalled Google Chrome, matching D20 (no Playwright
browser download). Only cargo/npm downloads and `target/` are cached; the Nix store is
fetched from cache.nixos.org each run until that proves slow. Pages deploys with the
official `upload-pages-artifact` / `deploy-pages` actions after a green run on `main`; the
static bundle already works from a sub-path and the service worker supplies the isolation
headers Pages cannot send (D5).

## D28 — Default path order: bleed-aware for the PCB presets, minimum jump otherwise (2026-10-25)

Owner request. The positive PCB preset exposes photoresist by laser direct imaging, where
jump footprints over areas that must stay unexposed matter, so it plans bleed-aware and
gets two reference layers: an artwork box (copper bounding box + 1 mm) and "Keep clear
(background)" = box − copper, which it protects. The negative preset ablates the
background; jumps run with the beam not Q-switched, far too weak to ablate, so ordering is
not physically critical there. It also plans bleed-aware, protecting the copper, and the
owner accepts either that or minimum jump. General marking, boundary cutting and new
profiles default to minimum jump (the model default changed from as-generated). Cost on
the 20 µm board: about 0.5 s per full evaluation in bleed-aware mode against 60–90 ms
otherwise; the plan is cached while the paths do not change.
